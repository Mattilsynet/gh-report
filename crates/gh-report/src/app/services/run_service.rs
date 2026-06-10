//! `RunService` — `ApplicationService` for the [`Run`] aggregate
//! (CHE-0054:R4).
//!
//! Post-Mission-H (`adr-fmt-cq7vb.11`, CHE-0069): the five (now six)
//! public methods are thin wrappers over
//! [`cherry_pit_merger::MergerHandle::dispatch`]. The triad —
//! load → handle → create-or-append → publish — lives inside the
//! lifted [`cherry_pit_merger::Merger`] consumed via
//! [`super::arms::RunArm`]. Call-site signatures stay byte-identical
//! per CHE-0054:R10.
//!
//! [`Run`]: crate::domain::aggregates::run::Run

use cherry_pit_core::CorrelationContext;
use cherry_pit_merger::MergerHandle;

use super::arms::{RunArm, RunCmd};
use crate::domain::aggregates::run::{
    CompleteSweep, FailSweep, PublishEvidence, RecordProgress, RenderPartial, Run, RunError,
    StartSweep,
};

/// `ApplicationService` for the [`Run`] aggregate.
///
/// Holds a [`cherry_pit_merger::MergerHandle`] clone whose
/// underlying [`cherry_pit_merger::Merger`] task is the sole writer
/// to every `Run` aggregate stream (CHE-0069:R4 single-task
/// front-door). Each public method translates its arguments into the
/// matching [`RunCmd`] variant and awaits the merger's typed
/// [`RunError`] reply.
///
/// [`Run`]: crate::domain::aggregates::run::Run
#[derive(Debug)]
pub struct RunService {
    handle: MergerHandle<Run, RunArm>,
}

impl RunService {
    /// Construct a `RunService` wired to the shared
    /// [`cherry_pit_merger::MergerHandle`].
    ///
    /// The supplied handle is `.clone()`-able into multiple services
    /// or across tasks; clones share the same underlying merger
    /// `mpsc::Sender` and dispatch into the same single-task front
    /// door (preserving the per-aggregate sole-writer invariant).
    #[must_use]
    pub fn with_handle(handle: MergerHandle<Run, RunArm>) -> Self {
        Self { handle }
    }

    /// Begin a new sweep run.
    ///
    /// Dispatches [`RunCmd::Start`] through the merger task. The
    /// merger arm runs the lazy create-or-append triad against the
    /// shared [`EventStore`](cherry_pit_core::EventStore) /
    /// [`EventBus`](cherry_pit_core::EventBus) — a same-`batch_id`
    /// retry observes the existing aggregate and surfaces
    /// [`RunError::AlreadyStarted`] from the aggregate's
    /// [`HandleCommand::handle`](cherry_pit_core::HandleCommand::handle)
    /// guard (CHE-0054:R1.a). The sole-writer invariant for the
    /// [`Run`](crate::domain::aggregates::run::Run) aggregate is
    /// enforced by [`cherry_pit_merger`]'s single-task front-door.
    ///
    /// # Errors
    ///
    /// - [`RunError::AlreadyStarted`] when an aggregate for
    ///   `batch_id` is past `Empty`.
    /// - [`RunError::Storage`] on [`StoreError`](cherry_pit_core::StoreError)
    ///   lifted via the arm's `From<StoreError>` impl.
    pub async fn start_sweep(
        &self,
        cmd: StartSweep,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        self.handle.dispatch(RunCmd::Start(cmd), ctx.clone()).await
    }

    /// Record a progress checkpoint mid-sweep.
    ///
    /// `batch_id` is the routing key per CHE-0054:R5.
    ///
    /// # Errors
    ///
    /// - [`RunError::NotStarted`] when the resolved aggregate is not
    ///   in `Started`.
    /// - [`RunError::RoutingMiss`] when `batch_id` has no routing-index
    ///   entry (raised by [`super::arms::RunArm::missing_key_error`]).
    pub async fn record_progress(
        &self,
        batch_id: &str,
        cmd: RecordProgress,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        self.handle
            .dispatch(
                RunCmd::Progress {
                    batch_id: batch_id.to_owned(),
                    cmd,
                },
                ctx.clone(),
            )
            .await
    }

