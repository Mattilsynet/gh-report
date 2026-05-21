//! `RunService` — `ApplicationService` for the [`Run`] aggregate
//! (CHE-0054:R4), rerouted through the [`Merger`] task at Track 4.0/3b.
//!
//! The five public methods preserve their pre-3b signatures verbatim
//! — call sites at `collect.rs` / `server.rs` did not move — but the
//! bodies are now thin wrappers that build a [`MergerCommand`] +
//! [`oneshot::channel`] reply, send through the Merger's
//! [`mpsc::Sender`], and await the reply. The load → handle → append
//! → publish triad lives inside the Merger arms (verbatim lifts of
//! the pre-3b service bodies — see [`super::merger`] module docs);
//! `RunService` no longer holds the [`EventStore`] or [`EventBus`]
//! handle, the routing index, or the sequence tracker.
//!
//! ## Generic-port discipline (Track 4.0/3b departure)
//!
//! Pre-3b the service was generic over `S: EventStore<Event =
//! DomainEvent>` + `B: EventBus<Event = DomainEvent>` per CHE-0005:R1.
//! Post-3b the service holds only a [`mpsc::Sender<MergerCommand>`]
//! and no longer touches either port directly, so the generics are
//! dropped. The [`Merger`] binds the concrete types at the
//! composition root — see [`super::merger`] module docs.
//!
//! [`Run`]: crate::domain::aggregates::run::Run
//! [`Merger`]: super::merger::Merger
//! [`MergerCommand`]: super::merger::MergerCommand
//! [`EventStore`]: cherry_pit_core::EventStore
//! [`EventBus`]: cherry_pit_core::EventBus
//! [`mpsc::Sender`]: tokio::sync::mpsc::Sender
//! [`oneshot::channel`]: tokio::sync::oneshot::channel

use cherry_pit_core::CorrelationContext;
use tokio::sync::{mpsc, oneshot};

use super::merger::MergerCommand;
use crate::domain::aggregates::run::{
    CompleteSweep, FailSweep, PublishEvidence, RecordProgress, RenderPartial, RunError, StartSweep,
};

/// `ApplicationService` for the [`Run`] aggregate.
///
/// Post-3b channel handle: a thin wrapper over the [`Merger`] task's
/// [`mpsc::Sender`]. Each method builds the corresponding
/// [`MergerCommand`] variant with a [`oneshot::Sender`] reply, sends
/// it through the merger queue, and awaits the typed
/// `Result<(), RunError>` response.
///
/// ## SMI invariant carry (Track 4.0/3b)
///
/// Routing the five `RunService` write paths through `merger_tx`
/// promotes the *sole-writer* invariant from latent to enforced for
/// the [`Run`] aggregate: every successful append to a `Run` stream
/// now flows through the single Merger task. `RepoService` /
/// `WebhookService` reroute at steps 4 / 5 close the analogous gap for
/// their aggregates; the final cross-aggregate sole-writer guarantee
/// is end-of-Track-4.0.
///
/// [`Run`]: crate::domain::aggregates::run::Run
/// [`Merger`]: super::merger::Merger
/// [`mpsc::Sender`]: tokio::sync::mpsc::Sender
#[derive(Debug)]
pub struct RunService {
    /// Producer end of the Merger command channel
    /// (`adr-fmt-nnn3` — Track 4.0/3b).
    merger_tx: mpsc::Sender<MergerCommand>,
}

impl RunService {
    /// Construct a `RunService` wired to the [`Merger`] command
    /// channel.
    ///
    /// The supplied `merger_tx` is shared with [`AppState`] and the
    /// other `ApplicationService` surfaces that will reroute in Track
    /// 4.0/4 ([`RepoService`]) and Track 4.0/5
    /// ([`WebhookService`]). Cloning the [`mpsc::Sender`] is cheap
    /// (refcount bump) and keeps the channel open for the process
    /// lifetime of [`AppState`].
    ///
    /// [`AppState`]: crate::app::state::AppState
    /// [`Merger`]: super::merger::Merger
    /// [`RepoService`]: super::repo_service::RepoService
    /// [`WebhookService`]: super::webhook_service::WebhookService
    /// [`mpsc::Sender`]: tokio::sync::mpsc::Sender
    #[must_use]
    pub fn with_merger_tx(merger_tx: mpsc::Sender<MergerCommand>) -> Self {
        Self { merger_tx }
    }

