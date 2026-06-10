//! Memory-Image bootstrap regression test (Track 7.5, M3).
//!
//! Asserts that the four routing indices on `AppState` (`runs_by_key`,
//! `repos_by_key`, `deliveries_by_id`, `next_seq`) populate from
//! event-log replay at boot — not from in-process writes alone.
//!
//! ## Why this test exists (CHE-0022:R6 + CHE-0048 line-24 exemption + CHE-0054:R5)
//!
//! Routing indices are derived state (CHE-0022:R6 forbids derived state
//! in event payloads). gh-report retires `baseline.msgpack` (commit
//! `63236ac`) and rebuilds in-memory routing state by full event-log
//! replay at `AppState` construction (CHE-0048 line-24 exemption +
//! CHE-0054:R5 amended in this mission: lazy → eager).
//!
//! ## Failure shape we are guarding against
//!
//! Pre-fix: `snapshot_fast_path_init` populates only `projection_state`
//! and `projection_checkpoint_seq`; the four `*_by_*` / `next_seq` maps
//! stay empty (`HashMap::new()` at construction in `state.rs:417-420`,
//! `:521-524`, `:758-761`); any post-restart command targeting a
//! known aggregate would `RoutingMiss` instead of resolving the
//! aggregate id.
//!
//! Post-fix: those maps are populated by enumerating
//! `InMemoryEventStore::list_aggregates()` (via the
//! [`cherry_pit_core::ListableEventStore`] trait) and folding each
//! aggregate's envelopes into the index that matches its variant.
//!
//! ## Routing rules verified
//!
//! | Variant                | Index populated        | Key source             |
//! |------------------------|------------------------|------------------------|
//! | `SweepStarted`         | `runs_by_key`          | `batch_id`             |
//! | `RepoEvaluated`        | `repos_by_key`         | `domain_key`           |
//! | `WebhookReceived`      | (see note below)       | n/a — see CHE-0054:R5  |
//! | (terminal/progress)    | (no new index entry)   | n/a                    |
//!
//! Note on `WebhookReceived`: the event payload does not carry the
//! `delivery_id` (it lives only on the `RecordDelivery` command).
//! `deliveries_by_id` therefore cannot be rebuilt from the event
//! stream and remains lazy-populated per the amended CHE-0054:R5
//! ("lazy fallback retained only for indices whose routing key is
//! not on the wire"). `next_seq`, however, is rebuildable because
//! the envelope itself carries `sequence`.

use std::sync::Arc;

use cherry_pit_core::{CorrelationContext, EventStore};
use gh_report::app::state::{AppState, EventStoreImpl};
use gh_report::domain::checks::{
    BranchProtectionDetails, BranchProtectionResult, BranchProtectionStatus, CodeownersResult,
    CodeownersStatus, DependabotResult, DependabotStatus, RepositoryChecks, SecretScanningResult,
    SecretScanningStatus, SecurityPolicyEvidence, SecurityPolicyResult, SecurityPolicyStatus,
};
use gh_report::domain::events::DomainEvent;
use gh_report::domain::evidence::RepositoryEvidence;
use gh_report::domain::repository::{Repository, Visibility};

#[tokio::test]
async fn bootstrap_replay_populates_routing_indices() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let events_dir = tmp.path().join("events");
    let projections_dir = tmp.path().join("projections");
    std::fs::create_dir_all(&events_dir).expect("mk events dir");
    std::fs::create_dir_all(&projections_dir).expect("mk projections dir");

    {
        let store = Arc::new(EventStoreImpl::create_pgno(&events_dir.join("events.pgno")).unwrap());
        let ctx = CorrelationContext::none();

        let run_event = DomainEvent::SweepStarted {
            org: "test-org".into(),
            repo_count: 3,
            batch_id: "batch-replay-001".into(),
            timestamp: "2026-05-19T00:00:00Z".into(),
            snapshot_signature: None,
        };
        let (_run_id, _) = store
            .create(vec![run_event], ctx.clone())
            .await
            .expect("create Run aggregate");

        let repo_event = DomainEvent::RepoEvaluated {
            domain_key: "id-repo-alpha".into(),
            repo_name: "repo-alpha".into(),
            success: true,
            source: "scheduled_batch".into(),
            duration_ms: 42,
            timestamp: "2026-05-19T00:00:01Z".into(),
            evidence: None,
        };
        let (_repo_id, _) = store
            .create(vec![repo_event], ctx)
            .await
            .expect("create Repo aggregate");
    }

    let app_state = AppState::with_stores(
        &events_dir,
        projections_dir,
        gh_report::config::runtime::PardosaBackend::Pgno,
    )
        .await
        .expect("with_stores");
    app_state
        .snapshot_fast_path_init()
        .await
        .expect("snapshot_fast_path_init");

    let runs_arc = app_state.runs_by_key_for_test();
    let runs = runs_arc.lock().expect("runs_by_key lock");
    assert!(
        runs.contains_key("batch-replay-001"),
        "runs_by_key must contain 'batch-replay-001' after replay; got keys: {:?}",
        runs.keys().collect::<Vec<_>>()
    );
    drop(runs);

    let repos_arc = app_state.repos_by_key_for_test();
    let repos = repos_arc.lock().expect("repos_by_key lock");
    assert!(
        repos.contains_key("id-repo-alpha"),
        "repos_by_key must contain 'id-repo-alpha' after replay; got keys: {:?}",
        repos.keys().collect::<Vec<_>>()
    );
    drop(repos);

    let next_seq_arc = app_state.next_seq_for_test();
    let next_seq = next_seq_arc.lock().expect("next_seq lock");
    assert_eq!(
        next_seq.len(),
        2,
        "next_seq must track both aggregates (Run + Repo); got {} entries",
        next_seq.len()
    );
    for (agg_id, seq) in next_seq.iter() {
        assert_eq!(
            seq.get(),
            1,
            "aggregate {:?} should be at sequence 1 after a single create; got {}",
            agg_id,
            seq.get()
        );
    }
}