    /// Mark the sweep complete (success terminal).
    ///
    /// # Errors
    ///
    /// - [`RunError::NotStarted`] (CHE-0054:R1.b terminal-xor).
    /// - [`RunError::RoutingMiss`] on routing-index miss.
    pub async fn complete(
        &self,
        batch_id: &str,
        cmd: CompleteSweep,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        self.handle
            .dispatch(
                RunCmd::Complete {
                    batch_id: batch_id.to_owned(),
                    cmd,
                },
                ctx.clone(),
            )
            .await
    }

    /// Mark the sweep failed (failure terminal).
    ///
    /// # Errors
    ///
    /// - [`RunError::NotStarted`] (CHE-0054:R1.b terminal-xor).
    /// - [`RunError::RoutingMiss`] on routing-index miss.
    pub async fn fail(
        &self,
        batch_id: &str,
        cmd: FailSweep,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        self.handle
            .dispatch(
                RunCmd::Fail {
                    batch_id: batch_id.to_owned(),
                    cmd,
                },
                ctx.clone(),
            )
            .await
    }

    /// Publish evidence after a successful sweep.
    ///
    /// # Errors
    ///
    /// - [`RunError::NotCompleted`] (CHE-0054:R1.c).
    /// - [`RunError::RoutingMiss`] on routing-index miss.
    pub async fn publish_evidence(
        &self,
        batch_id: &str,
        cmd: PublishEvidence,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        self.handle
            .dispatch(
                RunCmd::Publish {
                    batch_id: batch_id.to_owned(),
                    cmd,
                },
                ctx.clone(),
            )
            .await
    }

