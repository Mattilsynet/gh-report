//! In-memory test fixtures for `cherry-pit-core` ports.
//!
//! Gated behind `#[cfg(any(test, feature = "testing"))]` per the CHE-0058
//! carve-out over CHE-0030:R1; downstream crates opt in via
//! `cherry-pit-core = { features = ["testing"] }`. Items live under
//! `cherry_pit_core::testing::*` and are deliberately **not** re-exported
//! from the crate root — the production discoverable surface stays clean
//! (oracle §1.2 conservative scoping).
//!
//! ## Contents
//!
//! - [`FakeBus`] — `EventBus` impl that records every published envelope
//!   into an internal `Vec` for inspection. Publication is infallible by
//!   construction (CHE-0024:R1 non-fatal).
//! - [`InMemoryEventStore`] — `EventStore` impl matching the file-store
//!   reference behaviour at
//!   `crates/cherry-pit-gateway/src/event_store/msgpack_file.rs:316,437,480-525`:
//!   per-aggregate sequence counter from 1, `expected_sequence` guard,
//!   envelope construction via [`EventEnvelope::new`], `validate_stream`
//!   in [`load`](InMemoryEventStore::load) (CHE-0042:R4).
//! - [`InMemoryProjectionStore`] — concurrent map keyed by
//!   `(AggregateId, projection_name)` matching the shape CHE-0048:R5
//!   sanctions for the in-memory backend.
//!
//! ## Doctrine
//!
//! Zero added dependencies (CHE-0029:R4). All async returns use RPITIT
//! against `impl Future<Output = …> + Send` constructed from
//! `async { … }` blocks — `std::future::Future` + the `async` keyword
//! are sufficient with no runtime crate. State lives behind
//! `std::sync::Mutex` (CHE-0018:R3 — sync domain underneath async
//! port). No mutation/edit API on stored envelopes (CHE-0022:R5);
//! no `subscribe` method on `FakeBus` (CHE-0024:R2).

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::Mutex;

use crate::aggregate_id::AggregateId;
use crate::bus::EventBus;
use crate::correlation::CorrelationContext;
use crate::error::{StoreCreateResult, StoreError};
use crate::event::{DomainEvent, EventEnvelope};
use crate::store::EventStore;

// ─── FakeBus ───────────────────────────────────────────────────────

/// In-memory [`EventBus`] that records every published envelope.
///
/// Publication is infallible by construction — `publish` always resolves
/// to `Ok(())` after pushing the slice into an internal log. CHE-0024:R1
/// makes publication non-fatal, so failure-injection is out of scope; the
/// fixture exists to confirm "events were observed", not to model bus
/// faults. There is **no** `subscribe` method here — CHE-0024:R2 forbids
/// it on the `EventBus` trait surface. Tests inspect the log via
/// [`published`](Self::published).
///
/// Internals: `Vec<EventEnvelope<E>>` behind `std::sync::Mutex`.
pub struct FakeBus<E: DomainEvent> {
    published: Mutex<Vec<EventEnvelope<E>>>,
}

impl<E: DomainEvent> FakeBus<E> {
    /// Create an empty bus.
    #[must_use]
    pub fn new() -> Self {
        Self {
            published: Mutex::new(Vec::new()),
        }
    }

    /// Snapshot of every envelope ever published, in publish order.
    ///
    /// Returns a cloned `Vec`; callers cannot mutate the internal log.
    /// (CHE-0022:R5 — persisted envelopes are immutable once recorded.)
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned (i.e. a prior call
    /// panicked while holding it). Test-only code path.
    #[must_use]
    pub fn published(&self) -> Vec<EventEnvelope<E>> {
        self.published
            .lock()
            .expect("FakeBus mutex poisoned")
            .clone()
    }
}

impl<E: DomainEvent> Default for FakeBus<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: DomainEvent> EventBus for FakeBus<E> {
    type Event = E;