    /// Begin a new sweep run.
    ///
    /// Routes [`MergerCommand::StartSweep`] through the Merger task,
    /// which runs the create-path triad
    /// (load → handle → create → publish) verbatim against the
    /// shared [`EventStore`](cherry_pit_core::EventStore) /
    /// [`EventBus`](cherry_pit_core::EventBus) the Merger owns. The
    /// SMI sole-writer invariant for the [`Run`] aggregate is
    /// enforced by the Merger task being the only holder of the
    /// store write handle for Run streams from 3b onward.
    ///
    /// # Errors
    ///
    /// - [`RunError::AlreadyStarted`] when an existing aggregate for
    ///   the same `batch_id` is already past `Empty`.
    /// - Persistence failures surface as `RunError` only after
    ///   future enrichment (`#[non_exhaustive]` on `RunError` per
    ///   linus L1); for now an `EventStore` error panics inside the
    ///   Merger arm.
    ///
    /// # Panics
    ///
    /// Panics if the Merger task has shut down before the reply
    /// arrives — this can only happen at process teardown after
    /// [`AppState`] has been dropped, so a panic on the receiver
    /// surfaces a misuse-after-shutdown bug rather than a recoverable
    /// runtime condition.
    ///
    /// [`AppState`]: crate::app::state::AppState
    /// [`Run`]: crate::domain::aggregates::run::Run
    pub async fn start_sweep(
        &self,
        cmd: StartSweep,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        let (reply, rx) = oneshot::channel();
        self.merger_tx
            .send(MergerCommand::StartSweep {
                cmd,
                ctx: ctx.clone(),
                reply,
            })
            .await
            .expect("Merger task alive for AppState lifetime");
        rx.await.expect("Merger arm always replies before drop")
    }

    /// Record a progress checkpoint mid-sweep.
    ///
    /// `batch_id` is the routing key per CHE-0054:R5 — see the
    /// pre-3b doc for the rationale.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::NotStarted`] when the resolved aggregate
    /// is not in the `Started` phase.
    /// Returns [`RunError::RoutingMiss`] when `batch_id` has no entry
    /// in the routing index (CHE-0024:R1 non-fatal path).
    ///
    /// # Panics
    ///
    /// See [`Self::start_sweep`].
    pub async fn record_progress(
        &self,
        batch_id: &str,
        cmd: RecordProgress,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        let (reply, rx) = oneshot::channel();
        self.merger_tx
            .send(MergerCommand::RecordProgress {
                batch_id: batch_id.to_owned(),
                cmd,
                ctx: ctx.clone(),
                reply,
            })
            .await
            .expect("Merger task alive for AppState lifetime");
        rx.await.expect("Merger arm always replies before drop")
    }

    /// Mark the sweep complete (success terminal).
    ///
    /// # Errors
    ///
    /// Returns [`RunError::NotStarted`] when the resolved aggregate
    /// is not in the `Started` phase (terminal-xor invariant b).
    /// Returns [`RunError::RoutingMiss`] when `batch_id` has no entry
    /// in the routing index.
    ///
    /// # Panics
    ///
    /// See [`Self::start_sweep`].
    pub async fn complete(
        &self,
        batch_id: &str,
        cmd: CompleteSweep,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        let (reply, rx) = oneshot::channel();
        self.merger_tx
            .send(MergerCommand::CompleteSweep {
                batch_id: batch_id.to_owned(),
                cmd,
                ctx: ctx.clone(),
                reply,
            })
            .await
            .expect("Merger task alive for AppState lifetime");
        rx.await.expect("Merger arm always replies before drop")
    }

    /// Mark the sweep failed (failure terminal).
    ///
    /// # Errors
    ///
    /// Returns [`RunError::NotStarted`] when the resolved aggregate
    /// is not in the `Started` phase (terminal-xor invariant b).
    /// Returns [`RunError::RoutingMiss`] when `batch_id` has no entry
    /// in the routing index.
    ///
    /// # Panics
    ///
    /// See [`Self::start_sweep`].
    pub async fn fail(
        &self,
        batch_id: &str,
        cmd: FailSweep,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        let (reply, rx) = oneshot::channel();
        self.merger_tx
            .send(MergerCommand::FailSweep {
                batch_id: batch_id.to_owned(),
                cmd,
                ctx: ctx.clone(),
                reply,
            })
            .await
            .expect("Merger task alive for AppState lifetime");
        rx.await.expect("Merger arm always replies before drop")
    }