    /// Record a mid-sweep partial render (non-terminal, CHE-0054:R1.e).
    ///
    /// `batch_id` is the routing key.
    ///
    /// # Errors
    ///
    /// - [`RunError::NotStarted`] (`render_partial` requires
    ///   `Started` per CHE-0054:R1.e).
    /// - [`RunError::RoutingMiss`] on routing-index miss.
    pub async fn render_partial(
        &self,
        batch_id: &str,
        cmd: RenderPartial,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        self.handle
            .dispatch(
                RunCmd::Partial {
                    batch_id: batch_id.to_owned(),
                    cmd,
                },
                ctx.clone(),
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::num::NonZeroU64;
    use std::sync::{Arc, Mutex};

    use cherry_pit_app::InProcessEventBus;
    use cherry_pit_core::{AggregateId, EventEnvelope, EventStore};
    use tempfile::TempDir;

    use crate::app::services::merger::MergerHandles;
    use crate::app::state::EventStoreImpl;
    use crate::domain::events::DomainEvent;

    /// Build a Mission-H-shaped [`RunService`] backed by:
    ///
    /// - A tempdir [`MsgpackFileStore`] (Gap-β bead `adr-fmt-luxw`);
    /// - An [`InProcessEventBus`] for fan-out,
    /// - Three [`cherry_pit_merger::Merger`] tasks spawned via
    ///   [`MergerHandles::spawn`] over the same store/bus/indices/tracker
    ///   so the assertions below observe the merger-driven shared
    ///   state exactly as production does.
    ///
    /// Returns the tempdir (kept alive for the test — drop releases
    /// the CHE-0043:R1 flock on `{dir}/.lock`), the durable handles
    /// for direct inspection, and the [`RunService`] under test. The
    /// [`MergerJoinHandles`](super::super::merger::MergerJoinHandles)
    /// bundle is intentionally dropped — each merger task is kept
    /// alive by the [`MergerHandle`] clones inside the services
    /// (refcount-based shutdown when every clone drops).
    #[expect(clippy::unused_async, reason = "preserves .await callers")]
    async fn build_service() -> (
        TempDir,
        Arc<EventStoreImpl>,
        Arc<InProcessEventBus<DomainEvent>>,
        Arc<Mutex<HashMap<String, AggregateId>>>,
        Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
        RunService,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(EventStoreImpl::create_pgno(&dir.path().join("events.pgno")).unwrap());
        let bus = Arc::new(InProcessEventBus::<DomainEvent>::new());
        let runs_by_key = Arc::new(Mutex::new(HashMap::new()));
        let repos_by_key = Arc::new(Mutex::new(HashMap::new()));
        let tracker = Arc::new(Mutex::new(HashMap::new()));
        let (handles, _joins) = MergerHandles::spawn(
            Arc::clone(&store),
            Arc::clone(&bus),
            Arc::clone(&runs_by_key),
            repos_by_key,
            Arc::clone(&tracker),
        );
        let svc = RunService::with_handle(handles.run);
        (dir, store, bus, runs_by_key, tracker, svc)
    }

    #[tokio::test]
    async fn with_handle_constructs_service() {
        let (_dir, _store, _bus, _index, _tracker, _svc) = build_service().await;
    }

    /// Mission H — full Run lifecycle exercising all five service
    /// surfaces routed through the lifted
    /// [`cherry_pit_merger::Merger`]:
    /// `start → progress → progress → complete → publish_evidence`.
    ///
    /// Asserts the same five properties the pre-Mission-H lifecycle
    /// test asserted (envelope sequence + payload variants + bus
    /// capture + sequence tracker advance + single per-aggregate
    /// file) — proving the lift is observably equivalent at the
    /// `EventStore` / `EventBus` boundary.
    ///
    /// Contract enforcer for SMI invariants 1 (sole-writer: the
    /// lifted merger is the only writer to the Run stream) and 5
    /// (post-append publish: every appended envelope arrives on the
    /// bus before the reply resolves).
    #[tokio::test]
    async fn run_lifecycle_appends_persists_and_publishes_through_merger() {
        let (dir, store, bus, index, tracker, svc) = build_service().await;

        let captured: Arc<Mutex<Vec<EventEnvelope<DomainEvent>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let captured_for_handler = Arc::clone(&captured);
        bus.register(move |env: &EventEnvelope<DomainEvent>| {
            captured_for_handler
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(env.clone());
        });

        let ctx = CorrelationContext::none();
        let batch_id = "batch-lifecycle-001";

        svc.start_sweep(
            StartSweep {
                org: "octocat".into(),
                repo_count: 3,
                batch_id: batch_id.into(),
                timestamp: "2026-05-10T12:00:00Z".into(),
                snapshot_signature: "test-sig".into(),
            },
            &ctx,
        )
        .await
        .expect("start_sweep");

        svc.record_progress(
            batch_id,
            RecordProgress {
                batch_id: batch_id.into(),
                completed: 1,
                total: 3,
                timestamp: "2026-05-10T12:00:01Z".into(),
            },
            &ctx,
        )
        .await
        .expect("record_progress 1");

        svc.record_progress(
            batch_id,
            RecordProgress {
                batch_id: batch_id.into(),
                completed: 2,
                total: 3,
                timestamp: "2026-05-10T12:00:02Z".into(),
            },
            &ctx,
        )
        .await
        .expect("record_progress 2");

        svc.complete(
            batch_id,
            CompleteSweep {
                batch_id: batch_id.into(),
                duration_ms: 5000,
                repo_count: 3,
                timestamp: "2026-05-10T12:00:05Z".into(),
            },
            &ctx,
        )
        .await
        .expect("complete");

        svc.publish_evidence(
            batch_id,
            PublishEvidence {
                page_count: 7,
                warm_start: false,
                timestamp: "2026-05-10T12:00:06Z".into(),
            },
            &ctx,
        )
        .await
        .expect("publish_evidence");

        let assigned_id = {
            let guard = index
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard.get(batch_id).expect("index should map batch_id")
        };

        let loaded = store.load(assigned_id).await.expect("load");
        assert_lifecycle_stream(&loaded);

        assert_captured_sequence(&captured, 5);

        assert_tracker_seq(&tracker, assigned_id, 5);

        assert_pardosa_pgno_file(&dir);
    }

    fn assert_lifecycle_stream(loaded: &[EventEnvelope<DomainEvent>]) {
        assert_eq!(loaded.len(), 5, "5 envelopes after lifecycle");
        for (i, env) in loaded.iter().enumerate() {
            assert_eq!(
                env.sequence().get(),
                u64::try_from(i + 1).unwrap(),
                "envelope {i} should have sequence {}",
                i + 1
            );
        }
        assert!(matches!(
            loaded[0].payload(),
            DomainEvent::SweepStarted { .. }
        ));
        assert!(matches!(
            loaded[1].payload(),
            DomainEvent::SweepProgress {
                completed: 1,
                total: 3,
                ..
            }
        ));
        assert!(matches!(
            loaded[2].payload(),
            DomainEvent::SweepProgress {
                completed: 2,
                total: 3,
                ..
            }
        ));
        assert!(matches!(
            loaded[3].payload(),
            DomainEvent::SweepCompleted { repo_count: 3, .. }
        ));
        assert!(matches!(
            loaded[4].payload(),
            DomainEvent::EvidencePublished {
                page_count: 7,
                warm_start: false,
                ..
            }
        ));
    }

    fn assert_captured_sequence(
        captured: &Arc<Mutex<Vec<EventEnvelope<DomainEvent>>>>,
        expected_len: usize,
    ) {
        let envs = captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            envs.len(),
            expected_len,
            "{expected_len} envelopes published"
        );
        for (i, env) in envs.iter().enumerate() {
            assert_eq!(env.sequence().get(), u64::try_from(i + 1).unwrap());
        }
    }

    fn assert_tracker_seq(
        tracker: &Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
        id: AggregateId,
        expected: u64,
    ) {
        let guard = tracker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let seq = *guard.get(&id).expect("next_seq entry");
        assert_eq!(
            seq.get(),
            expected,
            "tracker should reflect last appended sequence"
        );
    }

    fn assert_pardosa_pgno_file(dir: &TempDir) {
        let expected = dir.path().join("events.pgno");
        assert!(
            expected.exists(),
            "expected `{}` to exist under {}",
            expected.display(),
            dir.path().display(),
        );
        assert!(!dir.path().join("1.msgpack").exists());
    }

    /// CHE-0024:R1 — append-path called for an unknown `batch_id`
    /// returns [`RunError::RoutingMiss`] rather than panicking. The
    /// merger arm preserves the error verbatim across the channel.
    #[tokio::test]
    async fn record_progress_on_unknown_batch_id_returns_routing_miss() {
        let (_dir, _store, _bus, _index, _tracker, svc) = build_service().await;
        let ctx = CorrelationContext::none();
        let cmd = RecordProgress {
            batch_id: "never-registered".into(),
            completed: 1,
            total: 3,
            timestamp: "2026-05-10T12:00:00Z".into(),
        };

        let err = svc
            .record_progress("never-registered", cmd, &ctx)
            .await
            .expect_err("unknown batch_id should not panic; must return RoutingMiss");

        assert_eq!(err, RunError::RoutingMiss("never-registered".into()));
    }

    /// Mission H smoke test for create-path: `start_sweep` through
    /// the lifted merger publishes exactly one `SweepStarted`
    /// envelope at sequence 1, populates the routing index, and
    /// records the sequence tracker.
    #[tokio::test]
    async fn start_sweep_create_path_persists_and_publishes_through_merger() {
        let (dir, store, bus, index, tracker, svc) = build_service().await;

        let captured: Arc<Mutex<Vec<EventEnvelope<DomainEvent>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let captured_for_handler = Arc::clone(&captured);
        bus.register(move |env: &EventEnvelope<DomainEvent>| {
            captured_for_handler
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(env.clone());
        });

        let cmd = StartSweep {
            org: "octocat".into(),
            repo_count: 3,
            batch_id: "batch-001".into(),
            timestamp: "2026-05-10T12:00:00Z".into(),
            snapshot_signature: "test-sig".into(),
        };
        let ctx = CorrelationContext::none();

        svc.start_sweep(cmd.clone(), &ctx)
            .await
            .expect("start_sweep should succeed on empty aggregate");

        let assigned_id = {
            let guard = index
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard
                .get(&cmd.batch_id)
                .expect("index should map batch_id to AggregateId")
        };
        let tracked_seq = {
            let guard = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard
                .get(&assigned_id)
                .expect("next_seq should record last applied seq")
        };
        assert_eq!(tracked_seq.get(), 1, "first event has sequence 1");

        let expected = dir.path().join("events.pgno");
        assert!(
            expected.exists(),
            "expected `{}` to exist after first append",
            expected.display(),
        );
        assert!(!dir.path().join("1.msgpack").exists());
        let loaded = store.load(assigned_id).await.expect("load should succeed");
        assert_eq!(loaded.len(), 1, "exactly one envelope persisted");
        assert_eq!(loaded[0].sequence().get(), 1, "first event has sequence 1");
        match loaded[0].payload() {
            DomainEvent::SweepStarted {
                org,
                repo_count,
                batch_id,
                ..
            } => {
                assert_eq!(org, "octocat");
                assert_eq!(*repo_count, 3u64);
                assert_eq!(batch_id, "batch-001");
            }
            other => panic!("expected SweepStarted, got {other:?}"),
        }

        let captured_envs = captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(captured_envs.len(), 1, "exactly one envelope published");
        assert_eq!(captured_envs[0].sequence().get(), 1);
        assert!(matches!(
            captured_envs[0].payload(),
            DomainEvent::SweepStarted { .. }
        ));
    }

    /// CHE-0054:R1.e — `render_partial` is non-terminal and admissible
    /// between `SweepStarted` and a terminal event. Drives the full
    /// 6-envelope sequence through the lifted merger.
    #[expect(
        clippy::too_many_lines,
        reason = "linear 6-step lifecycle assertion; factoring into helpers would obscure the envelope-order claim that is the whole point of the test"
    )]
    #[tokio::test]
    async fn render_partial_between_progress_and_complete() {
        let (_dir, store, _bus, index, tracker, svc) = build_service().await;

        let ctx = CorrelationContext::none();
        let batch_id = "batch-partial-001";

        svc.start_sweep(
            StartSweep {
                org: "octocat".into(),
                repo_count: 4,
                batch_id: batch_id.into(),
                timestamp: "2026-05-17T10:00:00Z".into(),
                snapshot_signature: "test-sig".into(),
            },
            &ctx,
        )
        .await
        .expect("start_sweep");

        svc.record_progress(
            batch_id,
            RecordProgress {
                batch_id: batch_id.into(),
                completed: 1,
                total: 4,
                timestamp: "2026-05-17T10:00:01Z".into(),
            },
            &ctx,
        )
        .await
        .expect("record_progress");

        svc.render_partial(
            batch_id,
            RenderPartial {
                batch_id: batch_id.into(),
                page_count: 2,
                pending_repos: 3,
                timestamp: "2026-05-17T10:00:02Z".into(),
            },
            &ctx,
        )
        .await
        .expect("render_partial 1");

        svc.render_partial(
            batch_id,
            RenderPartial {
                batch_id: batch_id.into(),
                page_count: 5,
                pending_repos: 1,
                timestamp: "2026-05-17T10:00:03Z".into(),
            },
            &ctx,
        )
        .await
        .expect("render_partial 2");

        svc.complete(
            batch_id,
            CompleteSweep {
                batch_id: batch_id.into(),
                duration_ms: 4000,
                repo_count: 4,
                timestamp: "2026-05-17T10:00:04Z".into(),
            },
            &ctx,
        )
        .await
        .expect("complete");

        svc.publish_evidence(
            batch_id,
            PublishEvidence {
                page_count: 7,
                warm_start: false,
                timestamp: "2026-05-17T10:00:05Z".into(),
            },
            &ctx,
        )
        .await
        .expect("publish_evidence");

        let assigned_id = {
            let guard = index
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard.get(batch_id).expect("index should map batch_id")
        };

        let loaded = store.load(assigned_id).await.expect("load");
        assert_eq!(loaded.len(), 6, "6 envelopes after lifecycle with partials");
        for (i, env) in loaded.iter().enumerate() {
            assert_eq!(
                env.sequence().get(),
                u64::try_from(i + 1).unwrap(),
                "envelope {i} should have sequence {}",
                i + 1
            );
        }
        assert!(matches!(
            loaded[0].payload(),
            DomainEvent::SweepStarted { .. }
        ));
        assert!(matches!(
            loaded[1].payload(),
            DomainEvent::SweepProgress {
                completed: 1,
                total: 4,
                ..
            }
        ));
        assert!(matches!(
            loaded[2].payload(),
            DomainEvent::PartialEvidenceRendered {
                page_count: 2,
                pending_repos: 3,
                ..
            }
        ));
        assert!(matches!(
            loaded[3].payload(),
            DomainEvent::PartialEvidenceRendered {
                page_count: 5,
                pending_repos: 1,
                ..
            }
        ));
        assert!(matches!(
            loaded[4].payload(),
            DomainEvent::SweepCompleted { repo_count: 4, .. }
        ));
        assert!(matches!(
            loaded[5].payload(),
            DomainEvent::EvidencePublished {
                page_count: 7,
                warm_start: false,
                ..
            }
        ));

        let guard = tracker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let seq = *guard.get(&assigned_id).expect("tracker entry");
        assert_eq!(
            seq.get(),
            6,
            "tracker should reflect last appended sequence"
        );
    }
}