    async fn publish(
        &self,
        events: &[EventEnvelope<Self::Event>],
    ) -> Result<(), crate::error::BusError> {
        // Clone-on-push: trait gives us a borrow, we own a Vec internally.
        // Synchronous mutation under Mutex inside an `async fn` body keeps
        // the returned future Send without requiring a runtime crate.
        let owned: Vec<EventEnvelope<E>> = events.to_vec();
        self.published
            .lock()
            .expect("FakeBus mutex poisoned")
            .extend(owned);
        Ok(())
    }
}

// ─── InMemoryEventStore ────────────────────────────────────────────

/// In-memory [`EventStore`] matching the file-store reference behaviour.
///
/// Per-aggregate sequence counter starts at `NonZeroU64::new(1).unwrap()`;
/// envelopes are constructed exclusively via [`EventEnvelope::new`]
/// (CHE-0042:R1). [`load`](Self::load) calls
/// [`EventEnvelope::validate_stream`] before returning even though the
/// store is in-process — honouring the call is the conformance shape
/// SM-4's harness will assert on (CHE-0042:R4).
///
/// Optimistic concurrency: `append` rejects with
/// [`StoreError::ConcurrencyConflict`] when `expected_sequence` does not
/// match the stream's actual last sequence (mirror of
/// `msgpack_file.rs:514-519`).
///
/// `AggregateId` allocation: a single per-store `NonZeroU64` counter
/// behind the same `Mutex` as the streams. First `create` returns id 1,
/// second returns id 2, and so on (CHE-0011, CHE-0020:R2–R3 — store
/// assigns; callers never invent ids).
pub struct InMemoryEventStore<E: DomainEvent> {
    state: Mutex<EventStoreState<E>>,
}

struct EventStoreState<E: DomainEvent> {
    /// Per-aggregate streams. Each is contiguous from sequence 1.
    streams: HashMap<AggregateId, Vec<EventEnvelope<E>>>,
    /// Next aggregate id to allocate. Always non-zero (starts at 1).
    next_id: NonZeroU64,
}

impl<E: DomainEvent> InMemoryEventStore<E> {
    /// Create an empty store with `next_id = 1`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(EventStoreState {
                streams: HashMap::new(),
                // CHE-0020:R2 — first store-assigned id is 1.
                next_id: NonZeroU64::MIN,
            }),
        }
    }
}

impl<E: DomainEvent> Default for InMemoryEventStore<E> {
    fn default() -> Self {
        Self::new()
    }
}

/// Allocate sequential envelopes starting at `start + 1`.
///
/// Mirrors `msgpack_file.rs:316` (`build_envelopes`): one shared
/// timestamp per batch (the batch is atomic), `event_id` via
/// [`uuid::Uuid::now_v7`] (CHE-0033:R1 — in-process v7 is correct here;
/// deterministic-v7 is reserved for substrate adapters).
fn build_envelopes<E: DomainEvent>(
    id: AggregateId,
    start_sequence: u64,
    events: Vec<E>,
    context: &CorrelationContext,
) -> Result<Vec<EventEnvelope<E>>, StoreError> {
    let timestamp = jiff::Timestamp::now();
    let mut envelopes = Vec::with_capacity(events.len());
    for (i, payload) in events.into_iter().enumerate() {
        // i_u64 + start_sequence + 1, all checked.
        let i_u64 = u64::try_from(i).unwrap_or(u64::MAX);
        let raw = start_sequence
            .checked_add(i_u64)
            .and_then(|s| s.checked_add(1))
            .ok_or_else(|| {
                StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(
                    "sequence overflow",
                ))
            })?;
        let sequence = NonZeroU64::new(raw).ok_or_else(|| {
            StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(
                "sequence must be non-zero",
            ))
        })?;
        let envelope = EventEnvelope::new(
            uuid::Uuid::now_v7(),
            id,
            sequence,
            timestamp,
            context.correlation_id(),
            context.causation_id(),
            payload,
        )
        .map_err(|e| StoreError::Infrastructure(Box::new(e)))?;
        envelopes.push(envelope);
    }
    Ok(envelopes)
}

impl<E: DomainEvent> EventStore for InMemoryEventStore<E> {
    type Event = E;

