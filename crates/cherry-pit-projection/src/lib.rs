//! Read-side adapters that drive [`cherry_pit_core::Projection`] from a
//! typed [`cherry_pit_core::EventStore`] per CHE-0048.

#![forbid(unsafe_code)]

use std::error::Error;
use std::fmt;
use std::marker::PhantomData;
use std::num::NonZeroU64;

use cherry_pit_core::{
    AggregateId, CorrelationContext, ErrorCategory, EventEnvelope, EventStore, Projection,
};

/// Errors returned by projection drivers and storage backends.
///
/// Variants split structural / unrecoverable failures
/// ([`CorruptData`](Self::CorruptData)) from transient infrastructure
/// failures ([`Infrastructure`](Self::Infrastructure)). Use
/// [`category`](Self::category) to drive retry policy per CHE-0021.
///
/// # Examples
///
/// ```
/// use cherry_pit_core::ErrorCategory;
/// use cherry_pit_projection::ProjectionError;
///
/// let corrupt = ProjectionError::CorruptData("bad bytes".into());
/// assert_eq!(corrupt.category(), ErrorCategory::Terminal);
///
/// let infra = ProjectionError::Infrastructure("disk full".into());
/// assert_eq!(infra.category(), ErrorCategory::Retryable);
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum ProjectionError {
    /// Persisted or loaded data failed structural validation.
    CorruptData(Box<dyn Error + Send + Sync>),

    /// Infrastructure failure while loading events or storing projection state.
    Infrastructure(Box<dyn Error + Send + Sync>),

    /// Advisory store-directory lock is held by another process or
    /// projection store instance. Surfaces CHE-0043:R1–R3 fencing
    /// contention to callers as a retryable failure.
    StoreLocked,

    /// `persist` was called with a `last_sequence` strictly lower than
    /// the existing on-disk checkpoint for the same aggregate. Rejected
    /// with no write performed (CHE-0097:R1).
    CheckpointRegression {
        /// The sequence already recorded on disk.
        existing: NonZeroU64,
        /// The lower sequence the caller attempted to persist.
        attempted: NonZeroU64,
    },
}

impl ProjectionError {
    /// Classify the projection failure for retry guidance.
    ///
    /// `CorruptData` and `CheckpointRegression` map to
    /// [`ErrorCategory::Terminal`] (CHE-0097:R3 — a sequence regression
    /// is a caller-side ordering bug, not transient); other variants map
    /// to [`ErrorCategory::Retryable`] (retry per CHE-0046).
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        match self {
            Self::CorruptData(_) | Self::CheckpointRegression { .. } => ErrorCategory::Terminal,
            Self::Infrastructure(_) | Self::StoreLocked => ErrorCategory::Retryable,
        }
    }

    /// Emit a structured `warn`-level event tagged with this error's
    /// retry category. Called at every public API boundary so operators
    /// see categorisation (retryable vs terminal) on every surfaced
    /// failure without instrumenting each internal `?` site (COM-0019 L04).
    fn emit_event(&self) {
        let category = match self.category() {
            ErrorCategory::Retryable => "retryable",
            ErrorCategory::Terminal => "terminal",
            _ => "unknown",
        };
        tracing::warn!(
            target: "cherry_pit_projection",
            category,
            error = %self,
            "projection error surfaced",
        );
    }
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CorruptData(e) => write!(f, "projection corrupt data: {e}"),
            Self::Infrastructure(e) => write!(f, "projection infrastructure error: {e}"),
            Self::StoreLocked => write!(
                f,
                "projection store directory is locked by another writer (CHE-0043)"
            ),
            Self::CheckpointRegression {
                existing,
                attempted,
            } => write!(
                f,
                "checkpoint sequence regression: attempted {attempted} is lower than existing {existing} (CHE-0097)"
            ),
        }
    }
}

impl Error for ProjectionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CorruptData(e) | Self::Infrastructure(e) => Some(e.as_ref()),
            Self::StoreLocked | Self::CheckpointRegression { .. } => None,
        }
    }
}

/// Result alias for projection operations.
pub type ProjectionResult<T> = Result<T, ProjectionError>;