    /// Publish evidence after a successful sweep.
    ///
    /// `batch_id` is the routing key. [`PublishEvidence`] does not
    /// carry `batch_id` in its payload — the service supplies the
    /// routing key explicitly per CHE-0054:R5.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::NotCompleted`] when the resolved aggregate
    /// is not in the `Completed` phase (invariant c).
    /// Returns [`RunError::RoutingMiss`] when `batch_id` has no entry
    /// in the routing index.
    ///
    /// # Panics
    ///
    /// See [`Self::start_sweep`].
    pub async fn publish_evidence(
        &self,
        batch_id: &str,
        cmd: PublishEvidence,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        let (reply, rx) = oneshot::channel();
        self.merger_tx
            .send(MergerCommand::PublishEvidence {
                batch_id: batch_id.to_owned(),
                cmd,
                ctx: ctx.clone(),
                reply,
            })
            .await
            .expect("Merger task alive for AppState lifetime");
        rx.await.expect("Merger arm always replies before drop")
    }

    /// Record a mid-sweep partial render (non-terminal, CHE-0054:R1.e).
    ///
    /// `batch_id` is the routing key. Distinct from
    /// [`Self::publish_evidence`] which is terminal and must follow
    /// `SweepCompleted`. `render_partial` may be called any number of
    /// times while the Run is in the `Started` phase and does not
    /// advance the phase.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::NotStarted`] when the resolved aggregate is
    /// not in the `Started` phase.
    /// Returns [`RunError::RoutingMiss`] when `batch_id` has no entry in
    /// the routing index.
    ///
    /// # Panics
    ///
    /// See [`Self::start_sweep`].
    pub async fn render_partial(
        &self,
        batch_id: &str,
        cmd: RenderPartial,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        let (reply, rx) = oneshot::channel();
        self.merger_tx
            .send(MergerCommand::RenderPartial {
                batch_id: batch_id.to_owned(),
                cmd,
                ctx: ctx.clone(),
                reply,
            })
            .await
            .expect("Merger task alive for AppState lifetime");
        rx.await.expect("Merger arm always replies before drop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::num::NonZeroU64;
    use std::sync::{Arc, Mutex};

    use cherry_pit_agent::InProcessEventBus;
    use cherry_pit_core::{AggregateId, EventEnvelope, EventStore};
    use tempfile::TempDir;

    use crate::app::services::merger::Merger;
    use crate::app::state::EventStoreImpl;
    use crate::domain::events::DomainEvent;
    use cherry_pit_gateway::MsgpackFileStore;

    /// Build a Track 4.0/3b-shaped `RunService` backed by:
    ///
    /// - A tempdir [`MsgpackFileStore`] (Gap-β bead `adr-fmt-luxw`);
    /// - An [`InProcessEventBus`] for fan-out,
    /// - A [`Merger`] task spawned over the same store/bus/indices/tracker
    ///   so the assertions below observe the Merger-driven shared state
    ///   exactly as production will at 3b/4/5.
    ///
    /// Returns the tempdir (kept alive for the test — drop releases the
    /// CHE-0043:R1 flock on `{dir}/.lock`), the durable handles for
    /// direct inspection, and the [`RunService`] under test. The Merger
    /// [`tokio::task::JoinHandle`] is intentionally dropped here —
    /// the task is kept alive by the [`mpsc::Sender`] inside the
    /// service; dropping the handle without aborting lets the task run
    /// for the test scope (the handle does **not** abort on drop, see
    /// `tokio::task::JoinHandle` docs).
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
        let store = Arc::new(MsgpackFileStore::<DomainEvent>::new(dir.path()));
        let bus = Arc::new(InProcessEventBus::<DomainEvent>::new());
        let runs_by_key = Arc::new(Mutex::new(HashMap::new()));
        let repos_by_key = Arc::new(Mutex::new(HashMap::new()));
        let deliveries_by_id = Arc::new(Mutex::new(HashMap::new()));
        let tracker = Arc::new(Mutex::new(HashMap::new()));
        let (merger_tx, _merger_handle) = Merger::spawn(
            Arc::clone(&store),
            Arc::clone(&bus),
            Arc::clone(&runs_by_key),
            repos_by_key,
            deliveries_by_id,
            Arc::clone(&tracker),
        );
        let svc = RunService::with_merger_tx(merger_tx);
        (dir, store, bus, runs_by_key, tracker, svc)
    }

    #[tokio::test]
    async fn with_merger_tx_constructs_service() {
        // Smoke test: 3b constructor surface compiles and yields a
        // service whose handle (the mpsc::Sender) is wired to a live
        // Merger task. Behaviour is covered by the lifecycle test.
        let (_dir, _store, _bus, _index, _tracker, _svc) = build_service().await;
    }

    /// 3b — full Run lifecycle exercising all five service surfaces
    /// routed through the Merger task:
    /// `start → progress → progress → complete → publish_evidence`.
    ///
    /// Asserts the same five properties the pre-3b lifecycle test
    /// asserted (envelope sequence + payload variants + bus capture
    /// + sequence tracker advance + single per-aggregate file) —
    /// proving the channel-reroute is observably equivalent at the
    /// `EventStore` / `EventBus` boundary.
    ///
    /// This is the contract enforcer for SMI invariants 1
    /// (sole-writer: the Merger is the only writer to the Run
    /// stream) and 5 (post-append publish: every appended envelope
    /// arrives on the bus before the reply resolves).
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

        // 1. start
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

        // 2. progress (1/3)
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

        // 3. progress (2/3)
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

        // 4. complete
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

        // 5. publish_evidence
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

        // Resolve the assigned id from the index.
        let assigned_id = {
            let guard = index
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard.get(batch_id).expect("index should map batch_id")
        };

        // (1) Stream contents.
        let loaded = store.load(assigned_id).await.expect("load");
        assert_lifecycle_stream(&loaded);

        // (2) Bus captured all 5 in order.
        assert_captured_sequence(&captured, 5);

        // (3) Sequence tracker == 5.
        assert_tracker_seq(&tracker, assigned_id, 5);

        // (4) Single per-aggregate file (CHE-0036:R1).
        assert_single_msgpack_file(&dir, assigned_id);
    }