    async fn load(&self, id: AggregateId) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        let state = self
            .state
            .lock()
            .expect("InMemoryEventStore mutex poisoned");
        let stream = state.streams.get(&id).cloned().unwrap_or_default();
        // CHE-0042:R4 — honour the conformance shape: validate_stream is
        // called even though the in-process construction makes corruption
        // structurally impossible. SM-4's harness asserts this contract.
        EventEnvelope::validate_stream(id, &stream)
            .map_err(|e| StoreError::CorruptData(Box::new(e)))?;
        Ok(stream)
    }

    async fn create(
        &self,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> StoreCreateResult<Self::Event> {
        if events.is_empty() {
            // Mirror msgpack_file.rs:445-450 — store.rs:176-177 contract.
            return Err(StoreError::Infrastructure(Box::<
                dyn std::error::Error + Send + Sync,
            >::from(
                "cannot create aggregate with zero events",
            )));
        }
        let mut state = self
            .state
            .lock()
            .expect("InMemoryEventStore mutex poisoned");
        let id = AggregateId::new(state.next_id);
        // Bump under the same lock so concurrent `create` calls cannot
        // collide on id allocation.
        let bumped = state.next_id.get().checked_add(1).ok_or_else(|| {
            StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(
                "aggregate ID overflow",
            ))
        })?;
        state.next_id = NonZeroU64::new(bumped).expect("bumped non-zero u64 cannot wrap to zero");

        let envelopes = build_envelopes(id, 0, events, &context)?;
        state.streams.insert(id, envelopes.clone());
        Ok((id, envelopes))
    }

    async fn append(
        &self,
        id: AggregateId,
        expected_sequence: NonZeroU64,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        if events.is_empty() {
            // store.rs:242 — empty append is a no-op.
            return Ok(Vec::new());
        }
        let mut state = self
            .state
            .lock()
            .expect("InMemoryEventStore mutex poisoned");

        // append to a never-created aggregate is an error
        // (msgpack_file.rs:504-508 + store.rs:226-228).
        let Some(stream) = state.streams.get(&id) else {
            return Err(StoreError::Infrastructure(Box::<
                dyn std::error::Error + Send + Sync,
            >::from(format!(
                "cannot append to aggregate {id}: not created (use create() first)"
            ))));
        };

        // Optimistic concurrency check (msgpack_file.rs:513-520).
        let actual_sequence = stream.last().map_or(0, |e| e.sequence().get());
        if actual_sequence != expected_sequence.get() {
            return Err(StoreError::ConcurrencyConflict {
                aggregate_id: id,
                expected_sequence,
                actual_sequence,
            });
        }

        let new_envelopes = build_envelopes(id, expected_sequence.get(), events, &context)?;
        // Re-borrow mutably to extend. (Earlier immutable borrow has ended
        // — `stream` was consumed by the if-let above.)
        let stream_mut = state
            .streams
            .get_mut(&id)
            .expect("stream existence checked above under same lock");
        stream_mut.extend(new_envelopes.iter().cloned());
        Ok(new_envelopes)
    }
}

// ─── InMemoryProjectionStore ───────────────────────────────────────

/// In-memory projection snapshot + checkpoint backend.
///
/// CHE-0048:R5 sanctions exactly this shape: concurrent map keyed by
/// `(AggregateId, projection_name)`, holding no durable state, rebuilds
/// from the [`EventStore`] on every process start (i.e. starts empty
/// each construction).
///
/// **Duplication with cherry-pit-projection.** The CHE-0048:R5 wording
/// places the in-memory backend in `cherry-pit-projection`; this core
/// fixture is the strictly-minimal trait-shape match consumed by SM-4's
/// harness (per the original SM-3 contract boundary note —
/// "duplication-permitted for v0.1"). The projection-crate variant
/// (when materialised) may carry file-store-adjacent ergonomics; both
/// coexist by design.
///
/// API mirrors `FileProjectionStore` (`load_snapshot` / `load_checkpoint` /
/// `persist` / `delete`) — the harness in SM-4 will name those methods on
/// both backends and the registrant tests will pass identical scenarios
/// against each.
pub struct InMemoryProjectionStore<P>
where
    P: Clone + Send + Sync + 'static,
{
    projection_name: String,
    state: Mutex<ProjectionState<P>>,
}

