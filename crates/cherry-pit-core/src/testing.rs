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
use crate::store::{EventStore, ListableEventStore};

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
        let owned: Vec<EventEnvelope<E>> = events.to_vec();
        self.published
            .lock()
            .expect("FakeBus mutex poisoned")
            .extend(owned);
        Ok(())
    }
}

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
            return Ok(Vec::new());
        }
        let mut state = self
            .state
            .lock()
            .expect("InMemoryEventStore mutex poisoned");

        let Some(stream) = state.streams.get(&id) else {
            return Err(StoreError::Infrastructure(Box::<
                dyn std::error::Error + Send + Sync,
            >::from(format!(
                "cannot append to aggregate {id}: not created (use create() first)"
            ))));
        };

        let actual_sequence = stream.last().map_or(0, |e| e.sequence().get());
        if actual_sequence != expected_sequence.get() {
            return Err(StoreError::ConcurrencyConflict {
                aggregate_id: id,
                expected_sequence,
                actual_sequence,
            });
        }

        let new_envelopes = build_envelopes(id, expected_sequence.get(), events, &context)?;
        let stream_mut = state
            .streams
            .get_mut(&id)
            .expect("stream existence checked above under same lock");
        stream_mut.extend(new_envelopes.iter().cloned());
        Ok(new_envelopes)
    }
}