/// Durable checkpoint for one `(aggregate_id, projection_name)` pair.
///
/// Canonical home: [`cherry_pit_core::ProjectionCheckpoint`]. Re-exported
/// here for back-compat — existing `cherry_pit_projection::ProjectionCheckpoint`
/// paths continue to resolve. Per CHE-0048 R9 the type lives in core; this
/// crate owns the file-backend storage that consumes it.
pub use cherry_pit_core::ProjectionCheckpoint;

mod write_cell;
pub use write_cell::WriteCell;

mod pardosa_store;
pub use pardosa_store::PardosaProjectionStore;

/// Ephemeral projection backend for tests and short-lived views.
///
/// The backend is parameterised by `P: Projection` and owns a single `P`
/// value in memory. It performs no durable writes and uses no dynamic
/// projection dispatch.
///
/// # Relationship to CHE-0048:R5
///
/// CHE-0048:R5 prescribes a concurrent hash map keyed by
/// `(aggregate_id, projection_name)`. In v0.1 the single-aggregate,
/// single-projection-per-driver-instance scope of CHE-0048:R6 collapses
/// that key to exactly one tuple per driver instance, so this backend
/// stores one `P` directly rather than a degenerate one-entry map. The
/// other two R5 obligations — no durable state and rebuild-from-`EventStore`
/// — are satisfied unchanged. Multi-projection composition and the
/// keyed-map shape are deferred until CHE-0048:R6 is relaxed (tracked as
/// a follow-up under epic `adr-fmt-hh07`; targeted at WU-5).
///
/// # Examples
///
/// ```
/// use cherry_pit_projection::InMemoryProjection;
/// use cherry_pit_core::{DomainEvent, EventEnvelope, Projection};
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// enum CounterEvent { Incremented }
/// impl DomainEvent for CounterEvent {
///     fn event_type(&self) -> &'static str { "counter.incremented" }
/// }
///
/// #[derive(Default)]
/// struct CounterView { total: u64 }
/// impl Projection for CounterView {
///     type Event = CounterEvent;
///     fn apply(&mut self, _event: &EventEnvelope<Self::Event>) { self.total += 1; }
/// }
///
/// let projection = InMemoryProjection::<CounterView>::new();
/// assert_eq!(projection.get().total, 0);
/// ```
#[derive(Debug, Clone)]
pub struct InMemoryProjection<P: Projection> {
    projection: P,
}

impl<P: Projection> InMemoryProjection<P> {
    /// Create an empty in-memory projection from `P::default()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            projection: P::default(),
        }
    }

    /// Borrow the current in-memory projection state.
    #[must_use]
    pub const fn get(&self) -> &P {
        &self.projection
    }

    /// Replace the in-memory projection state.
    pub fn replace(&mut self, projection: P) {
        self.projection = projection;
    }
}

impl<P: Projection> Default for InMemoryProjection<P> {
    fn default() -> Self {
        Self::new()
    }
}

/// Driver that rebuilds a projection from a typed event store.
///
/// `ProjectionDriver` is generic over a single `P: Projection` and a typed
/// `S: EventStore<Event = P::Event>` — never `Box<dyn _>` (CHE-0048:R3,
/// CHE-0005:R1). [`replay`](Self::replay) loads the full stream, runs
/// [`cherry_pit_core::EventEnvelope::validate_stream`] (CHE-0042:R4), then
/// folds events into `P::default()`.
///
/// # Examples
///
/// Construct a driver and replay a stream into a fresh projection
/// (`no_run`: signature-only, since this crate exports no concrete
/// `EventStore` impl to drive the doctest, keeping both traits generic
/// per CHE-0048:R3 + CHE-0005:R1 rather than adding a dev-dep solely
/// for doctest coverage).
///
/// ```no_run
/// use cherry_pit_core::{
///     AggregateId, DomainEvent, EventEnvelope, EventStore, Projection,
/// };
/// use cherry_pit_projection::{ProjectionDriver, ProjectionResult};
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// enum CounterEvent { Incremented }
/// impl DomainEvent for CounterEvent {
///     fn event_type(&self) -> &'static str { "counter.incremented" }
/// }
///
/// #[derive(Default, Clone, Serialize, Deserialize)]
/// struct CounterView { total: u64 }
/// impl Projection for CounterView {
///     type Event = CounterEvent;
///     fn apply(&mut self, _: &EventEnvelope<Self::Event>) { self.total += 1; }
/// }
///
/// async fn rebuild<S>(store: S, id: AggregateId) -> ProjectionResult<CounterView>
/// where
///     S: EventStore<Event = CounterEvent>,
/// {
///     let driver = ProjectionDriver::<CounterView, _>::new(store);
///     driver.replay(id, &cherry_pit_core::CorrelationContext::none()).await
/// }
/// ```
pub struct ProjectionDriver<P, S>
where
    P: Projection,
    S: EventStore<Event = P::Event>,
{
    store: S,
    _projection: PhantomData<fn() -> P>,
}