    /// Assert the stored envelope sequence for the run lifecycle test:
    /// 5 envelopes [`SweepStarted`, `SweepProgress(1/3)`, `SweepProgress(2/3)`,
    /// `SweepCompleted{3}`, `EvidencePublished{7, !warm}`], monotonic seqs.
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

    /// Assert the bus captured exactly `expected_len` envelopes in
    /// strict `1..=expected_len` sequence order.
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

    /// Assert the per-aggregate next-sequence tracker entry equals `expected`.
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

    /// Assert that the singleton aggregate's msgpack file exists at
    /// `<dir>/<id>.msgpack` under [`MsgpackFileStore`].
    fn assert_single_msgpack_file(dir: &TempDir, id: AggregateId) {
        let expected = dir.path().join(format!("{}.msgpack", id.get()));
        assert!(
            expected.exists(),
            "expected `{}` to exist under {}",
            expected.display(),
            dir.path().display(),
        );
    }

    /// CHE-0024:R1 — append-path called for an unknown `batch_id`
    /// returns `RunError::RoutingMiss` rather than panicking. The
    /// Merger arm preserves the error verbatim across the channel.
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

    /// 3b smoke test for create-path: assert that `start_sweep`
    /// through the Merger publishes exactly one `SweepStarted`
    /// envelope at sequence 1, populates the routing index, and
    /// records the sequence tracker. Mirrors the pre-3b
    /// `start_sweep_create_path_persists_and_publishes` test.
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

        // Under MsgpackFileStore each aggregate's events land in
        // `<dir>/<id>.msgpack`.
        let expected = dir.path().join(format!("{}.msgpack", assigned_id.get()));
        assert!(
            expected.exists(),
            "expected `{}` to exist after first append",
            expected.display(),
        );
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
    /// 6-envelope sequence
    /// `start → progress → render_partial → render_partial → complete
    /// → publish_evidence` through the Merger and asserts envelope
    /// order on the [`EventStore`]. The aggregate's phase-admissibility
    /// (Started-only for partial, Completed-only for publish) is the
    /// load-bearing invariant exercised here.
    #[expect(
        clippy::too_many_lines,
        reason = "linear 6-step lifecycle assertion; factoring into helpers would obscure the envelope-order claim that is the whole point of the test"
    )]
    #[tokio::test]
    async fn render_partial_between_progress_and_complete() {
        let (_dir, store, _bus, index, tracker, svc) = build_service().await;

        let ctx = CorrelationContext::none();
        let batch_id = "batch-partial-001";

        // 1. start
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

        // 2. progress (1/4)
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

        // 3. render_partial (first)
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

        // 4. render_partial (second)
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

        // 5. complete
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

        // 6. publish_evidence
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

        // Tracker reflects the final appended sequence.
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
