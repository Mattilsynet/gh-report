//! Warm-start regression: archived public repos with security-policy
//! evidence survive event-log replay and reflect correctly in cached
//! aggregate metrics from projection state.
//!
//! Guards the two-axis invariant (see `aggregate_metrics` and
//! `build_collection_statistics` in `aggregate/metrics.rs`):
//!
//! - `security_policy_coverage` denominator counts **all public**
//!   repos, including archived.
//! - `branch_protection_coverage` (and other "active-only"
//!   coverages) denominator counts **non-archived** repos only.
//! - `CollectionStatistics::archived_repos` is non-zero when the
//!   replayed projection state contains an archived entry.
//!
//! Unit tests cover each axis in isolation. This integration test
//! adds the higher-level contract: the invariants survive a
//! cross-handle warm start, where events are written through
//! `pardosa-eventstore` (`MsgpackFileStore`), the store handle drops
//! (releasing `RunLock` — the closest in-process analogue to a
//! restart), and state rehydrates through `AppState::with_stores` +
//! `snapshot_fast_path_init` (the chain `gh-report` runs on boot).

use gh_report::aggregate::metrics::{aggregate_metrics, build_collection_statistics};
use gh_report::app::state::AppState;
use gh_report::domain::checks::{
    BranchProtectionDetails, BranchProtectionResult, BranchProtectionStatus, CodeownersResult,
    CodeownersStatus, DependabotResult, DependabotStatus, RepositoryChecks, SecretScanningResult,
    SecretScanningStatus, SecurityPolicyEvidence, SecurityPolicyResult, SecurityPolicyStatus,
};
use gh_report::domain::evidence::RepositoryEvidence;
use gh_report::domain::repository::{Repository, Visibility};

fn default_nats_store_config() -> gh_report::config::runtime::NatsStoreConfig {
    gh_report::config::runtime::NatsStoreConfig::for_org(
        "org",
        gh_report::config::runtime::DEFAULT_NATS_URL,
    )
    .unwrap()
}

#[tokio::test]
async fn warm_start_replay_preserves_archived_public_security_policy_in_aggregate_metrics() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let events_dir = tmp.path().join("events");
    std::fs::create_dir_all(&events_dir).expect("mk events dir");

    seed_repo_evaluated_envelopes(&events_dir).await;

    let app_state = AppState::with_stores(
        &events_dir,
        gh_report::config::runtime::PardosaBackend::Pgno,
        default_nats_store_config(),
    )
    .await
    .expect("with_stores");
    app_state
        .snapshot_fast_path_init()
        .expect("snapshot_fast_path_init");

    let repositories: Vec<RepositoryEvidence> = {
        let projection_arc = app_state.projection_state_for_test();
        let projection = projection_arc.lock().expect("projection mutex");
        projection.repositories.values().cloned().collect()
    };

    assert_eq!(
        repositories.len(),
        3,
        "replay must rehydrate all three RepoEvaluated envelopes \
         into projection_state.repositories; got keys: {:?}",
        repositories
            .iter()
            .map(|r| r.repository.inventory_key.clone())
            .collect::<Vec<_>>(),
    );

    let archived = repositories
        .iter()
        .find(|r| r.repository.name == "archived-pub")
        .expect("archived-pub entry must survive replay");
    assert!(
        archived.repository.archived,
        "archived flag must survive replay"
    );
    assert_eq!(
        archived.repository.visibility,
        Visibility::Public,
        "visibility must survive replay"
    );
    assert_eq!(
        archived.checks.security_policy.status,
        SecurityPolicyStatus::Pass,
        "security_policy evidence must survive replay"
    );

    let stats = build_collection_statistics(&repositories);
    assert_eq!(
        stats.archived_repos, 1,
        "CollectionStatistics::archived_repos must reflect the \
         replayed archived entry; got {stats:?}",
    );
    assert_eq!(
        stats.total_repos, 2,
        "CollectionStatistics::total_repos counts only non-archived; \
         got {stats:?}",
    );
    assert_eq!(
        stats.public_repos, 1,
        "CollectionStatistics::public_repos counts only non-archived \
         public; got {stats:?}",
    );
    assert_eq!(
        stats.private_repos, 1,
        "CollectionStatistics::private_repos counts only non-archived \
         private; got {stats:?}",
    );

    let metrics = aggregate_metrics(&repositories);
    assert_eq!(
        metrics.security_policy_coverage.denominator, 2,
        "security_policy_coverage denominator must include the \
         archived public repo (policy-surface axis)",
    );
    assert_eq!(
        metrics.security_policy_coverage.numerator, 2,
        "both public repos pass security_policy (archived + active)",
    );
    assert_eq!(
        metrics.branch_protection_coverage.denominator, 2,
        "branch_protection_coverage denominator must exclude the \
         archived repo (active-only axis); got {metrics:?}",
    );
    assert_eq!(
        metrics.secret_scanning_coverage.denominator, 2,
        "secret_scanning_coverage denominator must include the archived \
         public repo, mirroring the security_policy policy-surface axis \
         (org-page secret-scanning is public-only, archived included; \
         UF2-D/UF2-7 only flips the OWNER-page denominator, not this org-page one)",
    );
    assert_eq!(
        metrics.dependabot_security_updates_coverage.denominator, 2,
        "dependabot_security_updates_coverage denominator must exclude archived",
    );
    assert_eq!(
        metrics.codeowners_coverage.denominator, 2,
        "codeowners_coverage denominator must exclude archived",
    );
}