impl<P, S> ProjectionDriver<P, S>
where
    P: Projection,
    S: EventStore<Event = P::Event>,
{
    /// Create a driver over a typed event store.
    #[must_use]
    pub const fn new(store: S) -> Self {
        Self {
            store,
            _projection: PhantomData,
        }
    }

    /// Replay all events for `aggregate_id` into a fresh `P::default()`.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::CorruptData`] when the loaded stream is
    /// not valid for `aggregate_id`, and [`ProjectionError::Infrastructure`]
    /// when the underlying store load fails.
    pub async fn replay(
        &self,
        aggregate_id: AggregateId,
        correlation: &cherry_pit_core::CorrelationContext,
    ) -> ProjectionResult<P> {
        self.replay_inner(aggregate_id, correlation)
            .await
            .map(|(projection, _)| projection)
            .inspect_err(ProjectionError::emit_event)
    }

    #[tracing::instrument(
        skip(self, correlation),
        fields(
            aggregate_id = %aggregate_id.get(),
            correlation_id = ?correlation.correlation_id(),
            causation_id = ?correlation.causation_id(),
        ),
    )]
    async fn replay_inner(
        &self,
        aggregate_id: AggregateId,
        correlation: &cherry_pit_core::CorrelationContext,
    ) -> ProjectionResult<(P, Option<NonZeroU64>)> {
        let stream = self
            .store
            .load(aggregate_id)
            .await
            .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?;
        cherry_pit_core::EventEnvelope::validate_stream(aggregate_id, &stream)
            .map_err(|e| ProjectionError::CorruptData(Box::new(e)))?;
        let mut projection = P::default();
        for event in &stream {
            tracing::trace!(
                target: "cherry_pit_projection",
                event_id = %event.event_id(),
                sequence = event.sequence().get(),
                "applying event",
            );
            projection.apply(event);
        }
        let last_sequence = stream.last().map(EventEnvelope::sequence);
        Ok((projection, last_sequence))
    }
}

/// Extension trait adding per-event projection application on top of
/// [`ProjectionDriver`]'s replay-only surface.
///
/// `ProjectionDriver` ships `replay` as its only stream-level operation.
/// Live publish handlers need a
/// single-envelope entry point for incremental projection updates;
/// `apply_one` provides that without modifying CHE-0048's driver (C14).
///
/// The default impl simply delegates to [`Projection::apply`] on a
/// caller-owned mutable projection — the driver itself is stateless
/// w.r.t. the live projection (it owns only the store binding). This
/// preserves single-writer-per-aggregate (CHE-0006) by leaving the
/// projection state where the consumer chooses to keep it.
///
/// Per CHE-0057:R4 this trait must never appear as a trait object;
/// the workspace tripwire (ripgrep on `Box`+`dyn`+the trait name across
/// `crates/`) enforces the discipline.
pub trait ProjectionDriverExt<P, S>
where
    P: Projection,
    S: EventStore<Event = P::Event>,
{
    /// Apply a single event envelope to a caller-owned projection.
    ///
    /// Synchronous per CHE-0018:R1 — `Projection::apply` is sync.
    fn apply_one(&self, projection: &mut P, envelope: &EventEnvelope<P::Event>) {
        projection.apply(envelope);
    }

    /// Replay the entire stream into a fresh `P::default()`.
    ///
    /// Pass-through to [`ProjectionDriver::replay`] for ergonomic
    /// access through the extension trait surface.
    ///
    /// # Errors
    ///
    /// Surfaces [`ProjectionError`] from the underlying driver.
    fn replay_all(
        &self,
        aggregate_id: AggregateId,
        correlation: &CorrelationContext,
    ) -> impl std::future::Future<Output = ProjectionResult<P>> + Send;
}

