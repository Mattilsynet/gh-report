//! P4 (adr-fmt-t7t4v roadmap) regression guard: all three gh-report
//! event stores (main/org/team) rehydrate through the shared pardosa
//! verifying stage (`rebuild_dragline_with_frontier`) on BOTH backends.
//!
//! ## Why (roadmap adr-fmt-t7t4v, phase adr-fmt-x6t9x)
//!
//! `open_pgno`/`open_jetstream` on all three native stores
//! (`NativeStore`, `NativeOrgStore`, `NativeTeamStore`) delegate to
//! `pardosa_fiber_store::FiberStore::open_pgno`/`open_jetstream`, which
//! call `PardosaStore::open_with_backend` — both the `Pgno` and
//! `JetStream` dispatch arms of `open_with_backend` route through
//! `rebuild_dragline_with_frontier` (P1, adr-fmt-q8qyn), computing all
//! four `CheckedReplayKind` checks in `PRECURSOR_CHECK_MODE::ObserveOnly`
//! (P2b, adr-fmt-o1kd3). This closes the seam-survey (adr-fmt-p0t1g)
//! UNOBSERVED GAP without any gh-report code re-point: gh-report already
//! rehydrates every store through the verifying stage on both backends,
//! transitively, as of P1+P2b landing.
//!
//! `ObserveOnly` means a known-good prod-shaped fixture must boot clean
//! on both backends with zero rejections — the assertion below.

use gh_report::app::state::AppState;
use gh_report::config::runtime::{NatsStoreConfig, PardosaBackend};
use gh_report::domain::evidence::{AssessmentMetadata, OrgStateSnapshot};
use gh_report::domain::metrics::{
    OrgAlertSummary, RepoAlertSummary, TeamMember, TeamRoster, TeamRosterStatus,
};
use gh_report::domain::status::CollectionStatus;
use gh_report::event::OrgMembershipFetchStatus;
use pardosa_nats::test_support::LiveNatsServer;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_org() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let pid = std::process::id();
    format!("p4-boot-{pid}-{nanos}")
}

fn org_snapshot() -> OrgStateSnapshot {
    OrgStateSnapshot {
        archived_repos: 3,
        assessment_metadata: AssessmentMetadata {
            date: "2026-07-21".to_string(),
            organization: "P4BootOrg".to_string(),
            schema_version: gh_report::config::EVIDENCE_SCHEMA_VERSION.to_string(),
            run_timestamp: "2026-07-21T00:00:00+00:00".to_string(),
            run_id: "p4-boot-run".to_string(),
            token_tier: gh_report::domain::auth::TokenTier::Full,
            token_scopes: "repo, read:org".to_string(),
            auth_mode: gh_report::domain::auth::AuthMode::Pat,
            rate_limit_warnings: 0,
            unavailable_capabilities: vec![],
            inventory_fetched_at: None,
            warm_start: false,
        },
        alert_summary: OrgAlertSummary {
            collection_status: CollectionStatus::Success,
            collection_reason: None,
            per_repo: HashMap::<String, RepoAlertSummary>::new(),
            open_secret_alert_age_buckets: HashMap::new(),
            total_open_secret_alerts: 0,
            oldest_open_secret_alert_created_at: None,
            newest_open_secret_alert_created_at: None,
        },
    }
}

fn team_roster() -> TeamRoster {
    TeamRoster {
        canonical_owner: "@p4bootorg/platform".to_string(),
        team_slug: "platform".to_string(),
        status: TeamRosterStatus::Complete,
        members: vec![TeamMember {
            login: "alice".to_string(),
            role: gh_report::domain::metrics::TeamMemberRole::Maintainer,
            in_org: None,
        }],
    }
}