/// CHE-0082:R5 (amended, commit c272237): a private/internal
/// branch-protection 404 with no controls is genuine absence (a
/// governance Fail, counted in the denominator) exactly like the
/// public case, matching codeowners/dependabot's uniform 404-as-Absent
/// treatment. Regression for the gap in `adr-fmt-vc97r` item 5: no test
/// previously asserted `branch_protection_coverage.denominator` ==
/// `codeowners_coverage.denominator` == dependabot's denominator when a
/// private non-archived repo has a genuine (non-permission) absence.
#[test]
fn branch_protection_denominator_matches_codeowners_and_dependabot_for_private_404_absence() {
    let repositories = private_404_absence_fixture();

    let metrics = aggregate_metrics(&repositories);

    assert_eq!(
        metrics.branch_protection_coverage.denominator, 2,
        "branch_protection_coverage denominator must count both non-archived \
         repos, including the private one with a genuine (non-permission) \
         404 absence; got {metrics:?}",
    );
    assert_eq!(
        metrics.branch_protection_coverage.denominator, metrics.codeowners_coverage.denominator,
        "branch_protection and codeowners denominators must match for the \
         same all-non-archived population",
    );
    assert_eq!(
        metrics.branch_protection_coverage.denominator,
        metrics.dependabot_security_updates_coverage.denominator,
        "branch_protection and dependabot denominators must match for the \
         same all-non-archived population",
    );
}

/// Builds a fixture where a private repo's branch-protection reason
/// mirrors what `classify_failure_reason`
/// (`collector::branch_protection`) emits for an absent-control 404
/// after the CHE-0082:R5 amendment: `NotFoundAbsent`, same as the
/// public case.
fn private_404_absence_fixture() -> Vec<RepositoryEvidence> {
    let ts = "2026-05-20T00:00:00Z";
    [
        (
            "public-active",
            Visibility::Public,
            SecurityPolicyStatus::Pass,
        ),
        (
            "private-active",
            Visibility::Private,
            SecurityPolicyStatus::NotApplicable,
        ),
    ]
    .into_iter()
    .map(
        |(name, visibility, security_policy_status)| RepositoryEvidence {
            repository: Repository {
                id: format!("id-{name}"),
                node_id: None,
                name: name.to_string(),
                visibility,
                language: None,
                default_branch: "main".to_string(),
                archived: false,
                has_issues: true,
                inventory_key: format!("owner/{name}"),
                updated_at: None,
                pushed_at: None,
                created_at: None,
                description: None,
                fork: false,
                is_empty: false,
                html_url: None,
                topics: vec![],
                license_spdx: None,
            },
            checks: RepositoryChecks {
                security_policy: SecurityPolicyResult {
                    status: security_policy_status,
                    evidence: SecurityPolicyEvidence::Setting,
                    path: None,
                    timestamp: ts.to_string(),
                },
                secret_scanning: SecretScanningResult {
                    status: SecretScanningStatus::Enabled,
                    has_open_alerts: Some(false),
                    alerts_observable: true,
                    reason: None,
                    timestamp: ts.to_string(),
                },
                dependabot_security_updates: DependabotResult {
                    status: DependabotStatus::Enabled,
                    reason: None,
                    timestamp: ts.to_string(),
                },
                branch_protection: BranchProtectionResult {
                    status: BranchProtectionStatus::Fail,
                    details: BranchProtectionDetails {
                        default_branch: "main".to_string(),
                        has_pr: None,
                        required_reviewers: None,
                        has_status_checks: None,
                        admin_equivalent: None,
                        has_broad_bypass: None,
                        reason: Some("not_found_absent".to_string()),
                        reason_kind: Some(
                            gh_report::domain::checks::CollectionFailureReason::NotFoundAbsent,
                        ),
                        http_status: Some(404),
                        force_push_blocked: None,
                        deletion_blocked: None,
                    },
                    timestamp: ts.to_string(),
                },
                codeowners: CodeownersResult {
                    status: CodeownersStatus::Absent,
                    path: None,
                    timestamp: ts.to_string(),
                    parsed: None,
                    truncation: None,
                },
            },
            last_commit: None,
        },
    )
    .collect()
}