struct ProjectionState<P> {
    /// Keyed by `aggregate_id` (the `projection_name` half of the
    /// `(aggregate_id, projection_name)` tuple lives on the store
    /// instance itself — one store per projection identity, mirroring
    /// `FileProjectionStore`).
    snapshots: HashMap<AggregateId, P>,
    /// Last applied sequence per aggregate, paired with the snapshot.
    /// Mirrors `ProjectionCheckpoint::last_sequence`.
    checkpoints: HashMap<AggregateId, u64>,
}

impl<P> InMemoryProjectionStore<P>
where
    P: Clone + Send + Sync + 'static,
{
    /// Create an empty store for a single projection identity.
    #[must_use]
    pub fn new(projection_name: impl Into<String>) -> Self {
        Self {
            projection_name: projection_name.into(),
            state: Mutex::new(ProjectionState {
                snapshots: HashMap::new(),
                checkpoints: HashMap::new(),
            }),
        }
    }

    /// Stable projection identity used in the composite key.
    #[must_use]
    pub fn projection_name(&self) -> &str {
        &self.projection_name
    }

    /// Snapshot for `aggregate_id`, if one has been persisted.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned. Test-only code path.
    pub fn load_snapshot(&self, aggregate_id: AggregateId) -> Option<P> {
        self.state
            .lock()
            .expect("InMemoryProjectionStore mutex poisoned")
            .snapshots
            .get(&aggregate_id)
            .cloned()
    }

    /// Checkpoint sequence for `aggregate_id`, if one has been persisted.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned. Test-only code path.
    pub fn load_checkpoint(&self, aggregate_id: AggregateId) -> Option<u64> {
        self.state
            .lock()
            .expect("InMemoryProjectionStore mutex poisoned")
            .checkpoints
            .get(&aggregate_id)
            .copied()
    }

    /// Persist a snapshot and its companion checkpoint atomically.
    ///
    /// Mirrors `FileProjectionStore::persist` ordering semantics: in the
    /// in-memory backend both maps are updated under the same lock, so
    /// the crash-window CHE-0048:R2 worries about (snapshot persisted
    /// without checkpoint) is structurally impossible. The conformance
    /// harness in SM-4 may still exercise the rebuild-from-EventStore
    /// path for this store and assert idempotence.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned. Test-only code path.
    pub fn persist(&self, aggregate_id: AggregateId, projection: P, last_sequence: u64) {
        let mut state = self
            .state
            .lock()
            .expect("InMemoryProjectionStore mutex poisoned");
        state.snapshots.insert(aggregate_id, projection);
        state.checkpoints.insert(aggregate_id, last_sequence);
    }

    /// Remove the snapshot and checkpoint for `aggregate_id` (no-op if
    /// absent). Mirrors `FileProjectionStore::delete`.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned. Test-only code path.
    pub fn delete(&self, aggregate_id: AggregateId) {
        let mut state = self
            .state
            .lock()
            .expect("InMemoryProjectionStore mutex poisoned");
        state.snapshots.remove(&aggregate_id);
        state.checkpoints.remove(&aggregate_id);
    }
}

