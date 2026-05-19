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
//! `PardosaFileEventStore::list_aggregates()` and folding each
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
use cherry_pit_pardosa::PardosaFileEventStore;
use gh_report::app::state::AppState;
use gh_report::domain::events::DomainEvent;

#[tokio::test]
async fn bootstrap_replay_populates_routing_indices() {
    // ── Arrange: seed a PardosaFileEventStore with one Run and one
    // Repo aggregate, then drop the store handle to release the
    // CHE-0043:R1 flock.
    let tmp = tempfile::tempdir().expect("tempdir");
    let events_dir = tmp.path().join("events");
    let projections_dir = tmp.path().join("projections");
    std::fs::create_dir_all(&events_dir).expect("mk events dir");
    std::fs::create_dir_all(&projections_dir).expect("mk projections dir");

    {
        let store = Arc::new(
            PardosaFileEventStore::<DomainEvent>::open(&events_dir)
                .expect("open store for seeding"),
        );
        let ctx = CorrelationContext::none();

        // Run aggregate: SweepStarted with batch_id "batch-replay-001".
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

        // Repo aggregate: RepoEvaluated with domain_key "id-repo-alpha".
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
    // store dropped — flock released.

    // ── Act: construct AppState over the seeded events dir and run
    // the bootstrap path.
    let app_state = AppState::with_stores(&events_dir, projections_dir);
    app_state
        .snapshot_fast_path_init()
        .await
        .expect("snapshot_fast_path_init");

    // ── Assert: routing indices populated from replay.
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
    // Every aggregate's first event has sequence 1.
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