async fn seed_repo_evaluated_envelopes(events_dir: &std::path::Path) {
    let state = AppState::with_stores(
        events_dir,
        gh_report::config::runtime::PardosaBackend::Pgno,
        default_nats_store_config(),
    )
    .await
    .expect("with_stores");

    for (slug, visibility, archived, policy_status, source_ts) in [
        (
            "archived-pub",
            Visibility::Public,
            true,
            SecurityPolicyStatus::Pass,
            "2026-05-20T00:00:00Z",
        ),
        (
            "active-pub",
            Visibility::Public,
            false,
            SecurityPolicyStatus::Pass,
            "2026-05-20T00:00:01Z",
        ),
        (
            "active-priv",
            Visibility::Private,
            false,
            SecurityPolicyStatus::NotApplicable,
            "2026-05-20T00:00:02Z",
        ),
    ] {
        state
            .record_repo(
                &format!("owner/{slug}"),
                evidence_for(
                    slug,
                    visibility,
                    archived,
                    policy_status,
                    SecurityPolicyEvidence::Setting,
                ),
                slug,
                source_ts,
            )
            .unwrap_or_else(|e| panic!("seed RepositoryStateCaptured for {slug}: {e:?}"));
    }
}

fn evidence_for(
    name: &str,
    visibility: Visibility,
    archived: bool,
    policy_status: SecurityPolicyStatus,
    policy_evidence: SecurityPolicyEvidence,
) -> RepositoryEvidence {
    let ts = "2026-05-20T00:00:00Z";
    RepositoryEvidence {
        repository: Repository {
            id: format!("id-{name}"),
            node_id: None,
            name: name.to_string(),
            visibility,
            language: None,
            default_branch: "main".to_string(),
            archived,
            has_issues: true,
            inventory_key: format!("owner/{name}"),
            updated_at: None,
            pushed_at: None,
            created_at: None,
            description: None,
            fork: false,
            is_empty: false,
            html_url: None,
            topics: vec![],
            license_spdx: None,
        },
        checks: RepositoryChecks {
            security_policy: SecurityPolicyResult {
                status: policy_status,
                evidence: policy_evidence,
                path: None,
                timestamp: ts.to_string(),
            },
            secret_scanning: SecretScanningResult {
                status: SecretScanningStatus::Enabled,
                has_open_alerts: Some(false),
                alerts_observable: true,
                reason: None,
                timestamp: ts.to_string(),
            },
            dependabot_security_updates: DependabotResult {
                status: DependabotStatus::Enabled,
                reason: None,
                timestamp: ts.to_string(),
            },
            branch_protection: BranchProtectionResult {
                status: BranchProtectionStatus::Pass,
                details: BranchProtectionDetails {
                    default_branch: "main".to_string(),
                    has_pr: Some(true),
                    required_reviewers: Some(1),
                    has_status_checks: Some(true),
                    admin_equivalent: Some(true),
                    has_broad_bypass: Some(false),
                    reason: None,
                    reason_kind: None,
                    http_status: None,
                    force_push_blocked: Some(true),
                    deletion_blocked: Some(true),
                },
                timestamp: ts.to_string(),
            },
            codeowners: CodeownersResult {
                status: CodeownersStatus::Conforming,
                path: Some(".github/CODEOWNERS".to_string()),
                timestamp: ts.to_string(),
                parsed: None,
                truncation: None,
            },
        },
        last_commit: None,
    }
}
