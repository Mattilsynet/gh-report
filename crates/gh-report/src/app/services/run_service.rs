//! `RunService` — ApplicationService for the [`Run`] aggregate
//! (CHE-0054:R4).
//!
//! Owns the load → handle → append → publish triad
//! (CHE-0008:R1 + CHE-0024:R3) for sweep-lifecycle use cases. Each
//! method resolves the run's `AggregateId` via the
//! `Arc<Mutex<HashMap<String, AggregateId>>>` index handle held on
//! [`RunService`] (CHE-0054:R5), loads the aggregate from the
//! `EventStore`, dispatches the command via the appropriate
//! [`HandleCommand`] impl, persists the resulting events with CAS on
//! the expected sequence (CHE-0042:R3, CHE-0054:R6), and publishes
//! them via the `EventBus`.
//!
//! ## Method body status
//!
//! All five `RunService` methods are wired (Inc B7'b-2 / B7'b-3).
//! The 14 production publish sites in `collect.rs`/`daemon.rs`/
//! `webhook/mod.rs` migrate to these calls in **B7'c**.
//!
//! ## Generic-port discipline
//!
//! `RunService` is generic over `S: EventStore<Event = DomainEvent>`
//! and `B: EventBus<Event = DomainEvent>` per CHE-0005:R1 (no
//! `Arc<dyn EventStore>` / `Box<dyn EventBus>`). Concrete
//! monomorphisation happens in [`AppState`](crate::app::state::AppState).
//!
//! [`Run`]: crate::domain::aggregates::run::Run
//! [`HandleCommand`]: cherry_pit_core::HandleCommand

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_core::{AggregateId, CorrelationContext, EventBus, EventStore};

use crate::domain::aggregates::run::{
    CompleteSweep, FailSweep, PublishEvidence, RecordProgress, RunError, StartSweep,
};
use crate::domain::events::DomainEvent;

/// ApplicationService for the [`Run`] aggregate.
///
/// Generic over the concrete [`EventStore`] and [`EventBus`]
/// implementations per CHE-0005:R1. The application composition
/// root in [`AppState`](crate::app::state::AppState) supplies the
/// concrete types ([`MsgpackFileStore`](cherry_pit_gateway::MsgpackFileStore)
/// + [`InProcessEventBus`](cherry_pit_agent::InProcessEventBus)).
///
/// ## Routing index (CHE-0054:R5)
///
/// `index` maps a domain key (Run uses `batch_id: String`) to the
/// store-assigned [`AggregateId`]. Routing is the
/// ApplicationService's responsibility — no helper derives the id
/// by destructuring an event variant (B7' Inc 5 abort root cause F1
/// fix).
///
/// ## Sequence tracker (CHE-0054:R6 / CHE-0042:R3)
///
/// `sequence_tracker` records the last applied sequence per
/// aggregate so the `append`-path can pass `expected_sequence:
/// NonZeroU64` for caller-tracked optimistic concurrency control.
/// Empty entries imply create-path.
///
/// [`Run`]: crate::domain::aggregates::run::Run
#[derive(Debug)]
pub struct RunService<S, B>
where
    S: EventStore<Event = DomainEvent>,
    B: EventBus<Event = DomainEvent>,
{
    /// Durable per-aggregate event store (load + create + append).
    store: Arc<S>,
    /// Synchronous in-process event bus (publish for fan-out).
    bus: Arc<B>,
    /// `batch_id` → `AggregateId` routing index (CHE-0054:R5).
    index: Arc<Mutex<HashMap<String, AggregateId>>>,
    /// Last-applied sequence per aggregate for caller-tracked CAS
    /// (CHE-0054:R6, CHE-0042:R3).
    sequence_tracker: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
}