impl<P, S> ProjectionDriverExt<P, S> for ProjectionDriver<P, S>
where
    P: Projection,
    S: EventStore<Event = P::Event>,
{
    fn replay_all(
        &self,
        aggregate_id: AggregateId,
        correlation: &CorrelationContext,
    ) -> impl std::future::Future<Output = ProjectionResult<P>> + Send {
        self.replay(aggregate_id, correlation)
    }
}

/// Heterogeneous fixed-arity tuple of [`ProjectionDriver`] instances.
///
/// Each tuple element is a distinct `ProjectionDriver<Pn, Sn>` where
/// every `(Pn, Sn)` pair is independent — the tuple shape preserves
/// per-projection type discipline (no `Box<dyn Projection>`, CHE-0005:R1).
///
/// v0.1 ships arities **0, 1 and 2** which suffice for the
/// ergonomic-benchmark gate (2-aggregate composition). Higher
/// arities up to ~8 are tracked as a `// FOLLOW-UP S7` extension gated
/// by the ergonomic benchmark — if the benchmark passes at arity 2 with
/// comfortable headroom, macro-expansion to arity 8 is purely mechanical
/// and lands in S7.
///
/// The trait is currently a marker — driver-level operations
/// (`apply_one`, `replay_all`) are exercised on the individual elements
/// via destructuring or pattern matching at the consumer site.
pub trait ProjectionDriverTuple {
    /// Number of projections in the tuple. Const-folded at the call
    /// site so consumers can `assert!(<T as ProjectionDriverTuple>::ARITY == 2)`.
    const ARITY: usize;
}

impl<P1, S1> ProjectionDriverTuple for (ProjectionDriver<P1, S1>,)
where
    P1: Projection,
    S1: EventStore<Event = P1::Event>,
{
    const ARITY: usize = 1;
}

impl<P1, S1, P2, S2> ProjectionDriverTuple for (ProjectionDriver<P1, S1>, ProjectionDriver<P2, S2>)
where
    P1: Projection,
    S1: EventStore<Event = P1::Event>,
    P2: Projection,
    S2: EventStore<Event = P2::Event>,
{
    const ARITY: usize = 2;
}

/// Marker for "no projections wired" — used when `App::new` is called
/// without projection parameters. The unit type implements
/// [`ProjectionDriverTuple`] with arity 0 so an empty composition is
/// expressible without special-casing in `App`.
impl ProjectionDriverTuple for () {
    const ARITY: usize = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use cherry_pit_core::{
        CorrelationContext, DomainEvent, EventEnvelope, StoreCreateResult, StoreError,
    };
    use serde::{Deserialize, Serialize};
    use std::num::NonZeroU64;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    enum CounterEvent {
        Incremented,
    }

