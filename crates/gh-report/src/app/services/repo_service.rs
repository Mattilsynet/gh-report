//! `RepoService` — `ApplicationService` for the [`Repo`] aggregate
//! (CHE-0054:R4), rerouted through the [`Merger`] task at Track 4.0/4.
//!
//! Symmetric to [`super::run_service::RunService`] at Track 4.0/3b:
//! the two public methods preserve their pre-step-4 signatures
//! verbatim — call sites at `daemon.rs` / `webhook/mod.rs` did not
//! move — but the bodies are now thin wrappers that build a
//! [`MergerCommand`] + [`oneshot::channel`] reply, send through the
//! Merger's [`mpsc::Sender`], and await the reply. The load → handle
//! → append → publish triad lives inside the Merger arms (verbatim
//! lifts of the pre-3a service bodies — see [`super::merger`] module
//! docs); `RepoService` no longer holds the [`EventStore`] or
//! [`EventBus`] handle, the routing index, or the sequence tracker.
//!
//! ## Generic-port discipline (Track 4.0/4 departure)
//!
//! Pre-step-4 the service was generic over `S: EventStore<Event =
//! DomainEvent>` + `B: EventBus<Event = DomainEvent>` per CHE-0005:R1.
//! Post-step-4 the service holds only a [`mpsc::Sender<MergerCommand>`]
//! and no longer touches either port directly, so the generics are
//! dropped (Option A — symmetric to `RunService` at 3b). The [`Merger`]
//! binds the concrete types at the composition root — see
//! [`super::merger`] module docs.
//!
//! [`Repo`]: crate::domain::aggregates::repo::Repo
//! [`Merger`]: super::merger::Merger
//! [`MergerCommand`]: super::merger::MergerCommand
//! [`EventStore`]: cherry_pit_core::EventStore
//! [`EventBus`]: cherry_pit_core::EventBus
//! [`mpsc::Sender`]: tokio::sync::mpsc::Sender
//! [`oneshot::channel`]: tokio::sync::oneshot::channel

use cherry_pit_core::CorrelationContext;
use tokio::sync::{mpsc, oneshot};

use super::merger::MergerCommand;
use crate::domain::aggregates::repo::{RecordEvaluation, RecordRemoval, RepoError};

/// `ApplicationService` for the [`Repo`] aggregate.
///
/// Post-step-4 channel handle: a thin wrapper over the [`Merger`]
/// task's [`mpsc::Sender`]. Each method builds the corresponding
/// [`MergerCommand`] variant with a [`oneshot::Sender`] reply, sends
/// it through the merger queue, and awaits the typed
/// `Result<(), RepoError>` response.
///
/// ## SMI invariant carry (Track 4.0/4)
///
/// Routing the two `RepoService` write paths through `merger_tx`
/// promotes the *sole-writer* invariant from latent to enforced for
/// the [`Repo`] aggregate: every successful append to a `Repo` stream
/// now flows through the single Merger task. `RunService` closed the
/// analogous gap for the [`Run`] aggregate at Track 4.0/3b;
/// `WebhookService` reroute at step 5 closes it for the
/// [`WebhookDelivery`] aggregate. The final cross-aggregate
/// sole-writer guarantee is end-of-Track-4.0.
///
/// [`Run`]: crate::domain::aggregates::run::Run
/// [`Repo`]: crate::domain::aggregates::repo::Repo
/// [`WebhookDelivery`]: crate::domain::aggregates::webhook::WebhookDelivery
/// [`Merger`]: super::merger::Merger
/// [`mpsc::Sender`]: tokio::sync::mpsc::Sender
#[derive(Debug)]
pub struct RepoService {
    /// Producer end of the Merger command channel
    /// (`adr-fmt-nnn3` — Track 4.0/4).
    merger_tx: mpsc::Sender<MergerCommand>,
}

