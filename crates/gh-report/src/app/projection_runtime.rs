//! Snapshot-fast-path projection runtime wiring.
//!
//! WU-6 v2 sub-mission B5' (charter `wu6v2-charter-1778415390`):
//! composes [`ProjectionDriver`] + [`InProcessEventBus`] +
//! [`ProjectionDriverExt`] for the gh-report process.
//!
//! ## What this module wires
//!
//! 1. **Snapshot-fast-path startup** ([`snapshot_fast_path_startup`]) per
//!    CHE-0051:R5 + CHE-0048:R2. On daemon start: load the persisted
//!    snapshot + checkpoint via [`FileProjectionStore`]; replay only
//!    events whose `sequence > checkpoint.last_sequence` from the
//!    [`InMemoryEventStore`] event store. Cold start (no checkpoint or no
//!    snapshot) replays the full stream into `EvidenceProjection::default()`.
//!    Per CHE-0048:R2, a present snapshot with an *absent* checkpoint
//!    must be treated as "rebuild" — the snapshot is discarded and the
//!    full stream is replayed (the persist invariant guarantees the
//!    checkpoint is written strictly after the snapshot, so a missing
//!    checkpoint signals a crash mid-`persist`).
//!
//! 2. **Bus-driven incremental projection updates**
//!    ([`register_projection_handler`]) per CHE-0051:R2/R5 + CHE-0024:§7.
//!    The handler closure registered against [`InProcessEventBus::register`]
//!    locks the shared projection state, calls
//!    [`ProjectionDriverExt::apply_one`] (which delegates to
//!    [`Projection::apply`]), and updates the running checkpoint
//!    sequence atomic. Synchronous fan-out per CHE-0024:§7 — handlers
//!    do NOT spawn or await.
//!
//! ## What this module does NOT wire (locked-out)
//!
//! - **No [`cherry_pit_agent::App`]**: the agent's `App` requires a
//!   `CommandGateway` (CHE-0051:R3) and the **S5.b bus-only lock**
//!   (charter §0 locked posture #3) forbids `CommandGateway` /
//!   `Aggregate` impl / `HandleCommand`. We therefore wire the bus +
//!   driver + projection state directly without going through `App`.
//!   Only [`InProcessEventBus`] and the [`ProjectionDriverExt`] trait
//!   from cherry-pit-agent are used; no `register_policy`, no policy
//!   registry, no `App::run`.
//!
//! - **No multi-aggregate composition**: per the **Tension-2 single
//!   aggregate lock** there is exactly one `OrgGovernance`-bound
//!   `EvidenceProjection` per process, keyed by the singleton
//!   [`crate::projection::ORG_GOVERNANCE_AGGREGATE_ID`].
//!
//! ## File-lock note (historical — no longer load-bearing)
//!
//! (was: `PardosaFileEventStore::open` acquired an exclusive advisory
//! `flock(2)` on `{dir}/.lock` at open time and held it for the store's
//! lifetime under CHE-0043:R1; the startup replay path therefore shared
//! the durable `Arc<PardosaFileEventStore<DomainEvent>>` held by
//! `AppState` into the [`ProjectionDriver`] via the [`SharedStore`]
//! newtype below. See follow-up bd issue for the PGNO-backed successor;
//! [`SharedStore`] survives the substitution to keep the consumer
//! surface stable.)
//!
//! ## Why a `Mutex<EvidenceProjection>` and not lock-free
//!
//! Per CHE-0024:§7 in-process bus delivery is synchronous within
//! `publish` — the handler runs to completion before `publish`
//! returns. CHE-0006 single-writer-per-aggregate means the projection
//! has exactly one writer (the bus handler). A `std::sync::Mutex`
//! suffices: contention is between (a) the bus handler (writer) and
//! (b) future read-side consumers (B8' render path, lazy metrics).
//! `parking_lot` / RCU is overkill at gh-report's scale.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use cherry_pit_agent::{InProcessEventBus, ProjectionDriverExt};
use cherry_pit_core::{
    AggregateId, CorrelationContext, EventEnvelope, EventStore, StoreCreateResult, StoreError,
};
use cherry_pit_core::testing::InMemoryEventStore;
use cherry_pit_projection::{
    FileProjectionStore, ProjectionDriver, ProjectionError, ProjectionResult,
};
use std::num::NonZeroU64;