impl<S, B> RunService<S, B>
where
    S: EventStore<Event = DomainEvent>,
    B: EventBus<Event = DomainEvent>,
{
    /// Construct a `RunService` wired to the given store, bus, and
    /// shared routing/sequence handles.
    ///
    /// Both `index` and `sequence_tracker` are typically shared with
    /// the [`AppState`](crate::app::state::AppState) so other
    /// service-method invocations and replay-time fast-path
    /// reconstruction observe the same view of routing.
    #[must_use]
    pub fn with_stores(
        store: Arc<S>,
        bus: Arc<B>,
        index: Arc<Mutex<HashMap<String, AggregateId>>>,
        sequence_tracker: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
    ) -> Self {
        Self {
            store,
            bus,
            index,
            sequence_tracker,
        }
    }

    /// Read access to the store handle (for diagnostics / tests).
    #[must_use]
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }

    /// Read access to the bus handle (for diagnostics / tests).
    #[must_use]
    pub fn bus(&self) -> &Arc<B> {
        &self.bus
    }

    /// Begin a new sweep run.
    ///
    /// Create-path triad (CHE-0054:R10, CHE-0024:R3): handle the
    /// command on a fresh `Run::default()`, persist the resulting
    /// events via `EventStore::create` (the store assigns the
    /// `AggregateId`), record the `batch_id → AggregateId` routing
    /// (CHE-0054:R5) and the per-aggregate sequence (CHE-0054:R6 /
    /// CHE-0042:R3), then publish the envelopes synchronously via
    /// the in-process bus.
    ///
    /// `EventBus::publish` failure is **non-fatal** per CHE-0024:R1
    /// ("publication failure is non-fatal — tracking-style processors
    /// can catch up on missed publications"). Persistence is the
    /// source of truth; a publish error is logged at `warn!` and
    /// swallowed.
    ///
    /// # Errors
    ///
    /// - [`RunError::AlreadyStarted`] when an existing aggregate for
    ///   the same `batch_id` is already past `Empty`.
    /// - Persistence failures surface as `RunError` only after
    ///   future enrichment (`#[non_exhaustive]` on `RunError` per
    ///   linus L1); for now an `EventStore` error panics via
    ///   `expect`. **B7'b-2 scope**: the create-path always starts
    ///   from a fresh `Run::default()` and the only `RunError` path
    ///   is `AlreadyStarted` raised when the `batch_id` is already
    ///   indexed (load returns prior events → fold to non-`Empty`
    ///   phase → `Run::handle` rejects).
    pub async fn start_sweep(
        &self,
        cmd: StartSweep,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        use cherry_pit_core::{Aggregate, HandleCommand};

        let domain_key = cmd.batch_id.clone();

        // 0. Resolve AggregateId (CHE-0054:R5). Create-path starts
        //    fresh when the batch_id is not yet indexed.
        let existing_id = super::shared::lookup(&self.index, &domain_key);

        // 1+2. Load + fold to current state (Empty for create-path).
        let (envelopes, last_seq) =
            super::shared::load_envelopes_or_empty(&self.store, existing_id).await;
        let mut state = crate::domain::aggregates::run::Run::default();
        for env in &envelopes {
            state.apply(env.payload());
        }

        // 3. Handle (pure). May reject with RunError::AlreadyStarted
        //    when an existing aggregate for this batch_id is past Empty.
        let new_events = state.handle(cmd)?;

        // 4. Persist via create or append. The append-path branch is
        //    unreachable here in practice — `Run::handle` above
        //    rejects StartSweep on a non-Empty Run with
        //    RunError::AlreadyStarted — but the shared helper covers
        //    both branches uniformly.
        let new_envelopes = super::shared::create_or_append(
            super::shared::PersistHandles {
                store: &self.store,
                index: &self.index,
                sequence_tracker: &self.sequence_tracker,
            },
            &domain_key,
            existing_id,
            last_seq,
            new_events,
            ctx,
        )
        .await;

        // 5. Publish — non-fatal per CHE-0024:R1.
        super::shared::publish_or_trace(&self.bus, &new_envelopes, "SweepStarted").await;

        Ok(())
    }

    /// Record a progress checkpoint mid-sweep.
    ///
    /// `batch_id` is the routing key — the service uses it to resolve
    /// the `AggregateId` from the index (CHE-0054:R5). The command's
    /// own `batch_id` field is treated strictly as event-payload data;
    /// routing is the ApplicationService's responsibility, separate
    /// from the command shape.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::NotStarted`] when the resolved aggregate
    /// is not in the `Started` phase.
    /// Returns [`RunError::RoutingMiss`] when `batch_id` has no entry
    /// in the routing index (CHE-0024:R1 non-fatal path).
    pub async fn record_progress(
        &self,
        batch_id: &str,
        cmd: RecordProgress,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        use cherry_pit_core::HandleCommand;

        let id = self.resolve_id(batch_id)?;
        let (state, last_seq) = self.load_and_fold(id).await;
        let new_events = state.handle(cmd)?;
        let new_envelopes = self.append_and_track(id, last_seq, new_events, ctx).await;
        super::shared::publish_or_trace(&self.bus, &new_envelopes, "SweepProgress").await;
        Ok(())
    }

    /// Mark the sweep complete (success terminal).
    ///
    /// `batch_id` is the routing key — see [`record_progress`](Self::record_progress).
    ///
    /// # Errors
    ///
    /// Returns [`RunError::NotStarted`] when the resolved aggregate
    /// is not in the `Started` phase (terminal-xor invariant b).
    /// Returns [`RunError::RoutingMiss`] when `batch_id` has no entry
    /// in the routing index (CHE-0024:R1 non-fatal path).
    pub async fn complete(
        &self,
        batch_id: &str,
        cmd: CompleteSweep,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        use cherry_pit_core::HandleCommand;

        let id = self.resolve_id(batch_id)?;
        let (state, last_seq) = self.load_and_fold(id).await;
        let new_events = state.handle(cmd)?;
        let new_envelopes = self.append_and_track(id, last_seq, new_events, ctx).await;
        super::shared::publish_or_trace(&self.bus, &new_envelopes, "SweepCompleted").await;
        Ok(())
    }

    /// Mark the sweep failed (failure terminal).
    ///
    /// `batch_id` is the routing key — see [`record_progress`](Self::record_progress).
    ///
    /// # Errors
    ///
    /// Returns [`RunError::NotStarted`] when the resolved aggregate
    /// is not in the `Started` phase (terminal-xor invariant b).
    /// Returns [`RunError::RoutingMiss`] when `batch_id` has no entry
    /// in the routing index (CHE-0024:R1 non-fatal path).
    pub async fn fail(
        &self,
        batch_id: &str,
        cmd: FailSweep,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        use cherry_pit_core::HandleCommand;

        let id = self.resolve_id(batch_id)?;
        let (state, last_seq) = self.load_and_fold(id).await;
        let new_events = state.handle(cmd)?;
        let new_envelopes = self.append_and_track(id, last_seq, new_events, ctx).await;
        super::shared::publish_or_trace(&self.bus, &new_envelopes, "SweepFailed").await;
        Ok(())
    }

    /// Publish evidence after a successful sweep.
    ///
    /// `batch_id` is the routing key. [`PublishEvidence`] does not
    /// carry `batch_id` in its payload (the command represents the
    /// post-completion evidence-publish use case, conceptually
    /// "publish evidence for the run we just completed"); the service
    /// supplies the routing key explicitly per CHE-0054:R5.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::NotCompleted`] when the resolved aggregate
    /// is not in the `Completed` phase (invariant c).
    /// Returns [`RunError::RoutingMiss`] when `batch_id` has no entry
    /// in the routing index (CHE-0024:R1 non-fatal path).
    pub async fn publish_evidence(
        &self,
        batch_id: &str,
        cmd: PublishEvidence,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        use cherry_pit_core::HandleCommand;

        let id = self.resolve_id(batch_id)?;
        let (state, last_seq) = self.load_and_fold(id).await;
        let new_events = state.handle(cmd)?;
        let new_envelopes = self.append_and_track(id, last_seq, new_events, ctx).await;
        super::shared::publish_or_trace(&self.bus, &new_envelopes, "EvidencePublished").await;
        Ok(())
    }

    // --- private append-path helpers (CHE-0054:R10 triad sub-steps) ---

    /// Resolve a `batch_id` routing key to its `AggregateId` from the
    /// index (CHE-0054:R5).
    ///
    /// # Errors
    ///
    /// Returns [`RunError::RoutingMiss`] when the routing key is
    /// unknown — callers of the four append-path methods must have
    /// called [`start_sweep`](Self::start_sweep) first. The append-path
    /// callers in `collect.rs` wrap this in a non-fatal `warn!` arm
    /// (CHE-0024:R1), so a missing routing-index entry surfaces as a
    /// log line rather than aborting the cycle.
    fn resolve_id(&self, batch_id: &str) -> Result<AggregateId, RunError> {
        let guard = self
            .index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .get(batch_id)
            .copied()
            .ok_or_else(|| RunError::RoutingMiss(batch_id.into()))
    }

    /// Load the per-aggregate event stream and fold it into a fresh
    /// [`Run`] aggregate. Returns the rebuilt state alongside the
    /// last-applied sequence (for caller-tracked CAS, CHE-0054:R6).
    ///
    /// Panics when the indexed aggregate has zero events (a routing
    /// bug — start_sweep must have produced ≥1 event for the index
    /// entry to exist) or when `EventStore::load` fails (typed
    /// surface in B7'c).
    ///
    /// [`Run`]: crate::domain::aggregates::run::Run
    async fn load_and_fold(
        &self,
        id: AggregateId,
    ) -> (crate::domain::aggregates::run::Run, NonZeroU64) {
        use cherry_pit_core::Aggregate;

        let envelopes = self
            .store
            .load(id)
            .await
            .expect("EventStore::load failure path enriched in B7'c");
        let mut state = crate::domain::aggregates::run::Run::default();
        for env in &envelopes {
            state.apply(env.payload());
        }
        let last_seq = envelopes
            .last()
            .map(cherry_pit_core::EventEnvelope::sequence)
            .expect("indexed AggregateId must have ≥1 envelope (corrupt routing otherwise)");
        (state, last_seq)
    }

    /// Append `new_events` with caller-tracked CAS on `last_seq`
    /// (CHE-0054:R6 / CHE-0042:R3) and update the sequence tracker
    /// to the resulting last sequence.
    ///
    /// Panics on `EventStore::append` failure or when the returned
    /// envelope vec is empty (the latter is impossible per the
    /// `EventStore` contract: `append` of N events returns N
    /// envelopes); typed surface in B7'c.
    async fn append_and_track(
        &self,
        id: AggregateId,
        last_seq: NonZeroU64,
        new_events: Vec<DomainEvent>,
        ctx: &CorrelationContext,
    ) -> Vec<cherry_pit_core::EventEnvelope<DomainEvent>> {
        let new_envelopes = self
            .store
            .append(id, last_seq, new_events, ctx.clone())
            .await
            .expect("EventStore::append failure path enriched in B7'c");
        if let Some(env) = new_envelopes.last() {
            let next = env.sequence();
            let mut guard = self
                .sequence_tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.insert(id, next);
        }
        new_envelopes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use cherry_pit_agent::InProcessEventBus;
    use cherry_pit_core::{EventEnvelope, EventStore};
    use cherry_pit_gateway::MsgpackFileStore;
    use tempfile::TempDir;

    /// Construct a B7'b-shaped RunService backed by a tempdir
    /// MsgpackFileStore (per Gap-β bead `adr-fmt-luxw`) and an
    /// in-process bus.
    #[allow(
        clippy::type_complexity,
        reason = "test helper returns the four shared handles plus the service; factoring would obscure the wiring under test"
    )]
    fn build_service() -> (
        TempDir,
        Arc<MsgpackFileStore<DomainEvent>>,
        Arc<InProcessEventBus<DomainEvent>>,
        Arc<Mutex<HashMap<String, AggregateId>>>,
        Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
        RunService<MsgpackFileStore<DomainEvent>, InProcessEventBus<DomainEvent>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MsgpackFileStore::<DomainEvent>::new(dir.path()));
        let bus = Arc::new(InProcessEventBus::<DomainEvent>::new());
        let index = Arc::new(Mutex::new(HashMap::new()));
        let tracker = Arc::new(Mutex::new(HashMap::new()));
        let svc = RunService::with_stores(
            Arc::clone(&store),
            Arc::clone(&bus),
            Arc::clone(&index),
            Arc::clone(&tracker),
        );
        (dir, store, bus, index, tracker, svc)
    }

    #[test]
    fn with_stores_constructs_service() {
        // Smoke test: B7'b-1 constructor surface compiles and yields a
        // service with both port handles attached. Method-body wiring
        // arrives in B7'b-2..3.
        let (_dir, _store, _bus, _index, _tracker, svc) = build_service();
        let _: &Arc<MsgpackFileStore<DomainEvent>> = svc.store();
        let _: &Arc<InProcessEventBus<DomainEvent>> = svc.bus();
    }

    /// Inc 2 (B7'b-2) — `start_sweep` create-path triad: load → handle
    /// → create → publish. Asserts:
    ///   1. `EventStore::create` ran: file `<id>.msgpack` present
    ///      (CHE-0036:R1) and contains exactly one `SweepStarted`
    ///      envelope at sequence 1.
    ///   2. `EventBus::publish` ran: registered handler captured one
    ///      envelope.
    ///   3. Routing index populated: `batch_id` → `AggregateId`
    ///      (CHE-0054:R5).
    ///   4. Sequence tracker populated: `AggregateId` → `NonZeroU64(1)`
    ///      (CHE-0054:R6 / CHE-0042:R3).
    #[tokio::test]
    async fn start_sweep_create_path_persists_and_publishes() {
        let (dir, store, bus, index, tracker, svc) = build_service();

        // Subscriber that records every published envelope.
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
        };
        let ctx = CorrelationContext::none();

        svc.start_sweep(cmd.clone(), &ctx)
            .await
            .expect("start_sweep should succeed on empty aggregate");

        // (3) Routing index populated.
        let assigned_id = {
            let guard = index
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard
                .get(&cmd.batch_id)
                .expect("index should map batch_id to AggregateId")
        };

        // (4) Sequence tracker populated.
        let tracked_seq = {
            let guard = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard
                .get(&assigned_id)
                .expect("sequence_tracker should record last applied seq")
        };
        assert_eq!(tracked_seq.get(), 1, "first event has sequence 1");

        // (1) File present per CHE-0036:R1 + envelope contents.
        let store_file = dir.path().join(format!("{assigned_id}.msgpack"));
        assert!(
            store_file.exists(),
            "MsgpackFileStore should have created {store_file:?}"
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
                assert_eq!(*repo_count, 3);
                assert_eq!(batch_id, "batch-001");
            }
            other => panic!("expected SweepStarted, got {other:?}"),
        }

        // (2) Bus subscriber received the envelope.
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

    /// Inc 3 (B7'b-3) — full Run lifecycle exercising all four
    /// append-path methods on a single aggregate:
    /// `start → progress → progress → complete → publish_evidence`.
    ///
    /// Asserts:
    ///   1. Stream contains exactly 5 envelopes at sequences 1..=5
    ///      with the expected payload variants in order.
    ///   2. Bus subscriber captured all 5 envelopes in order.
    ///   3. Sequence tracker advanced to `NonZeroU64(5)`.
    ///   4. Routing index unchanged (still maps `batch_id` →
    ///      same `AggregateId`; CHE-0054:R5).
    ///   5. Single per-aggregate file (CHE-0036:R1) — no extra
    ///      streams created by the append-path.
    #[tokio::test]
    async fn run_lifecycle_appends_persists_and_publishes() {
        let (dir, store, bus, index, tracker, svc) = build_service();

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

        // (2) Bus captured all 5 in order.
        let captured_envs = captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(captured_envs.len(), 5, "5 envelopes published");
        for (i, env) in captured_envs.iter().enumerate() {
            assert_eq!(env.sequence().get(), u64::try_from(i + 1).unwrap());
        }

        // (3) Sequence tracker == 5.
        let tracked_seq = {
            let guard = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard.get(&assigned_id).expect("sequence_tracker entry")
        };
        assert_eq!(
            tracked_seq.get(),
            5,
            "tracker should reflect last appended sequence"
        );

        // (4) Single per-aggregate file (CHE-0036:R1).
        let store_file = dir.path().join(format!("{assigned_id}.msgpack"));
        assert!(store_file.exists(), "single per-aggregate file");
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .expect("readdir")
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "msgpack"))
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "exactly one .msgpack file under the store dir"
        );
    }

    /// CHE-0024:R1 — append-path called for an unknown `batch_id`
    /// returns `RunError::RoutingMiss` rather than panicking, so the
    /// caller's non-fatal `warn!` arm in `collect.rs` can log and
    /// continue.
    #[tokio::test]
    async fn record_progress_on_unknown_batch_id_returns_routing_miss() {
        let (_dir, _store, _bus, _index, _tracker, svc) = build_service();
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
}
