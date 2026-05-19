//! Snapshot-fast-path projection runtime wiring.
//!
//! WU-6 v2 sub-mission B5' (charter `wu6v2-charter-1778415390`):
//! composes [`ProjectionDriver`] + [`InProcessEventBus`] +
//! [`ProjectionDriverExt`] for the gh-report process.
//!
//! ## What this module wires
//!
//! **Bus-driven incremental projection updates**
//! ([`register_projection_handler`]) per CHE-0051:R2/R5 + CHE-0024:§7.
//! The handler closure registered against [`InProcessEventBus::register`]
//! locks the shared projection state, calls
//! [`ProjectionDriverExt::apply_one`] (which delegates to
//! [`Projection::apply`]), and updates the running checkpoint
//! sequence atomic. Synchronous fan-out per CHE-0024:§7 — handlers
//! do NOT spawn or await.
//!
//! Boot-time projection rehydration moved to
//! [`crate::app::state::AppState::bootstrap_replay_state`] under bd
//! `adr-fmt-5rwbu` (cpp-r-b-r-c): a single unified replay covering
//! every aggregate, folding events into both routing indices and
//! projection state — superseding the per-aggregate
//! snapshot+checkpoint fast-path (`snapshot_fast_path_startup`,
//! removed) which only rehydrated `ORG_GOVERNANCE_AGGREGATE_ID`. The
//! CHE-0048 line-24 replay-as-rebuild exemption applies: there is no
//! on-disk snapshot/checkpoint surface in the current
//! `InMemoryEventStore`-backed build.
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
use cherry_pit_projection::ProjectionDriver;
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
    use cherry_pit_core::EventEnvelope;
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