/// At HEAD (pre-fix), `AppState::snapshot_fast_path_init` folds events
/// into `projection_state` only for `ORG_GOVERNANCE_AGGREGATE_ID` (=1).
/// `RepoEvaluated` envelopes are emitted by per-repo aggregates on
/// `AggregateId(2..)` and are never folded — `projection_state.
/// repositories` stays empty even though `bootstrap_replay_state`
/// (renamed from `bootstrap_replay_indices` in mission `cpp-r-b-r-c`)
/// successfully walked the same envelopes (and populated `repos_by_key`).
///
/// Test shape: seed one `SweepStarted` on a Run aggregate (gets
/// `AggregateId(1)` — same id the projection-fold currently processes,
/// but Run events do not populate `repositories`) and one
/// `RepoEvaluated` (with non-`None` `evidence`) on a Repo aggregate
/// (gets `AggregateId(2)` — the id the projection-fold currently
/// skips). Drive `snapshot_fast_path_init`, then read
/// `projection_state` and expect the repo entry present. Pre-fix
/// this fails because the read-model is empty; post-fix it passes
/// because the unified replay folds every aggregate's envelopes
/// into `projection_state`.
///
/// Does NOT cross a process or `with_stores` boundary — the
/// in-memory store substitute would drop state. The bug under test
/// is in the boot-replay LOGIC, not in persistence; the in-process
/// test exercises the relevant code path directly via the same
/// `AppState` handle (seed → init → assert) by reaching into
/// `app_state.event_store` (a `pub` field) to seed the same store
/// the init path will read.
#[tokio::test]
async fn restart_rehydrates_projection_state() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let events_dir = tmp.path().join("events");
    let projections_dir = tmp.path().join("projections");
    std::fs::create_dir_all(&events_dir).expect("mk events dir");
    std::fs::create_dir_all(&projections_dir).expect("mk projections dir");

    let app_state = AppState::with_stores(
        &events_dir,
        projections_dir,
        gh_report::config::runtime::PardosaBackend::Pgno,
    )
        .await
        .expect("with_stores");
    let event_store: &Arc<EventStoreImpl> = app_state
        .event_store
        .as_ref()
        .expect("event_store wired by with_stores");
    let ctx = CorrelationContext::none();

    let run_event = DomainEvent::SweepStarted {
        org: "test-org".into(),
        repo_count: 1,
        batch_id: "batch-rehydrate-001".into(),
        timestamp: "2026-05-19T00:00:00Z".into(),
        snapshot_signature: None,
    };
    event_store
        .create(vec![run_event], ctx.clone())
        .await
        .expect("create Run aggregate");

    let repo_event = DomainEvent::RepoEvaluated {
        domain_key: "owner/repo-rehydrate".into(),
        repo_name: "repo-rehydrate".into(),
        success: true,
        source: "scheduled_batch".into(),
        duration_ms: 42,
        timestamp: "2026-05-19T00:00:01Z".into(),
        evidence: Some(Box::new(minimal_evidence("repo-rehydrate"))),
    };
    event_store
        .create(vec![repo_event], ctx)
        .await
        .expect("create Repo aggregate");

    app_state
        .snapshot_fast_path_init()
        .await
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
