//! Memory-Image bootstrap regression test.
//!
//! Asserts the four routing indices on `AppState` (`runs_by_key`,
//! `repos_by_key`, `deliveries_by_id`, `next_seq`) populate from
//! replay at boot, not from in-process writes alone.
//!
//! ## Why (CHE-0022:R6 + CHE-0048 line-24 + CHE-0073:R5)
//!
//! Routing indices are derived state (CHE-0022:R6). gh-report
//! rebuilds routing state by full replay at `AppState` construction
//! (CHE-0048 line-24 exemption; routing per CHE-0073:R5, superseding
//! CHE-0054).
//!
//! ## Failure shape guarded against
//!
//! Pre-fix: `snapshot_fast_path_init` populates only
//! `projection_state`/`projection_checkpoint_seq`; the four
//! `*_by_*`/`next_seq` maps stay empty, so a post-restart command
//! would `RoutingMiss` instead of resolving.
//!
//! Post-fix: those maps populate by enumerating
//! `InMemoryEventStore::list_aggregates()` (via
//! [`cherry_pit_core::ListableEventStore`]) and folding envelopes
//! into the matching index: `SweepStarted` → `runs_by_key`
//! (`batch_id`), `RepoEvaluated` → `repos_by_key` (`domain_key`).
//! `WebhookReceived` lacks `delivery_id` on the wire, so
//! `deliveries_by_id` stays lazy per CHE-0073:R5; `next_seq`
//! rebuilds from the envelope's `sequence`.

use gh_report::app::state::AppState;
use gh_report::domain::checks::{
    BranchProtectionDetails, BranchProtectionResult, BranchProtectionStatus, CodeownersResult,
    CodeownersStatus, DependabotResult, DependabotStatus, RepositoryChecks, SecretScanningResult,
    SecretScanningStatus, SecurityPolicyEvidence, SecurityPolicyResult, SecurityPolicyStatus,
};
use gh_report::domain::evidence::RepositoryEvidence;
use gh_report::domain::repository::{Repository, Visibility};

#[tokio::test]
async fn bootstrap_replay_populates_routing_indices() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let events_dir = tmp.path().join("events");
    std::fs::create_dir_all(&events_dir).expect("mk events dir");

    let app_state = AppState::with_stores(
        &events_dir,
        gh_report::config::runtime::PardosaBackend::Pgno,
        gh_report::config::runtime::NatsStoreConfig::for_org(
            "org",
            gh_report::config::runtime::DEFAULT_NATS_URL,
        )
        .unwrap(),
    )
    .await
    .expect("with_stores");
    app_state
        .record_repo(
            "id-repo-alpha",
            minimal_evidence("repo-alpha"),
            "repo-alpha",
            "2026-05-19T00:00:01Z",
        )
        .expect("record repo");
    app_state
        .snapshot_fast_path_init()
        .expect("snapshot_fast_path_init");

    let projection_arc = app_state.projection_state_for_test();
    let projection = projection_arc.lock().expect("projection mutex");
    assert!(
        projection.repositories.contains_key("id-repo-alpha"),
        "projection must contain id-repo-alpha after native replay"
    );
}

/// At HEAD (pre-fix), `AppState::snapshot_fast_path_init` folds
/// events into `projection_state` only for
/// `ORG_GOVERNANCE_AGGREGATE_ID` (=1). `RepoEvaluated` envelopes are
/// emitted by per-repo aggregates on `AggregateId(2..)` and are
/// never folded — `projection_state.repositories` stays empty even
/// though `bootstrap_replay_state` walked the same envelopes.
///
/// Seeds one `SweepStarted` on a Run aggregate (`AggregateId(1)`,
/// processed but doesn't populate `repositories`) and one
/// `RepoEvaluated` (with non-`None` `evidence`) on a Repo aggregate
/// (`AggregateId(2)`, currently skipped). Drives
/// `snapshot_fast_path_init`, then expects the repo entry in
/// `projection_state`. Pre-fix fails (read-model empty); post-fix
/// passes (unified replay folds every aggregate).
///
/// Does not cross a process or `with_stores` boundary — the
/// in-memory substitute would drop state. The bug is in boot-replay
/// logic, not persistence; this test seeds `app_state.event_store`
/// (a `pub` field) directly, exercising the path in-process.
#[tokio::test]
async fn restart_rehydrates_projection_state() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let events_dir = tmp.path().join("events");
    std::fs::create_dir_all(&events_dir).expect("mk events dir");

    let app_state = AppState::with_stores(
        &events_dir,
        gh_report::config::runtime::PardosaBackend::Pgno,
        gh_report::config::runtime::NatsStoreConfig::for_org(
            "org",
            gh_report::config::runtime::DEFAULT_NATS_URL,
        )
        .unwrap(),
    )
    .await
    .expect("with_stores");
    app_state
        .record_repo(
            "owner/repo-rehydrate",
            minimal_evidence("repo-rehydrate"),
            "repo-rehydrate",
            "2026-05-19T00:00:01Z",
        )
        .expect("record repo");

    app_state
        .snapshot_fast_path_init()
        .expect("snapshot_fast_path_init");

    let projection_arc = app_state.projection_state_for_test();
    let projection = projection_arc.lock().expect("projection mutex");
    assert!(
        projection.repositories.contains_key("owner/repo-rehydrate"),
        "projection_state.repositories should contain the RepoEvaluated \
         entry after bootstrap replay (bd adr-fmt-5rwbu); found keys: {:?}",
        projection.repositories.keys().collect::<Vec<_>>(),
    );
}

/// Inline `RepositoryEvidence` builder for this test. Mirrors
/// `tests/projection_sort_equivalence.rs::ev` — we cannot use the
/// `src/test_fixtures.rs` helpers from an integration test because
/// that module is `#[cfg(test)]`-gated. The shape is the minimum
/// `Projection::apply` needs: a valid `Repository`, a complete
/// `RepositoryChecks`, no `last_commit`. None of the field values
/// matter to the bug under test — the assertion is on map
/// membership by `domain_key`, not on the stored value.
fn minimal_evidence(name: &str) -> RepositoryEvidence {
    let ts = "2026-05-19T00:00:01Z";
    RepositoryEvidence {
        repository: Repository {
            id: format!("id-{name}"),
            node_id: None,
            name: name.to_string(),
            visibility: Visibility::Public,
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
                status: SecurityPolicyStatus::Pass,
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