    impl DomainEvent for CounterEvent {
        fn event_type(&self) -> &'static str {
            "counter.incremented"
        }
    }

    #[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct CounterView {
        total: u64,
    }

    impl Projection for CounterView {
        type Event = CounterEvent;

        fn apply(&mut self, _event: &EventEnvelope<Self::Event>) {
            self.total += 1;
        }
    }

    struct StaticStore {
        stream: Mutex<Vec<EventEnvelope<CounterEvent>>>,
    }

    impl StaticStore {
        fn new(stream: Vec<EventEnvelope<CounterEvent>>) -> Self {
            Self {
                stream: Mutex::new(stream),
            }
        }
    }

    impl EventStore for StaticStore {
        type Event = CounterEvent;

        async fn load(
            &self,
            _id: AggregateId,
        ) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
            Ok(self.stream.lock().expect("stream mutex").clone())
        }

        async fn create(
            &self,
            _events: Vec<Self::Event>,
            _context: CorrelationContext,
        ) -> StoreCreateResult<Self::Event> {
            Err(StoreError::Infrastructure("unused".into()))
        }

        async fn append(
            &self,
            _id: AggregateId,
            _expected_sequence: NonZeroU64,
            _events: Vec<Self::Event>,
            _context: CorrelationContext,
        ) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
            Err(StoreError::Infrastructure("unused".into()))
        }
    }

    fn aggregate_id(value: u64) -> AggregateId {
        AggregateId::new(NonZeroU64::new(value).expect("non-zero id"))
    }

    fn envelope(id: AggregateId, sequence: u64) -> EventEnvelope<CounterEvent> {
        EventEnvelope::new(
            uuid::Uuid::now_v7(),
            id,
            NonZeroU64::new(sequence).expect("non-zero sequence"),
            jiff::Timestamp::now(),
            None,
            None,
            CounterEvent::Incremented,
        )
        .expect("valid envelope")
    }

    #[test]
    fn inmem_defaults_to_empty_projection() {
        let backend = InMemoryProjection::<CounterView>::new();
        assert_eq!(backend.get().total, 0);
    }

    #[test]
    fn inmem_replace_updates_ephemeral_state() {
        let mut backend = InMemoryProjection::<CounterView>::new();
        backend.replace(CounterView { total: 3 });
        assert_eq!(backend.get().total, 3);
    }

    #[tokio::test]
    async fn inmem_driver_replays_valid_stream() {
        let id = aggregate_id(1);
        let store = StaticStore::new(vec![envelope(id, 1), envelope(id, 2)]);
        let driver = ProjectionDriver::<CounterView, _>::new(store);

        let projection = driver
            .replay(id, &CorrelationContext::none())
            .await
            .expect("replay succeeds");

        assert_eq!(projection.total, 2);
    }

    #[tokio::test]
    async fn validation_bad_stream_returns_typed_error_without_partial_application() {
        let id = aggregate_id(1);
        let store = StaticStore::new(vec![envelope(id, 2)]);
        let driver = ProjectionDriver::<CounterView, _>::new(store);

        let err = driver
            .replay(id, &CorrelationContext::none())
            .await
            .expect_err("invalid stream rejected");

        assert!(matches!(err, ProjectionError::CorruptData(_)));
        assert_eq!(err.category(), ErrorCategory::Terminal);
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(256))]


        /// CHE-0048:R3 — `apply` is deterministic and idempotent over a
        /// fixed event stream: replaying the same envelope sequence twice
        /// against fresh projections yields equal final states. Two
        /// independent replays exercise the property without depending on
        /// driver-internal retry semantics.
        #[test]
        fn r3_apply_is_idempotent_over_a_fixed_event_stream(count in 1_u64..32) {
            let rt = tokio::runtime::Runtime::new().expect("runtime");
            rt.block_on(async move {
                let id = aggregate_id(1);
                let stream: Vec<EventEnvelope<CounterEvent>> =
                    (1..=count).map(|seq| envelope(id, seq)).collect();

                let store_a = StaticStore::new(stream.clone());
                let driver_a = ProjectionDriver::<CounterView, _>::new(store_a);
                let first = driver_a
                    .replay(id, &CorrelationContext::none())
                    .await
                    .expect("first replay");

                let store_b = StaticStore::new(stream);
                let driver_b = ProjectionDriver::<CounterView, _>::new(store_b);
                let second = driver_b
                    .replay(id, &CorrelationContext::none())
                    .await
                    .expect("second replay");

                assert_eq!(first, second);
                assert_eq!(first.total, count);
            });
        }


    }

    #[test]
    fn apply_one_delegates_to_projection_apply() {
        let id = aggregate_id(1);
        let store = StaticStore::new(vec![]);
        let driver = ProjectionDriver::<CounterView, _>::new(store);
        let mut view = CounterView::default();
        driver.apply_one(&mut view, &envelope(id, 1));
        driver.apply_one(&mut view, &envelope(id, 2));
        assert_eq!(view.total, 2);
    }

    #[test]
    fn tuple_arity_0() {
        assert_eq!(<() as ProjectionDriverTuple>::ARITY, 0);
    }

    #[test]
    fn tuple_arity_1() {
        type T = (ProjectionDriver<CounterView, StaticStore>,);
        assert_eq!(<T as ProjectionDriverTuple>::ARITY, 1);
    }

    #[test]
    fn tuple_arity_2() {
        type T = (
            ProjectionDriver<CounterView, StaticStore>,
            ProjectionDriver<CounterView, StaticStore>,
        );
        assert_eq!(<T as ProjectionDriverTuple>::ARITY, 2);
    }
}