// ─── Smoke tests ───────────────────────────────────────────────────
//
// These are intentionally narrow: just enough to prove the fixture
// shapes compile against the trait surfaces, the auto-increment
// counter starts at 1, `expected_sequence` mismatch returns
// `ConcurrencyConflict`, unknown aggregates load empty, and
// `FakeBus::published` reflects what was published. Full conformance
// belongs in SM-4's harness (not this sub-mission).

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    // Minimal hand-rolled block_on (~30 lines, zero deps) so smoke tests
    // can await futures without pulling tokio. CHE-0029:R4 inviolate.
    // Allowed by oracle §1.3 second resolution path. Polls in a hot loop
    // on a no-op Waker — adequate for fixtures whose async paths never
    // actually yield (every future here is ready immediately because the
    // operations are sync under a Mutex).
    struct NoopWaker;
    impl Wake for NoopWaker {
        fn wake(self: Arc<Self>) {}
    }
    fn block_on<F: Future>(fut: F) -> F::Output {
        let waker: Waker = Arc::new(NoopWaker).into();
        let mut cx = Context::from_waker(&waker);
        // Pinning to the stack: `fut` is not moved after this point and
        // is dropped at end of scope. `Box::pin` would allocate; pin! does
        // not.
        let mut fut = std::pin::pin!(fut);
        loop {
            if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    enum TestEvent {
        Happened { value: u32 },
    }
    impl DomainEvent for TestEvent {
        fn event_type(&self) -> &'static str {
            "test.happened"
        }
    }

    // ── FakeBus ───────────────────────────────────────────────

    #[test]
    fn fake_bus_records_published_envelopes() {
        let bus: FakeBus<TestEvent> = FakeBus::new();
        let id = AggregateId::new(NonZeroU64::new(1).unwrap());
        let env = EventEnvelope::new(
            uuid::Uuid::now_v7(),
            id,
            NonZeroU64::new(1).unwrap(),
            jiff::Timestamp::now(),
            None,
            None,
            TestEvent::Happened { value: 7 },
        )
        .unwrap();

        block_on(bus.publish(std::slice::from_ref(&env))).unwrap();
        let log = bus.published();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].payload(), env.payload());
    }

    #[test]
    fn fake_bus_publish_is_infallible_on_empty() {
        let bus: FakeBus<TestEvent> = FakeBus::new();
        block_on(bus.publish(&[])).unwrap();
        assert!(bus.published().is_empty());
    }

    // ── InMemoryEventStore ────────────────────────────────────

    fn store() -> InMemoryEventStore<TestEvent> {
        InMemoryEventStore::new()
    }

    #[test]
    fn create_assigns_id_starting_at_1_and_seq_1() {
        let s = store();
        let (id, envs) = block_on(s.create(
            vec![TestEvent::Happened { value: 1 }],
            CorrelationContext::none(),
        ))
        .unwrap();
        assert_eq!(id.get(), 1, "first aggregate id is 1");
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].sequence().get(), 1, "first sequence is 1");
    }

    #[test]
    fn create_assigns_sequential_ids() {
        let s = store();
        let (id1, _) = block_on(s.create(
            vec![TestEvent::Happened { value: 1 }],
            CorrelationContext::none(),
        ))
        .unwrap();
        let (id2, _) = block_on(s.create(
            vec![TestEvent::Happened { value: 2 }],
            CorrelationContext::none(),
        ))
        .unwrap();
        assert_eq!(id1.get(), 1);
        assert_eq!(id2.get(), 2);
    }

    #[test]
    fn create_rejects_empty_events() {
        let s = store();
        let err = block_on(s.create(vec![], CorrelationContext::none())).unwrap_err();
        assert!(matches!(err, StoreError::Infrastructure(_)));
    }

    #[test]
    fn load_unknown_aggregate_returns_empty_vec() {
        let s = store();
        let id = AggregateId::new(NonZeroU64::new(99).unwrap());
        let envs = block_on(s.load(id)).unwrap();
        assert!(envs.is_empty());
    }

    #[test]
    fn create_then_load_roundtrips_with_contiguous_sequences() {
        let s = store();
        let (id, _) = block_on(s.create(
            vec![
                TestEvent::Happened { value: 1 },
                TestEvent::Happened { value: 2 },
                TestEvent::Happened { value: 3 },
            ],
            CorrelationContext::none(),
        ))
        .unwrap();
        let envs = block_on(s.load(id)).unwrap();
        assert_eq!(envs.len(), 3);
        for (i, e) in envs.iter().enumerate() {
            assert_eq!(e.sequence().get(), u64::try_from(i).unwrap() + 1);
            assert_eq!(e.aggregate_id(), id);
        }
    }

    #[test]
    fn append_extends_stream_with_correct_sequences() {
        let s = store();
        let (id, created) = block_on(s.create(
            vec![TestEvent::Happened { value: 1 }],
            CorrelationContext::none(),
        ))
        .unwrap();
        let last_seq = created.last().unwrap().sequence();
        let appended = block_on(s.append(
            id,
            last_seq,
            vec![
                TestEvent::Happened { value: 2 },
                TestEvent::Happened { value: 3 },
            ],
            CorrelationContext::none(),
        ))
        .unwrap();
        assert_eq!(appended.len(), 2);
        assert_eq!(appended[0].sequence().get(), 2);
        assert_eq!(appended[1].sequence().get(), 3);

        let full = block_on(s.load(id)).unwrap();
        assert_eq!(full.len(), 3);
    }

    #[test]
    fn append_rejects_stale_expected_sequence_with_conflict() {
        let s = store();
        let (id, _) = block_on(s.create(
            vec![TestEvent::Happened { value: 1 }],
            CorrelationContext::none(),
        ))
        .unwrap();
        // Real last sequence is 1; supply 99.
        let err = block_on(s.append(
            id,
            NonZeroU64::new(99).unwrap(),
            vec![TestEvent::Happened { value: 2 }],
            CorrelationContext::none(),
        ))
        .unwrap_err();
        match err {
            StoreError::ConcurrencyConflict {
                aggregate_id,
                expected_sequence,
                actual_sequence,
            } => {
                assert_eq!(aggregate_id, id);
                assert_eq!(expected_sequence.get(), 99);
                assert_eq!(actual_sequence, 1);
            }
            other => panic!("expected ConcurrencyConflict, got {other:?}"),
        }
    }

    #[test]
    fn append_to_never_created_aggregate_is_infrastructure_error() {
        let s = store();
        let phantom = AggregateId::new(NonZeroU64::new(42).unwrap());
        let err = block_on(s.append(
            phantom,
            NonZeroU64::new(1).unwrap(),
            vec![TestEvent::Happened { value: 1 }],
            CorrelationContext::none(),
        ))
        .unwrap_err();
        assert!(matches!(err, StoreError::Infrastructure(_)));
    }

    #[test]
    fn empty_append_is_noop() {
        let s = store();
        let (id, _) = block_on(s.create(
            vec![TestEvent::Happened { value: 1 }],
            CorrelationContext::none(),
        ))
        .unwrap();
        let envs = block_on(s.append(
            id,
            NonZeroU64::new(1).unwrap(),
            vec![],
            CorrelationContext::none(),
        ))
        .unwrap();
        assert!(envs.is_empty());
    }

    // ── InMemoryProjectionStore ───────────────────────────────

    #[derive(Debug, Clone, PartialEq, Default)]
    struct CounterView {
        total: u64,
    }

    #[test]
    fn projection_store_load_missing_returns_none() {
        let s: InMemoryProjectionStore<CounterView> = InMemoryProjectionStore::new("counter");
        let id = AggregateId::new(NonZeroU64::new(1).unwrap());
        assert!(s.load_snapshot(id).is_none());
        assert!(s.load_checkpoint(id).is_none());
    }

    #[test]
    fn projection_store_persist_then_load_roundtrips() {
        let s: InMemoryProjectionStore<CounterView> = InMemoryProjectionStore::new("counter");
        let id = AggregateId::new(NonZeroU64::new(1).unwrap());
        s.persist(id, CounterView { total: 5 }, 5);
        assert_eq!(s.load_snapshot(id), Some(CounterView { total: 5 }));
        assert_eq!(s.load_checkpoint(id), Some(5));
        assert_eq!(s.projection_name(), "counter");
    }

    #[test]
    fn projection_store_delete_removes_snapshot_and_checkpoint() {
        let s: InMemoryProjectionStore<CounterView> = InMemoryProjectionStore::new("counter");
        let id = AggregateId::new(NonZeroU64::new(1).unwrap());
        s.persist(id, CounterView { total: 3 }, 3);
        s.delete(id);
        assert!(s.load_snapshot(id).is_none());
        assert!(s.load_checkpoint(id).is_none());
    }
}