impl<E: DomainEvent> ListableEventStore for InMemoryEventStore<E> {
    async fn list_aggregates(&self) -> Result<Vec<AggregateId>, StoreError> {
        let state = self
            .state
            .lock()
            .expect("InMemoryEventStore mutex poisoned");
        Ok(state.streams.keys().copied().collect())
    }
}

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
    /// Mirrors `ProjectionCheckpoint::last_sequence` — `NonZeroU64`
    /// because sequence numbers start at 1 and a checkpoint exists
    /// only when at least one event has been folded.
    checkpoints: HashMap<AggregateId, NonZeroU64>,
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
    pub fn load_checkpoint(&self, aggregate_id: AggregateId) -> Option<NonZeroU64> {
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
    pub fn persist(&self, aggregate_id: AggregateId, projection: P, last_sequence: NonZeroU64) {
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

pub mod conformance {
    //! Runtime-assertable conformance for [`EventStore`], [`Aggregate`],
    //! [`Projection`].
    //!
    //! Each `fn` asserts the documented trait contract by exercising the
    //! impl through its public surface. On contract violation the fn
    //! **panics with a descriptive message** — the calling integration
    //! test fails, surfacing the specific invariant that broke. Panics
    //! are the right shape here because (a) these are test helpers, not
    //! production code paths, (b) `Aggregate::apply` is panic-only by
    //! CHE-0009:R2 so the harness is consistent with the traits it
    //! covers, and (c) `#[test]` translates panic into a failing test.
    //!
    //! ## No trait objects
    //!
    //! All fns are generic over concrete `S: EventStore`, `A: Aggregate`,
    //! `P: Projection`. There is no `Box<dyn EventStore>` / `BoxFuture`
    //! anywhere in the public surface (CHE-0005:R1, CHE-0025:R2). Async
    //! fns use RPITIT — caller awaits with its own runtime.
    //!
    //! ## Factories
    //!
    //! [`assert_event_store_conformance`] and
    //! [`assert_projection_conformance`] take a `make_store: impl Fn() -> S`
    //! closure so each scenario receives a fresh, isolated store
    //! (tempdir-backed for file stores, freshly-constructed for in-memory
    //! ones). This avoids cross-scenario state bleed without imposing a
    //! reset method on the trait surface (CHE-0022:R5 — no mutation API
    //! on stored envelopes).

    use std::num::NonZeroU64;

    use crate::aggregate::Aggregate;
    use crate::aggregate_id::AggregateId;
    use crate::correlation::CorrelationContext;
    use crate::error::StoreError;
    use crate::projection::Projection;
    use crate::store::EventStore;

    /// Assert every documented invariant of the [`EventStore`] trait
    /// against a concrete impl `S`.
    ///
    /// Scenarios exercised (each on a fresh store from `make_store`):
    ///
    /// 1. `create` → `load` round-trip preserves contiguous sequences
    ///    from `1`.
    /// 2. `append` with stale `expected_sequence` returns
    ///    [`StoreError::ConcurrencyConflict`] (CHE-0041:R3 + ref impl
    ///    `msgpack_file.rs:513-520`).
    /// 3. `load` of an unknown `AggregateId` returns `Ok(vec![])`
    ///    (CHE-0019:R1 + `store.rs:115-117`).
    /// 4. `create` with empty events returns
    ///    [`StoreError::Infrastructure`] (`store.rs:176-177`).
    /// 5. `append` to a never-created aggregate returns
    ///    [`StoreError::Infrastructure`] (CHE-0019:R3 +
    ///    `msgpack_file.rs:504-508`).
    /// 6. Monotonicity-and-gap-rejection: `append` after a successful
    ///    `create` lands sequences exactly contiguous with what
    ///    `create` produced (the doc-trait contract at
    ///    `store.rs:115-120`).
    ///
    /// Panics with a descriptive message on first violation.
    ///
    /// # Parameters
    ///
    /// - `make_store` — called once per scenario; returns a fresh,
    ///   empty `S`. Implementations backed by the filesystem typically
    ///   wrap a `tempfile::TempDir` so each scenario sees an isolated
    ///   directory.
    /// - `make_event` — produces a fresh `S::Event` per index. The
    ///   harness uses indices `0..3` to build small streams; events
    ///   need not be distinct, but distinguishable payloads make
    ///   failure messages easier to triage.
    ///
    /// # Panics
    ///
    /// Panics on any contract violation, with the violated invariant
    /// named in the panic message.
    pub async fn assert_event_store_conformance<S, F, ME>(make_store: F, make_event: ME)
    where
        S: EventStore,
        F: Fn() -> S,
        ME: Fn(u32) -> S::Event,
    {
        scenario_create_load_roundtrip::<S, _, _>(&make_store, &make_event).await;
        scenario_stale_expected_sequence::<S, _, _>(&make_store, &make_event).await;
        scenario_load_unknown_returns_empty::<S, _, _>(&make_store, &make_event).await;
        scenario_empty_create_is_infrastructure::<S, _, _>(&make_store, &make_event).await;
        scenario_append_to_phantom_is_infrastructure::<S, _, _>(&make_store, &make_event).await;
        scenario_create_then_append_is_monotone::<S, _, _>(&make_store, &make_event).await;
    }

    /// Scenario 1: `create` returns envelopes with sequences contiguous
    /// from 1, and `load` returns the same stream (store.rs:115-120).
    async fn scenario_create_load_roundtrip<S, F, ME>(make_store: &F, make_event: &ME)
    where
        S: EventStore,
        F: Fn() -> S,
        ME: Fn(u32) -> S::Event,
    {
        let store = make_store();
        let (id, created) = store
            .create(
                vec![make_event(0), make_event(1), make_event(2)],
                CorrelationContext::none(),
            )
            .await
            .expect("create with non-empty events must succeed");
        assert_eq!(
            created.len(),
            3,
            "create must return one envelope per supplied event"
        );
        for (i, env) in created.iter().enumerate() {
            let expected = u64::try_from(i).expect("i fits in u64") + 1;
            assert_eq!(
                env.sequence().get(),
                expected,
                "envelope[{i}] sequence must be {expected} (contiguous from 1)",
            );
            assert_eq!(
                env.aggregate_id(),
                id,
                "envelope[{i}] aggregate_id must match store-assigned id",
            );
        }

        let loaded = store
            .load(id)
            .await
            .expect("load after create must succeed");
        assert_eq!(
            loaded.len(),
            3,
            "load must return the full stream just created"
        );
        for (i, env) in loaded.iter().enumerate() {
            let expected = u64::try_from(i).expect("i fits in u64") + 1;
            assert_eq!(
                env.sequence().get(),
                expected,
                "loaded envelope[{i}] sequence must be {expected}",
            );
        }
    }

    /// Scenario 2: a stale `expected_sequence` on `append` must yield
    /// `StoreError::ConcurrencyConflict` echoing all three fields
    /// (CHE-0041:R3 + store.rs:269-272).
    async fn scenario_stale_expected_sequence<S, F, ME>(make_store: &F, make_event: &ME)
    where
        S: EventStore,
        F: Fn() -> S,
        ME: Fn(u32) -> S::Event,
    {
        let store = make_store();
        let (id, created) = store
            .create(vec![make_event(0)], CorrelationContext::none())
            .await
            .expect("create must succeed");
        let real_last = created
            .last()
            .expect("create returns ≥1 envelope")
            .sequence();
        let stale = NonZeroU64::new(real_last.get().saturating_add(99)).expect("non-zero by add");
        let result = store
            .append(id, stale, vec![make_event(1)], CorrelationContext::none())
            .await;
        let Err(err) = result else {
            panic!(
                "stale expected_sequence must reject; got Ok (CHE-0041:R3 + \
                 store.rs:269-272)",
            );
        };
        match err {
            StoreError::ConcurrencyConflict {
                aggregate_id,
                expected_sequence,
                actual_sequence,
            } => {
                assert_eq!(
                    aggregate_id, id,
                    "ConcurrencyConflict aggregate_id must match",
                );
                assert_eq!(
                    expected_sequence, stale,
                    "ConcurrencyConflict expected_sequence must echo caller's input",
                );
                assert_eq!(
                    actual_sequence,
                    real_last.get(),
                    "ConcurrencyConflict actual_sequence must reflect store state",
                );
            }
            other => panic!(
                "expected StoreError::ConcurrencyConflict, got {other:?} \
                 (CHE-0041:R3 + store.rs:269-272)",
            ),
        }
    }

    /// Scenario 3: `load` of an unknown `AggregateId` returns
    /// `Ok(vec![])`, not an error (CHE-0019:R1 + store.rs:115-117).
    async fn scenario_load_unknown_returns_empty<S, F, ME>(make_store: &F, _make_event: &ME)
    where
        S: EventStore,
        F: Fn() -> S,
        ME: Fn(u32) -> S::Event,
    {
        let store = make_store();
        let phantom = AggregateId::new(NonZeroU64::new(987_654_321).expect("non-zero"));
        let loaded = store
            .load(phantom)
            .await
            .expect("load of unknown aggregate must return Ok(vec![]), not error");
        assert!(
            loaded.is_empty(),
            "load of unknown aggregate must return empty Vec (CHE-0019:R1 + store.rs:115-117), \
             got {} envelopes",
            loaded.len(),
        );
    }

    /// Scenario 4: `create` with an empty events vec must return
    /// `StoreError::Infrastructure` (store.rs:176-177).
    async fn scenario_empty_create_is_infrastructure<S, F, ME>(make_store: &F, _make_event: &ME)
    where
        S: EventStore,
        F: Fn() -> S,
        ME: Fn(u32) -> S::Event,
    {
        let store = make_store();
        let result = store.create(Vec::new(), CorrelationContext::none()).await;
        let Err(err) = result else {
            panic!("create with empty events must fail; got Ok (store.rs:176-177)");
        };
        assert!(
            matches!(err, StoreError::Infrastructure(_)),
            "empty-events create must return StoreError::Infrastructure, got {err:?}",
        );
    }

    /// Scenario 5: `append` to an aggregate id that was never `create`d
    /// must return `StoreError::Infrastructure` (CHE-0019:R3).
    async fn scenario_append_to_phantom_is_infrastructure<S, F, ME>(make_store: &F, make_event: &ME)
    where
        S: EventStore,
        F: Fn() -> S,
        ME: Fn(u32) -> S::Event,
    {
        let store = make_store();
        let phantom = AggregateId::new(NonZeroU64::new(42).expect("non-zero"));
        let result = store
            .append(
                phantom,
                NonZeroU64::new(1).expect("non-zero"),
                vec![make_event(0)],
                CorrelationContext::none(),
            )
            .await;
        let Err(err) = result else {
            panic!("append to never-created aggregate must fail; got Ok (CHE-0019:R3)");
        };
        assert!(
            matches!(err, StoreError::Infrastructure(_)),
            "append-to-phantom must return StoreError::Infrastructure, got {err:?}",
        );
    }

    /// Scenario 6: `create` followed by `append` produces a monotone,
    /// gap-free, end-to-end-loadable stream.
    async fn scenario_create_then_append_is_monotone<S, F, ME>(make_store: &F, make_event: &ME)
    where
        S: EventStore,
        F: Fn() -> S,
        ME: Fn(u32) -> S::Event,
    {
        let store = make_store();
        let (id, created) = store
            .create(
                vec![make_event(0), make_event(1)],
                CorrelationContext::none(),
            )
            .await
            .expect("create must succeed");
        let last_seq = created.last().expect("≥1 envelope").sequence();
        assert_eq!(
            last_seq.get(),
            2,
            "after create of 2 events, last sequence must be 2",
        );
        let appended = store
            .append(
                id,
                last_seq,
                vec![make_event(2), make_event(3)],
                CorrelationContext::none(),
            )
            .await
            .expect("append with correct expected_sequence must succeed");
        assert_eq!(appended.len(), 2, "append returns one envelope per event");
        assert_eq!(
            appended[0].sequence().get(),
            3,
            "first appended sequence must be last_create_seq + 1",
        );
        assert_eq!(
            appended[1].sequence().get(),
            4,
            "second appended sequence must be contiguous",
        );

        let full = store
            .load(id)
            .await
            .expect("load after append must succeed");
        assert_eq!(
            full.len(),
            4,
            "stream after create+append must contain all 4 events",
        );
        for (i, env) in full.iter().enumerate() {
            let expected = u64::try_from(i).expect("i fits in u64") + 1;
            assert_eq!(
                env.sequence().get(),
                expected,
                "full stream envelope[{i}] sequence must be {expected} (monotone, gap-free)",
            );
            assert_eq!(env.aggregate_id(), id, "all envelopes scoped to id");
        }
    }

    /// Assert the [`Aggregate`] trait contract for `A`.
    ///
    /// Scenarios exercised:
    ///
    /// 1. `A::default()` constructs without panic (CHE-0012:R1 — zero state).
    /// 2. `A::default()` does NOT satisfy the caller-supplied `probe`
    ///    (i.e. the default state is distinguishable from a non-default
    ///    state — CHE-0012:R3).
    /// 3. Applying every event in `events` to a fresh default does not
    ///    panic (CHE-0009:R1 — apply is total over well-formed events).
    /// 4. After applying every event, the resulting state DOES satisfy
    ///    `probe` (i.e. the events meaningfully changed state).
    /// 5. Replay determinism: applying the same event sequence to two
    ///    fresh defaults yields states that both satisfy `probe`
    ///    (CHE-0009:R2 implicit; tightened in
    ///    [`assert_projection_conformance`] which can use `PartialEq`).
    ///
    /// # Parameters
    ///
    /// - `events` — a non-empty sequence of well-formed events that
    ///   together drive `A` to a non-default state.
    /// - `probe` — observer returning `true` iff the aggregate has
    ///   reached the non-default state induced by `events`. Caller-
    ///   supplied because [`Aggregate`] has no required state-
    ///   inspection method (CHE-0020:R1 — no `id()`, and by
    ///   generalisation no required state accessors).
    ///
    /// # Panics
    ///
    /// Panics on any contract violation. Panics if `events` is empty
    /// (the harness cannot prove the default-vs-applied distinction
    /// without at least one event).
    pub fn assert_aggregate_conformance<A>(events: &[A::Event], probe: impl Fn(&A) -> bool)
    where
        A: Aggregate,
    {
        assert!(
            !events.is_empty(),
            "assert_aggregate_conformance requires ≥1 event to distinguish default-vs-applied",
        );

        let default_a = A::default();

        assert!(
            !probe(&default_a),
            "A::default() must NOT satisfy the caller-supplied probe — the probe must \
             distinguish default-state from event-driven state (CHE-0012:R3)",
        );

        let mut a = A::default();
        for ev in events {
            a.apply(ev);
        }
        assert!(
            probe(&a),
            "after applying all events, A must satisfy probe — events did not produce \
             the expected state transition (CHE-0009:R1)",
        );

        let mut a2 = A::default();
        for ev in events {
            a2.apply(ev);
        }
        assert!(
            probe(&a2),
            "replay determinism violated: same event sequence applied to a second fresh \
             A::default() did not satisfy probe (CHE-0009:R2)",
        );
    }

    /// Assert the [`Projection`] trait contract for `P` against a
    /// concrete [`EventStore`] backing.
    ///
    /// Scenarios exercised:
    ///
    /// 1. `P::default()` constructs without panic.
    /// 2. Replay-equivalence (CHE-0048:R3): applying the envelopes
    ///    produced by a `create` call to two fresh `P::default()`
    ///    instances yields equal `P`s (per the caller-supplied
    ///    `compare`).
    /// 3. Re-replay determinism: applying the same envelope sequence
    ///    twice into the same fresh `P` yields the same outcome
    ///    relative to a single-application baseline (idempotence at
    ///    the projection-fold level; CHE-0048:R3 documented
    ///    obligation).
    ///
    /// # Parameters
    ///
    /// - `make_store` — fresh isolated store per scenario.
    /// - `make_event` — produces a fresh `P::Event` per index.
    /// - `compare` — equality observer over `P`. Pass `|a, b| a == b`
    ///   when `P: PartialEq`; supply a custom closure for projections
    ///   that intentionally do not implement `PartialEq`.
    ///
    /// # Panics
    ///
    /// Panics on any contract violation.
    pub async fn assert_projection_conformance<P, S, F, ME, C>(
        make_store: F,
        make_event: ME,
        compare: C,
    ) where
        P: Projection,
        S: EventStore<Event = P::Event>,
        F: Fn() -> S,
        ME: Fn(u32) -> P::Event,
        C: Fn(&P, &P) -> bool,
    {
        let _ = P::default();

        let store = make_store();
        let (id, _) = store
            .create(
                vec![make_event(0), make_event(1), make_event(2)],
                CorrelationContext::none(),
            )
            .await
            .expect("create must succeed");
        let envs = store
            .load(id)
            .await
            .expect("load after create must succeed for replay");
        assert_eq!(envs.len(), 3, "loaded stream must reflect created events");

        let mut p1 = P::default();
        for env in &envs {
            p1.apply(env);
        }
        let mut p2 = P::default();
        for env in &envs {
            p2.apply(env);
        }
        assert!(
            compare(&p1, &p2),
            "CHE-0048:R3 replay-equivalence violated: same envelope sequence into two \
             fresh P::default() instances did not yield equal projections",
        );

        let mut p3 = P::default();
        for env in &envs {
            p3.apply(env);
        }
        assert!(
            compare(&p1, &p3),
            "projection apply must be deterministic over identical input \
             (CHE-0009:R1 generalised to projections)",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::task::{Context, Poll, Waker};

    fn block_on<F: Future>(fut: F) -> F::Output {
        let mut cx = Context::from_waker(Waker::noop());
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
        let five = NonZeroU64::new(5).unwrap();
        s.persist(id, CounterView { total: 5 }, five);
        assert_eq!(s.load_snapshot(id), Some(CounterView { total: 5 }));
        assert_eq!(s.load_checkpoint(id), Some(five));
        assert_eq!(s.projection_name(), "counter");
    }

    #[test]
    fn projection_store_delete_removes_snapshot_and_checkpoint() {
        let s: InMemoryProjectionStore<CounterView> = InMemoryProjectionStore::new("counter");
        let id = AggregateId::new(NonZeroU64::new(1).unwrap());
        s.persist(id, CounterView { total: 3 }, NonZeroU64::new(3).unwrap());
        s.delete(id);
        assert!(s.load_snapshot(id).is_none());
        assert!(s.load_checkpoint(id).is_none());
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn evs(n: usize) -> Vec<TestEvent> {
            (0..n)
                .map(|i| TestEvent::Happened {
                    value: u32::try_from(i).unwrap(),
                })
                .collect()
        }

        proptest! {
            #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

            #[test]
            fn create_yields_contiguous_sequences_from_one(n in 1usize..16) {
                let s = store();
                let (id, envs) = block_on(s.create(evs(n), CorrelationContext::none())).unwrap();
                prop_assert_eq!(envs.len(), n);
                for (i, e) in envs.iter().enumerate() {
                    let expected = u64::try_from(i).unwrap() + 1;
                    prop_assert_eq!(e.sequence().get(), expected);
                    prop_assert_eq!(e.aggregate_id(), id);
                }
                let loaded = block_on(s.load(id)).unwrap();
                prop_assert_eq!(loaded.len(), n);
                for (i, e) in loaded.iter().enumerate() {
                    let expected = u64::try_from(i).unwrap() + 1;
                    prop_assert_eq!(e.sequence().get(), expected);
                }
            }

            #[test]
            fn append_extends_stream_monotone_across_batches(
                create_n in 1usize..6,
                batches in proptest::collection::vec(1usize..5, 1..5),
            ) {
                let s = store();
                let (id, created) = block_on(
                    s.create(evs(create_n), CorrelationContext::none()),
                ).unwrap();
                let mut last_seq = created.last().unwrap().sequence();
                let mut total = create_n;
                for batch_n in &batches {
                    let appended = block_on(s.append(
                        id,
                        last_seq,
                        evs(*batch_n),
                        CorrelationContext::none(),
                    )).unwrap();
                    prop_assert_eq!(appended.len(), *batch_n);
                    for (i, e) in appended.iter().enumerate() {
                        let expected = last_seq.get() + 1 + u64::try_from(i).unwrap();
                        prop_assert_eq!(e.sequence().get(), expected);
                        prop_assert_eq!(e.aggregate_id(), id);
                    }
                    total += batch_n;
                    last_seq = appended.last().unwrap().sequence();
                }
                let all = block_on(s.load(id)).unwrap();
                prop_assert_eq!(all.len(), total);
                for (i, e) in all.iter().enumerate() {
                    let expected = u64::try_from(i).unwrap() + 1;
                    prop_assert_eq!(e.sequence().get(), expected);
                    prop_assert_eq!(e.aggregate_id(), id);
                }
            }

            #[test]
            fn append_rejects_any_stale_expected_sequence(
                create_n in 2usize..8,
                stale_raw in 1u64..10_000,
            ) {
                let s = store();
                let (id, created) = block_on(
                    s.create(evs(create_n), CorrelationContext::none()),
                ).unwrap();
                let real = created.last().unwrap().sequence().get();
                prop_assume!(stale_raw != real);
                let stale = NonZeroU64::new(stale_raw).unwrap();
                let err = block_on(s.append(
                    id,
                    stale,
                    evs(1),
                    CorrelationContext::none(),
                )).unwrap_err();
                match err {
                    StoreError::ConcurrencyConflict {
                        aggregate_id,
                        expected_sequence,
                        actual_sequence,
                    } => {
                        prop_assert_eq!(aggregate_id, id);
                        prop_assert_eq!(expected_sequence, stale);
                        prop_assert_eq!(actual_sequence, real);
                    }
                    other => prop_assert!(false, "expected ConcurrencyConflict, got {other:?}"),
                }
                let loaded = block_on(s.load(id)).unwrap();
                prop_assert_eq!(loaded.len(), create_n);
            }

            #[test]
            fn separate_aggregates_get_distinct_sequential_ids(k in 1usize..12) {
                let s = store();
                let mut ids = Vec::with_capacity(k);
                for i in 0..k {
                    let (id, _) = block_on(s.create(
                        vec![TestEvent::Happened { value: u32::try_from(i).unwrap() }],
                        CorrelationContext::none(),
                    )).unwrap();
                    ids.push(id.get());
                }
                let expected: Vec<u64> = (1..=u64::try_from(k).unwrap()).collect();
                prop_assert_eq!(ids, expected);
            }

            #[test]
            fn fake_bus_records_duplicate_publishes_in_order(n in 1usize..16) {
                let bus: FakeBus<TestEvent> = FakeBus::new();
                let id = AggregateId::new(NonZeroU64::new(1).unwrap());
                let envs: Vec<EventEnvelope<TestEvent>> = (1..=n)
                    .map(|i| {
                        EventEnvelope::new(
                            uuid::Uuid::now_v7(),
                            id,
                            NonZeroU64::new(u64::try_from(i).unwrap()).unwrap(),
                            jiff::Timestamp::now(),
                            None,
                            None,
                            TestEvent::Happened { value: u32::try_from(i).unwrap() },
                        )
                        .unwrap()
                    })
                    .collect();
                block_on(bus.publish(&envs)).unwrap();
                block_on(bus.publish(&envs)).unwrap();
                let log = bus.published();
                prop_assert_eq!(log.len(), n * 2);
                for (i, e) in log[..n].iter().enumerate() {
                    let expected = u64::try_from(i).unwrap() + 1;
                    prop_assert_eq!(e.sequence().get(), expected);
                }
                for (i, e) in log[n..].iter().enumerate() {
                    let expected = u64::try_from(i).unwrap() + 1;
                    prop_assert_eq!(e.sequence().get(), expected);
                }
            }
        }
    }
}