use crate::domain::events::DomainEvent;
use crate::projection::EvidenceProjection;

/// Shareable handle around an [`Arc<InMemoryEventStore<E>>`].
///
/// Interim substrate until the PGNO-backed successor `EventStore` lands
/// (follow-up to mission `cherry-pit-pardosa-deletion-1779215265`); the
/// newtype is preserved because its consumer surface (`SharedStore`
/// constructor + `EventStore` impl) is referenced throughout the
/// runtime, but the historical flock semantics (CHE-0043:R1) no
/// longer apply — `InMemoryEventStore` has no on-disk surface.
///
/// All [`EventStore`] methods delegate transparently to the inner
/// store via deref-through-Arc.
#[derive(Clone)]
pub struct SharedStore<E>(Arc<InMemoryEventStore<E>>)
where
    E: cherry_pit_core::DomainEvent;

impl<E> SharedStore<E>
where
    E: cherry_pit_core::DomainEvent,
{
    /// Wrap a shared [`InMemoryEventStore`] for driver use.
    #[must_use]
    pub fn new(inner: Arc<InMemoryEventStore<E>>) -> Self {
        Self(inner)
    }
}

impl<E> EventStore for SharedStore<E>
where
    E: cherry_pit_core::DomainEvent,
{
    type Event = E;

    fn load(
        &self,
        id: AggregateId,
    ) -> impl std::future::Future<Output = Result<Vec<EventEnvelope<Self::Event>>, StoreError>> + Send
    {
        let store = Arc::clone(&self.0);
        async move { store.load(id).await }
    }

    fn create(
        &self,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> impl std::future::Future<Output = StoreCreateResult<Self::Event>> + Send {
        let store = Arc::clone(&self.0);
        async move { store.create(events, context).await }
    }

    fn append(
        &self,
        id: AggregateId,
        expected_sequence: NonZeroU64,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> impl std::future::Future<Output = Result<Vec<EventEnvelope<Self::Event>>, StoreError>> + Send
    {
        let store = Arc::clone(&self.0);
        async move { store.append(id, expected_sequence, events, context).await }
    }
}

/// Output of [`snapshot_fast_path_startup`]: the materialised projection
/// state and the highest sequence applied.
///
/// Cold start (no snapshot) returns `last_applied_sequence = 0`.
#[derive(Debug)]
pub struct StartupState {
    /// Materialised projection (snapshot + post-checkpoint replay folded in).
    pub projection: EvidenceProjection,
    /// Highest event sequence folded into [`Self::projection`]. `0` when
    /// the event stream is empty.
    pub last_applied_sequence: u64,
    /// `true` when a usable snapshot+checkpoint pair was loaded; `false`
    /// when the cold-start full-replay path was taken.
    pub used_snapshot_fast_path: bool,
}

/// Startup procedure per CHE-0051:R5 + CHE-0048:R2.
///
/// Sequence:
///
/// 1. Load checkpoint via `projection_store.load_checkpoint(id)`. If
///    absent → cold start (full replay; persist invariant means no
///    checkpoint ⇒ no trustworthy snapshot).
/// 2. Load snapshot via `projection_store.load_snapshot(id)`. If absent
///    despite checkpoint present → corrupt-state condition; treat as
///    cold start and surface a `tracing::warn`.
/// 3. Load full event stream from `event_store`. Filter envelopes by
///    `seq > checkpoint.last_sequence`.
/// 4. Apply each filtered envelope to the snapshot in order via
///    [`ProjectionDriverExt::apply_one`].
///
/// Returns the materialised state. The caller stores it under
/// `Arc<Mutex<EvidenceProjection>>` and registers a bus handler via
/// [`register_projection_handler`].
///
/// ## Concurrency / aborts
///
/// Pre-mortem B5' abort: cold-start replay >10× warm-start under
/// synthetic load. The fast-path filter (`seq > checkpoint`) is an
/// `O(N)` scan of the loaded stream; on a warm checkpoint where N
/// post-checkpoint events is small this is dominated by snapshot
/// deserialisation cost (single msgpack file). Cold start replays
/// the full log (linear in stream length).
///
/// # Errors
///
/// Surfaces [`ProjectionError`] from snapshot/checkpoint load,
/// [`ProjectionError::Infrastructure`] wrapping
/// `cherry_pit_core::StoreError` from event-store load, or
/// [`ProjectionError::CorruptData`] from
/// `EventEnvelope::validate_stream`.
pub async fn snapshot_fast_path_startup(
    event_store: Arc<InMemoryEventStore<DomainEvent>>,
    projection_store: &FileProjectionStore<EvidenceProjection>,
    aggregate_id: AggregateId,
) -> ProjectionResult<StartupState> {
    let checkpoint = projection_store.load_checkpoint(aggregate_id).await?;

    let (mut projection, replay_from_seq, used_fast_path) = match checkpoint {
        Some(cp) => {
            if let Some(snap) = projection_store.load_snapshot(aggregate_id).await? {
                (snap, cp.last_sequence(), true)
            } else {
                tracing::warn!(
                    aggregate_id = %aggregate_id.get(),
                    last_sequence = cp.last_sequence(),
                    "checkpoint present but snapshot missing — invariant violation; \
                     falling back to cold-start full replay"
                );
                (EvidenceProjection::default(), 0_u64, false)
            }
        }
        None => (EvidenceProjection::default(), 0_u64, false),
    };

    // Load full stream from the shared durable event store. The driver
    // (constructed below) shares the same Arc, so no second open() —
    // and therefore no second flock — is taken (CHE-0043:R1). Validation
    // happens inside cherry_pit_core::EventEnvelope::validate_stream
    // when invoked through ProjectionDriver::replay; here we filter
    // ourselves and apply via ProjectionDriverExt::apply_one, so we
    // run validate_stream explicitly to preserve CHE-0042:R4 semantics.
    let stream = event_store
        .load(aggregate_id)
        .await
        .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?;
    cherry_pit_core::EventEnvelope::validate_stream(aggregate_id, &stream)
        .map_err(|e| ProjectionError::CorruptData(Box::new(e)))?;

    // Construct a transient driver to satisfy the brief's wiring
    // shape ("ProjectionDriver + ProjectionDriverExt"). The driver
    // wraps SharedStore<DomainEvent>(Arc::clone(&event_store)); under
    // CHE-0043:R1 this is the only safe pattern (a fresh open() on
    // the same dir would return StoreError::StoreLocked because the
    // caller's Arc still owns the directory flock). apply_one's
    // default impl never calls into the store, so the shared handle
    // is read-only on this path in practice.
    let driver =
        ProjectionDriver::<EvidenceProjection, _>::new(SharedStore::new(Arc::clone(&event_store)));

    let mut last_seq = replay_from_seq;
    for envelope in stream
        .iter()
        .filter(|e| e.sequence().get() > replay_from_seq)
    {
        driver.apply_one(&mut projection, envelope);
        last_seq = envelope.sequence().get();
    }

    Ok(StartupState {
        projection,
        last_applied_sequence: last_seq,
        used_snapshot_fast_path: used_fast_path,
    })
}

/// Register a bus handler that drives [`ProjectionDriverExt::apply_one`]
/// for every published envelope.
///
/// Per CHE-0024:§7 the handler runs synchronously inside
/// [`InProcessEventBus::publish`]. The handler:
///
/// 1. Locks `projection_state` (poisoned-lock recovery via
///    `PoisonError::into_inner` so a panicking earlier handler does
///    not stall the bus).
/// 2. Calls `driver.apply_one(&mut *guard, envelope)`.
/// 3. Updates `checkpoint_seq` to the envelope's sequence (max).
///
/// `driver` is moved into the closure (single-aggregate, single
/// driver per process — Tension-2 lock).
///
/// ## What this does NOT do
///
/// - **No snapshot persistence on every event.** B5' wires the
///   in-memory checkpoint atomic only; durable
///   `projection_store.persist(...)` is a separate concern (eager-
///   snapshot-on-append vs periodic-checkpoint trade-off; default is
///   "periodic, driven by the daemon's collection loop", out of
///   scope for B5'). The atomic exists so a future scheduler can
///   read the running sequence without locking the projection.
/// - **No retry / dead-letter on handler panic.** Handlers must not
///   panic (CHE-0024:§7). A panicking handler poisons the mutex; the
///   recovery path keeps the bus live but the projection state may
///   be inconsistent for the panicked envelope.
pub fn register_projection_handler<S>(
    bus: &InProcessEventBus<DomainEvent>,
    driver: Arc<ProjectionDriver<EvidenceProjection, S>>,
    projection_state: Arc<Mutex<EvidenceProjection>>,
    checkpoint_seq: Arc<AtomicU64>,
) where
    S: EventStore<Event = DomainEvent> + Send + Sync + 'static,
{
    bus.register(move |envelope| {
        let mut guard = projection_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        driver.apply_one(&mut *guard, envelope);
        let seq = envelope.sequence().get();
        // Monotonic max-store: bus delivers envelopes in publish order
        // but a future re-ordering subscription model (or test injection)
        // could deliver out of order. Use fetch_max to preserve the
        // "last_applied_sequence is monotonically non-decreasing"
        // invariant.
        checkpoint_seq.fetch_max(seq, Ordering::AcqRel);
    });
}

/// Smallest non-zero sequence; used by tests and as a sentinel for
/// "no envelope has been applied yet" (the atomic carries a `u64` so
/// `0` is the natural "none" value).
pub const NO_SEQUENCE_APPLIED: u64 = 0;

#[cfg(test)]
mod tests {
    use super::*;

    use crate::projection::ORG_GOVERNANCE_AGGREGATE_ID;
    use cherry_pit_core::{CorrelationContext, EventEnvelope};
    use jiff::Timestamp;
    use std::num::NonZeroU64;

    fn envelope(seq: u64, payload: DomainEvent) -> EventEnvelope<DomainEvent> {
        EventEnvelope::new(
            uuid::Uuid::now_v7(),
            ORG_GOVERNANCE_AGGREGATE_ID,
            NonZeroU64::new(seq).expect("non-zero seq"),
            Timestamp::now(),
            None,
            None,
            payload,
        )
        .expect("valid envelope")
    }

    fn repo_removed(key: &str) -> DomainEvent {
        DomainEvent::RepoRemoved {
            domain_key: key.into(),
            repo_name: key.into(),
            timestamp: "2026-04-20T12:00:00Z".into(),
        }
    }

    fn sweep_started() -> DomainEvent {
        DomainEvent::SweepStarted {
            org: "org".into(),
            repo_count: 1,
            batch_id: "b".into(),
            timestamp: "2026-04-20T12:00:00Z".into(),
            snapshot_signature: None,
        }
    }

    /// Cold start (no snapshot, no checkpoint) replays the full event
    /// stream into a fresh `EvidenceProjection::default()`. Pre-flight
    /// for the snapshot-fast-path branch.
    #[tokio::test]
    async fn cold_start_replays_full_log_when_no_checkpoint() {
        let tmp = tempfile::tempdir().expect("tmp");
        let events_dir = tmp.path().join("events");
        let projections_dir = tmp.path().join("projections");
        std::fs::create_dir_all(&events_dir).expect("mkdir events");
        std::fs::create_dir_all(&projections_dir).expect("mkdir projections");

        // Seed the event store with 3 events via create + append.
        let event_store =
            Arc::new(InMemoryEventStore::<DomainEvent>::new());
            // (was: PardosaFileEventStore::<DomainEvent>::open(&events_dir).expect("open"); see follow-up bd issue)
            let _ = &events_dir;
        let ctx = CorrelationContext::none();
        let (id, initial) = event_store
            .create(vec![sweep_started(), repo_removed("ghost-1")], ctx.clone())
            .await
            .expect("create");
        // The store assigns its own next id; we only support the
        // singleton in production but the test store is fresh.
        let last_seq_nz = initial.last().expect("non-empty").sequence();
        event_store
            .append(id, last_seq_nz, vec![repo_removed("ghost-2")], ctx)
            .await
            .expect("append");

        let projection_store =
            FileProjectionStore::<EvidenceProjection>::new(&projections_dir, "evidence");

        let state = snapshot_fast_path_startup(Arc::clone(&event_store), &projection_store, id)
            .await
            .expect("startup");

        assert!(!state.used_snapshot_fast_path, "no snapshot ⇒ cold path");
        assert_eq!(state.last_applied_sequence, 3);
        // Both RepoRemoved events are no-op-on-empty per
        // EvidenceProjection::apply, so repositories stays empty;
        // assertion is on the sequence accounting (the substantive
        // proof of cold-replay).
        assert!(state.projection.repositories.is_empty());
    }

    /// Snapshot-fast-path: when a snapshot+checkpoint exist, only
    /// envelopes with `seq > checkpoint.last_sequence` are applied.
    /// This is the load-bearing B5' assertion.
    #[tokio::test]
    async fn snapshot_fast_path_skips_replayed_events() {
        let tmp = tempfile::tempdir().expect("tmp");
        let events_dir = tmp.path().join("events");
        let projections_dir = tmp.path().join("projections");
        std::fs::create_dir_all(&events_dir).expect("mkdir events");
        std::fs::create_dir_all(&projections_dir).expect("mkdir projections");

        // Seed event store with seq 1..=3.
        let event_store =
            Arc::new(InMemoryEventStore::<DomainEvent>::new());
            // (was: PardosaFileEventStore::<DomainEvent>::open(&events_dir).expect("open"); see follow-up bd issue)
            let _ = &events_dir;
        let ctx = CorrelationContext::none();
        let (id, initial) = event_store
            .create(vec![sweep_started(), repo_removed("k1")], ctx.clone())
            .await
            .expect("create");
        let last_seq_nz = initial.last().expect("nonempty").sequence();
        event_store
            .append(id, last_seq_nz, vec![repo_removed("k2")], ctx)
            .await
            .expect("append");

        // Persist a snapshot tagged with checkpoint.last_sequence = 2,
        // simulating "we've already applied envelopes 1 and 2; the
        // restart must apply only envelope 3."
        let projection_store =
            FileProjectionStore::<EvidenceProjection>::new(&projections_dir, "evidence");
        let snap = EvidenceProjection::default();
        projection_store
            .persist(id, &snap, 2)
            .await
            .expect("persist");

        let state = snapshot_fast_path_startup(Arc::clone(&event_store), &projection_store, id)
            .await
            .expect("startup");

        assert!(
            state.used_snapshot_fast_path,
            "snapshot present ⇒ fast path"
        );
        assert_eq!(state.last_applied_sequence, 3, "only seq 3 applied");
    }

    /// Per CHE-0048:R2: a present snapshot with an *absent* checkpoint
    /// signals a crash mid-`persist`. Treat as "rebuild" (cold start),
    /// not "trust snapshot".
    #[tokio::test]
    async fn orphan_snapshot_without_checkpoint_falls_back_to_cold_start() {
        // FileProjectionStore::load_checkpoint validates identity, so
        // we can't easily simulate "checkpoint exists, snapshot does
        // not" through the public API in one direction without
        // hand-writing files. The reverse case ("snapshot exists,
        // checkpoint missing") is the actual CHE-0048:R2 invariant
        // and is tested directly: load_checkpoint returns None ⇒ cold
        // start regardless of any orphan snapshot.
        let tmp = tempfile::tempdir().expect("tmp");
        let events_dir = tmp.path().join("events");
        let projections_dir = tmp.path().join("projections");
        std::fs::create_dir_all(&events_dir).expect("mkdir e");
        std::fs::create_dir_all(&projections_dir).expect("mkdir p");

        let event_store =
            Arc::new(InMemoryEventStore::<DomainEvent>::new());
            // (was: PardosaFileEventStore::<DomainEvent>::open(&events_dir).expect("open"); see follow-up bd issue)
            let _ = &events_dir;
        let ctx = CorrelationContext::none();
        let (id, initial) = event_store
            .create(vec![sweep_started()], ctx)
            .await
            .expect("create");
        let _ = initial;

        // Hand-write a snapshot file with NO sibling checkpoint.
        let projection_store =
            FileProjectionStore::<EvidenceProjection>::new(&projections_dir, "evidence");
        let snap_path = projection_store.snapshot_path(id);
        let bytes =
            rmp_serde::encode::to_vec_named(&EvidenceProjection::default()).expect("encode");
        std::fs::write(&snap_path, bytes).expect("write orphan snapshot");
        assert!(snap_path.exists());
        assert!(!projection_store.checkpoint_path(id).exists());

        let state = snapshot_fast_path_startup(Arc::clone(&event_store), &projection_store, id)
            .await
            .expect("startup");

        assert!(
            !state.used_snapshot_fast_path,
            "checkpoint absent ⇒ cold path even though snapshot exists"
        );
        assert_eq!(state.last_applied_sequence, 1);
    }

    /// Bus handler wiring: a registered handler mutates the shared
    /// projection state and updates the checkpoint atomic.
    #[tokio::test]
    async fn bus_handler_applies_envelope_to_shared_state() {
        use cherry_pit_core::EventBus;

        let tmp = tempfile::tempdir().expect("tmp");
        let events_dir = tmp.path().join("events");
        std::fs::create_dir_all(&events_dir).expect("mkdir");
        let store =
            Arc::new(InMemoryEventStore::<DomainEvent>::new());
            // (was: PardosaFileEventStore::<DomainEvent>::open(&events_dir).expect("open"); see follow-up bd issue)
            let _ = &events_dir;
        let driver = Arc::new(ProjectionDriver::<EvidenceProjection, _>::new(
            SharedStore::new(Arc::clone(&store)),
        ));

        let projection_state = Arc::new(Mutex::new(EvidenceProjection::default()));
        let checkpoint_seq = Arc::new(AtomicU64::new(NO_SEQUENCE_APPLIED));
        let bus: InProcessEventBus<DomainEvent> = InProcessEventBus::new();

        register_projection_handler(
            &bus,
            Arc::clone(&driver),
            Arc::clone(&projection_state),
            Arc::clone(&checkpoint_seq),
        );

        bus.publish(&[envelope(1, sweep_started()), envelope(2, repo_removed("k"))])
            .await
            .expect("publish");

        // Both envelopes applied — checkpoint advances to max sequence.
        assert_eq!(checkpoint_seq.load(Ordering::Acquire), 2);
        // RepoRemoved on empty map is a no-op (idempotent); the
        // assertion of interest is sequence accounting.
        assert!(projection_state.lock().unwrap().repositories.is_empty());
    }

    /// Out-of-order envelope publishes still leave the atomic at the
    /// max sequence (`fetch_max` guarantee). Guards against a future
    /// publisher that batches envelopes non-monotonically.
    #[tokio::test]
    async fn checkpoint_atomic_uses_max_not_last_publish() {
        use cherry_pit_core::EventBus;

        let tmp = tempfile::tempdir().expect("tmp");
        let events_dir = tmp.path().join("events");
        std::fs::create_dir_all(&events_dir).expect("mkdir");
        let store =
            Arc::new(InMemoryEventStore::<DomainEvent>::new());
            // (was: PardosaFileEventStore::<DomainEvent>::open(&events_dir).expect("open"); see follow-up bd issue)
            let _ = &events_dir;
        let driver = Arc::new(ProjectionDriver::<EvidenceProjection, _>::new(
            SharedStore::new(Arc::clone(&store)),
        ));

        let projection_state = Arc::new(Mutex::new(EvidenceProjection::default()));
        let checkpoint_seq = Arc::new(AtomicU64::new(0));
        let bus: InProcessEventBus<DomainEvent> = InProcessEventBus::new();
        register_projection_handler(
            &bus,
            Arc::clone(&driver),
            Arc::clone(&projection_state),
            Arc::clone(&checkpoint_seq),
        );

        // Publish seq 5, then seq 3 (out of order).
        bus.publish(&[envelope(5, sweep_started())]).await.unwrap();
        bus.publish(&[envelope(3, sweep_started())]).await.unwrap();

        assert_eq!(
            checkpoint_seq.load(Ordering::Acquire),
            5,
            "max sequence wins"
        );
    }
}