impl RepoService {
    /// Construct a `RepoService` wired to the [`Merger`] command
    /// channel.
    ///
    /// The supplied `merger_tx` is shared with [`AppState`] and the
    /// other `ApplicationService` surfaces — at step 4 with
    /// [`RunService`] (already rerouted at 3b); at step 5 with
    /// [`WebhookService`]. Cloning the [`mpsc::Sender`] is cheap
    /// (refcount bump) and keeps the channel open for the process
    /// lifetime of [`AppState`].
    ///
    /// [`AppState`]: crate::app::state::AppState
    /// [`Merger`]: super::merger::Merger
    /// [`RunService`]: super::run_service::RunService
    /// [`WebhookService`]: super::webhook_service::WebhookService
    /// [`mpsc::Sender`]: tokio::sync::mpsc::Sender
    #[must_use]
    pub fn with_merger_tx(merger_tx: mpsc::Sender<MergerCommand>) -> Self {
        Self { merger_tx }
    }

    /// Record a repository evaluation.
    ///
    /// `domain_key` is the routing key — the Merger arm uses it to
    /// resolve the `AggregateId` from the index (CHE-0054:R5),
    /// **lazily creating** a fresh aggregate the first time the key
    /// is seen. The command's own `domain_key` field is treated
    /// strictly as event-payload data; routing is the
    /// `ApplicationService`'s responsibility, separate from the
    /// command shape (mirrors the `batch_id` pattern on
    /// [`RunService`](super::run_service::RunService)).
    ///
    /// Routes [`MergerCommand::RecordEvaluation`] through the Merger
    /// task. The SMI sole-writer invariant for the [`Repo`]
    /// aggregate is enforced by the Merger task being the only
    /// holder of the store write handle for Repo streams from step
    /// 4 onward.
    ///
    /// [`Repo`]: crate::domain::aggregates::repo::Repo
    ///
    /// # Errors
    ///
    /// Returns [`RepoError::AlreadyRemoved`] when the resolved
    /// aggregate is in the terminal `Removed` phase (invariant c).
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
    pub async fn record_evaluation(
        &self,
        domain_key: &str,
        cmd: RecordEvaluation,
        ctx: &CorrelationContext,
    ) -> Result<(), RepoError> {
        let (reply, rx) = oneshot::channel();
        self.merger_tx
            .send(MergerCommand::RecordEvaluation {
                domain_key: domain_key.to_owned(),
                cmd: Box::new(cmd),
                ctx: ctx.clone(),
                reply,
            })
            .await
            .expect("Merger task alive for AppState lifetime");
        rx.await.expect("Merger arm always replies before drop")
    }