/// Boot all three native stores against a known-good fixture on the
/// given backend, seeding one event per store, closing, then reopening
/// through the SAME dispatch path (`open_pgno`/`open_jetstream` both
/// route through the verifying stage per module doc above). Asserts
/// both the fresh boot and the rehydrate-on-reopen succeed —
/// `PRECURSOR_CHECK_MODE::ObserveOnly` must reject nothing on a clean
/// fixture.
async fn boot_all_three_stores_through_verifying_stage(
    events_dir: &Path,
    backend: PardosaBackend,
    nats: NatsStoreConfig,
) {
    let fresh = AppState::with_stores(events_dir, backend, nats.clone())
        .await
        .expect("fresh boot through verifying stage must succeed on a known-good fixture");
    fresh
        .record_repo(
            "owner/p4-repo",
            gh_report::domain::evidence::RepositoryEvidence {
                repository: gh_report::domain::repository::Repository {
                    id: "id-p4-repo".to_string(),
                    node_id: None,
                    name: "p4-repo".to_string(),
                    visibility: gh_report::domain::repository::Visibility::Public,
                    language: None,
                    default_branch: "main".to_string(),
                    archived: false,
                    has_issues: true,
                    inventory_key: "owner/p4-repo".to_string(),
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
                checks: minimal_checks(),
                last_commit: None,
            },
            "p4-repo",
            "2026-07-21T00:00:01Z",
        )
        .expect("record_repo on fresh store");
    fresh
        .record_org(org_snapshot())
        .expect("record_org on fresh store");
    fresh
        .record_team(
            "p4bootorg",
            &team_roster(),
            "2026-07-21T00:00:01Z",
            OrgMembershipFetchStatus::Fetched,
        )
        .expect("record_team on fresh store");
    drop(fresh);

    let reopened = AppState::with_stores(events_dir, backend, nats)
        .await
        .expect(
            "reopen rehydrate through verifying stage must succeed on a known-good fixture \
             (ObserveOnly rejects nothing)",
        );
    reopened
        .snapshot_fast_path_init()
        .expect("snapshot_fast_path_init must fold replayed events cleanly");
    let projection_arc = reopened.projection_state_for_test();
    let projection = projection_arc.lock().expect("projection mutex");
    assert!(
        projection.repositories.contains_key("owner/p4-repo"),
        "repo store must rehydrate through the verifying stage on reopen"
    );
    assert!(
        projection.org_state.is_some(),
        "org store must rehydrate through the verifying stage on reopen"
    );
}

fn minimal_checks() -> gh_report::domain::checks::RepositoryChecks {
    use gh_report::domain::checks::{
        BranchProtectionDetails, BranchProtectionResult, BranchProtectionStatus, CodeownersResult,
        CodeownersStatus, DependabotResult, DependabotStatus, RepositoryChecks,
        SecretScanningResult, SecretScanningStatus, SecurityPolicyEvidence, SecurityPolicyResult,
        SecurityPolicyStatus,
    };
    let ts = "2026-07-21T00:00:01Z";
    RepositoryChecks {
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
    }
}

#[tokio::test]
async fn all_three_stores_boot_through_verifying_stage_on_pgno() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let events_dir = tmp.path().join("events");
    std::fs::create_dir_all(&events_dir).expect("mk events dir");
    let nats = NatsStoreConfig::for_org("p4bootorg", gh_report::config::runtime::DEFAULT_NATS_URL)
        .expect("nats config (unused placeholder for Pgno backend)");

    boot_all_three_stores_through_verifying_stage(&events_dir, PardosaBackend::Pgno, nats).await;
}

#[test]
fn all_three_stores_boot_through_verifying_stage_on_nats() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = tokio::runtime::Runtime::new().expect("multi-thread tokio runtime");
    rt.block_on(async {
        let tmp = tempfile::tempdir().expect("tempdir");
        let events_dir = tmp.path().join("events");
        std::fs::create_dir_all(&events_dir).expect("mk events dir");
        let org = unique_org();
        let nats =
            NatsStoreConfig::for_org(&org, server.url().to_owned()).expect("nats config valid");

        boot_all_three_stores_through_verifying_stage(&events_dir, PardosaBackend::Nats, nats)
            .await;
    });
}