    /// Record a repository removal (terminal).
    ///
    /// `domain_key` is the routing key — see
    /// [`record_evaluation`](Self::record_evaluation). A
    /// `RecordRemoval` for a never-evaluated repo lazily creates the
    /// aggregate (allowed per CHE-0054:R2 — webhook-driven removal
    /// can arrive without prior local evaluation).
    ///
    /// # Errors
    ///
    /// Returns [`RepoError::AlreadyRemoved`] when the resolved
    /// aggregate is already in the `Removed` phase.
    ///
    /// # Panics
    ///
    /// See [`Self::record_evaluation`].
    pub async fn record_removal(
        &self,
        domain_key: &str,
        cmd: RecordRemoval,
        ctx: &CorrelationContext,
    ) -> Result<(), RepoError> {
        let (reply, rx) = oneshot::channel();
        self.merger_tx
            .send(MergerCommand::RecordRemoval {
                domain_key: domain_key.to_owned(),
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
    use pardosa_eventstore::PardosaLogEventStore;

    /// Build a Track 4.0/4-shaped `RepoService` backed by a [`Merger`]
    /// task spawned over a shared tempdir [`PardosaLogEventStore`] +
    /// [`InProcessEventBus`] + the three routing indices + sequence
    /// tracker. Symmetric to the `RunService` 3b test harness.
    ///
    /// Returns the tempdir (kept alive for the duration of the test —
    /// drop releases the CHE-0043:R1 flock the store holds on
    /// `{dir}/.lock`), the durable handles for direct inspection, and
    /// the [`RepoService`] under test. The Merger
    /// [`tokio::task::JoinHandle`] is intentionally dropped — the task
    /// is kept alive by the [`mpsc::Sender`] inside the service.
    async fn build_service() -> (
        TempDir,
        Arc<EventStoreImpl>,
        Arc<InProcessEventBus<DomainEvent>>,
        Arc<Mutex<HashMap<String, AggregateId>>>,
        Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
        RepoService,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(
            PardosaLogEventStore::<DomainEvent>::open(dir.path())
                .await
                .expect("open test event store"),
        );
        let bus = Arc::new(InProcessEventBus::<DomainEvent>::new());
        let runs_by_key = Arc::new(Mutex::new(HashMap::new()));
        let repos_by_key = Arc::new(Mutex::new(HashMap::new()));
        let deliveries_by_id = Arc::new(Mutex::new(HashMap::new()));
        let tracker = Arc::new(Mutex::new(HashMap::new()));
        let (merger_tx, _merger_handle) = Merger::spawn(
            Arc::clone(&store),
            Arc::clone(&bus),
            runs_by_key,
            Arc::clone(&repos_by_key),
            deliveries_by_id,
            Arc::clone(&tracker),
        );
        let svc = RepoService::with_merger_tx(merger_tx);
        (dir, store, bus, repos_by_key, tracker, svc)
    }

    #[tokio::test]
    async fn with_merger_tx_constructs_service() {
        // Smoke test: step-4 constructor surface compiles and yields a
        // service whose handle (the mpsc::Sender) is wired to a live
        // Merger task. Behaviour is covered by the lifecycle test.
        let (_dir, _store, _bus, _index, _tracker, _svc) = build_service().await;
    }

    /// Step-4 — full Repo lifecycle exercising both service surfaces
    /// routed through the Merger task:
    /// `evaluate (create) → evaluate (append) → remove (append, terminal)`.
    ///
    /// Asserts the same six properties the pre-step-4 lifecycle test
    /// asserted (envelope sequence + payload variants + bus capture +
    /// routing index populated + sequence tracker advance + single
    /// per-aggregate file + post-removal rejection) — proving the
    /// channel-reroute is observably equivalent at the `EventStore` /
    /// `EventBus` boundary.
    ///
    /// This is the contract enforcer for SMI invariants 1
    /// (sole-writer: the Merger is the only writer to the Repo
    /// stream) and 5 (post-append publish: every appended envelope
    /// arrives on the bus before the reply resolves) for the Repo
    /// aggregate.
    #[tokio::test]
    async fn repo_lifecycle_lazy_creates_then_appends_then_terminates_through_merger() {
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
        let domain_key = "octocat/hello";

        // 1. evaluate (create-path on first reference).
        svc.record_evaluation(
            domain_key,
            RecordEvaluation {
                domain_key: domain_key.into(),
                repo_name: "hello".into(),
                success: true,
                source: "scheduled_batch".into(),
                duration_ms: 100,
                timestamp: "2026-05-10T12:00:00Z".into(),
                evidence: None,
            },
            &ctx,
        )
        .await
        .expect("first record_evaluation");

        // 2. evaluate (append-path on second reference).
        svc.record_evaluation(
            domain_key,
            RecordEvaluation {
                domain_key: domain_key.into(),
                repo_name: "hello".into(),
                success: false,
                source: "scheduled_batch".into(),
                duration_ms: 80,
                timestamp: "2026-05-10T12:01:00Z".into(),
                evidence: None,
            },
            &ctx,
        )
        .await
        .expect("second record_evaluation");

        // 3. remove (append-path, terminal).
        svc.record_removal(
            domain_key,
            RecordRemoval {
                domain_key: domain_key.into(),
                repo_name: "hello".into(),
                timestamp: "2026-05-10T12:02:00Z".into(),
            },
            &ctx,
        )
        .await
        .expect("record_removal");

        // Routing index resolves.
        let assigned_id = {
            let guard = index
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard.get(domain_key).expect("index should map domain_key")
        };

        // Stream contents.
        let loaded = store.load(assigned_id).await.expect("load");
        assert_lifecycle_stream(&loaded);

        // Bus captured all 3 in order.
        assert_captured_sequence(&captured, 3);

        // Sequence tracker == 3.
        assert_tracker_seq(&tracker, assigned_id, 3);

        // Single per-aggregate file (CHE-0036:R1).
        assert_single_pardosa_file(&dir, assigned_id);

        // Post-removal rejection (CHE-0054:R2.c).
        let err = svc
            .record_evaluation(
                domain_key,
                RecordEvaluation {
                    domain_key: domain_key.into(),
                    repo_name: "hello".into(),
                    success: true,
                    source: "scheduled_batch".into(),
                    duration_ms: 1,
                    timestamp: "2026-05-10T12:03:00Z".into(),
                    evidence: None,
                },
                &ctx,
            )
            .await
            .expect_err("evaluate after remove must reject");
        assert_eq!(err, RepoError::AlreadyRemoved);
    }

    /// Assert the stored envelope sequence for the lifecycle test:
    /// 3 envelopes, monotonically-numbered, payloads
    /// [`RepoEvaluated{success:true}`, `RepoEvaluated{success:false}`, `RepoRemoved`].
    fn assert_lifecycle_stream(loaded: &[EventEnvelope<DomainEvent>]) {
        assert_eq!(loaded.len(), 3, "3 envelopes after lifecycle");
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
            DomainEvent::RepoEvaluated { success: true, .. }
        ));
        assert!(matches!(
            loaded[1].payload(),
            DomainEvent::RepoEvaluated { success: false, .. }
        ));
        assert!(matches!(
            loaded[2].payload(),
            DomainEvent::RepoRemoved { .. }
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
        assert_eq!(envs.len(), expected_len);
        for (i, env) in envs.iter().enumerate() {
            assert_eq!(env.sequence().get(), u64::try_from(i + 1).unwrap());
        }
    }

    /// Assert the per-aggregate next-sequence tracker entry equals
    /// `expected`.
    fn assert_tracker_seq(
        tracker: &Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
        id: AggregateId,
        expected: u64,
    ) {
        let guard = tracker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let seq = *guard.get(&id).expect("tracker entry");
        assert_eq!(seq.get(), expected);
    }

    /// Assert that the aggregate's on-disk artefact exists exactly once
    /// at `<dir>/<id>.log` under [`PardosaLogEventStore`]
    /// (CHE-0036:R1 — file-per-aggregate).
    fn assert_single_pardosa_file(dir: &TempDir, id: AggregateId) {
        let expected = dir.path().join(format!("{}.log", id.get()));
        assert!(
            expected.exists(),
            "expected `{}` to exist under {}",
            expected.display(),
            dir.path().display(),
        );
    }

    /// Step-4 — covers the lazy-create branch on `record_removal`:
    /// webhook-driven removal arrives for a never-evaluated repo
    /// (allowed per CHE-0054:R2 — no pre-evaluation precondition).
    #[tokio::test]
    async fn repo_removal_lazy_creates_when_never_evaluated_through_merger() {
        let (_dir, store, _bus, index, tracker, svc) = build_service().await;

        let ctx = CorrelationContext::none();
        let domain_key = "ghost/never-seen";

        svc.record_removal(
            domain_key,
            RecordRemoval {
                domain_key: domain_key.into(),
                repo_name: "never-seen".into(),
                timestamp: "2026-05-10T12:00:00Z".into(),
            },
            &ctx,
        )
        .await
        .expect("lazy create-on-removal");

        let assigned_id = {
            let guard = index
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard
                .get(domain_key)
                .expect("index entry created lazily on removal")
        };
        let loaded = store.load(assigned_id).await.expect("load");
        assert_eq!(loaded.len(), 1);
        assert!(matches!(
            loaded[0].payload(),
            DomainEvent::RepoRemoved { .. }
        ));
        let tracked_seq = {
            let guard = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard.get(&assigned_id).expect("tracker entry")
        };
        assert_eq!(tracked_seq.get(), 1);
    }
}
