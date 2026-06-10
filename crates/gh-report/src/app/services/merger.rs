//! `MergerHandles` — thin per-aggregate dispatch bundle backed by
//! three [`cherry_pit_merger::Merger`] instances (Mission H, bd
//! `adr-fmt-cq7vb.11`).
//!
//! Replaces the pre-mission single 8-variant `MergerCommand` enum
//! and the 700-LoC in-crate `Merger` with three thin
//! [`MergerArm`](cherry_pit_merger::MergerArm) impls
//! (see [`super::arms`]) plus three
//! [`MergerHandle`](cherry_pit_merger::MergerHandle) clones
//! ([`MergerHandles::run`], [`MergerHandles::repo`],
//! [`MergerHandles::webhook`]). All call-site signatures on
//! [`super::run_service::RunService`] /
//! [`super::repo_service::RepoService`] /
//! [`super::webhook_service::WebhookService`] stay byte-identical
//! per CHE-0054:R10; the channel envelope shape (per-aggregate
//! [`RunCmd`]/[`RepoCmd`]/[`WebhookCmd`] enums) lives strictly behind
//! the [`MergerHandle::dispatch`](cherry_pit_merger::MergerHandle::dispatch)
//! surface.
//!
//! ## I1 TOCTOU
//!
//! Closed structurally by [`cherry_pit_merger::Merger`]'s
//! single-task front door per CHE-0069:R4 — the regression pin lives
//! in `crates/cherry-pit-merger/tests/i1_toctou_pin.rs`; the
//! consumer-side equivalence remains pinned by
//! `super::repo_service::tests::concurrent_same_domain_key_evaluations_create_exactly_one_aggregate`.
//!
//! ## Per-aggregate routing indices
//!
//! Per the hopper-G back-brief, gh-report keeps its three routing
//! indices (`runs_by_key`, `repos_by_key`, vestigial
//! `deliveries_by_id`) on [`AppState`](crate::app::state::AppState).
//! [`MergerHandles::spawn`] borrows the first two via `Arc<Mutex<_>>`
//! and threads them into the appropriate per-aggregate
//! [`cherry_pit_merger::Merger`] instance at spawn time; the webhook
//! merger receives a private throw-away index because its arm uses
//! [`PersistMode::Create`](cherry_pit_merger::PersistMode::Create)
//! which never touches the borrowed handle (see [`super::arms`]
//! module docs).

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_app::InProcessEventBus;
use cherry_pit_core::{AggregateId, EventBus};
use cherry_pit_merger::{Merger, MergerHandle};

use super::arms::{RepoArm, RunArm, WebhookArm};
use crate::app::state::EventStoreImpl;
use crate::domain::aggregates::repo::Repo;
use crate::domain::aggregates::run::Run;
use crate::domain::aggregates::webhook::WebhookDelivery;
use crate::domain::events::DomainEvent;

pub use super::arms::{RepoCmd, RunCmd, WebhookCmd};

/// Concrete monomorphisation of the durable per-aggregate event store
/// wired into [`AppState`](crate::app::state::AppState).
///
/// Bound at the merger-spawn surface (rather than threaded as a
/// generic) because there is exactly one concrete pair in gh-report
/// per CHE-0005:R1 + CHE-0054 §"Open γ" resolution at Inc B7'a-6.
type Store = EventStoreImpl;
/// Concrete monomorphisation of the in-process bus. See [`Store`].
type Bus = InProcessEventBus<DomainEvent>;

/// Three per-aggregate [`MergerHandle`] clones held by
/// [`AppState`](crate::app::state::AppState) and the three
/// `ApplicationService`s.
///
/// Each handle is independently [`Clone`] (each wraps an
/// `mpsc::Sender`); the three services receive their own clone via
/// `MergerHandles::{run,repo,webhook}.clone()` and dispatch
/// independently. Three separate merger tasks (one per aggregate)
/// means concurrent dispatch across aggregates does not serialise at
/// a single channel — the I1 TOCTOU guarantee is per-aggregate per
/// CHE-0069:R4.
pub struct MergerHandles<B = Bus>
where
    B: EventBus<Event = DomainEvent> + Send + Sync + 'static,
{
    /// [`Run`](crate::domain::aggregates::run::Run) dispatch handle.
    pub run: MergerHandle<Run, RunArm>,
    /// [`Repo`](crate::domain::aggregates::repo::Repo) dispatch handle.
    pub repo: MergerHandle<Repo, RepoArm>,
    /// [`WebhookDelivery`](crate::domain::aggregates::webhook::WebhookDelivery)
    /// dispatch handle.
    pub webhook: MergerHandle<WebhookDelivery, WebhookArm>,
    /// Lifetime marker for the bus type parameter — the three
    /// underlying [`cherry_pit_merger::Merger`] instances are all
    /// generic over the same `B`.
    _bus: std::marker::PhantomData<B>,
}

/// Lifetime guard for the three [`cherry_pit_merger::Merger`] task
/// join handles. Held by
/// [`AppState`](crate::app::state::AppState); dropping it allows the
/// tasks to terminate when their handles are dropped (channel-closed
/// branch in the merger's `run` loop).
pub struct MergerJoinHandles {
    pub run: tokio::task::JoinHandle<()>,
    pub repo: tokio::task::JoinHandle<()>,
    pub webhook: tokio::task::JoinHandle<()>,
}

impl MergerHandles<Bus> {
    /// Spawn the three per-aggregate
    /// [`cherry_pit_merger::Merger`] tasks against the production
    /// [`Bus`] and return the public handle bundle plus the three
    /// [`tokio::task::JoinHandle`]s for lifetime tracking.
    ///
    /// `runs_by_key` and `repos_by_key` are the routing indices
    /// owned by [`AppState`](crate::app::state::AppState); they are
    /// borrowed (`Arc::clone`d) into the appropriate per-aggregate
    /// merger at spawn time and read on every command per CHE-0054:R5.
    /// `next_seq` is shared across all three mergers (one
    /// per-aggregate-id sequence map covers every aggregate kind).
    ///
    /// The webhook merger uses [`PersistMode::Create`](cherry_pit_merger::PersistMode::Create)
    /// throughout (no routing-index touch — see [`super::arms`]
    /// module docs), so its [`cherry_pit_merger::Merger`] receives a
    /// private throw-away index that always stays empty. The unused
    /// `deliveries_by_id` borrow on [`AppState`] is preserved for
    /// downstream consumers that may materialise it via a different
    /// mechanism later.
    #[must_use]
    pub fn spawn(
        store: Arc<Store>,
        bus: Arc<Bus>,
        runs_by_key: Arc<Mutex<HashMap<String, AggregateId>>>,
        repos_by_key: Arc<Mutex<HashMap<String, AggregateId>>>,
        next_seq: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
    ) -> (Self, MergerJoinHandles) {
        Self::spawn_inner::<Bus>(store, bus, runs_by_key, repos_by_key, next_seq)
    }
}

impl<B> MergerHandles<B>
where
    B: EventBus<Event = DomainEvent> + Send + Sync + 'static,
{
    /// Generic spawn body shared by [`Self::spawn`] (concrete [`Bus`])
    /// and [`Self::with_bus_for_test`] (arbitrary `B`).
    fn spawn_inner<Be>(
        store: Arc<Store>,
        bus: Arc<Be>,
        runs_by_key: Arc<Mutex<HashMap<String, AggregateId>>>,
        repos_by_key: Arc<Mutex<HashMap<String, AggregateId>>>,
        next_seq: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
    ) -> (MergerHandles<Be>, MergerJoinHandles)
    where
        Be: EventBus<Event = DomainEvent> + Send + Sync + 'static,
    {
        let webhook_index = Arc::new(Mutex::new(HashMap::new()));

        let (run_handle, run_join) = Merger::<Run, _, _, _>::spawn(
            RunArm,
            Arc::clone(&store),
            Arc::clone(&bus),
            runs_by_key,
            Arc::clone(&next_seq),
        );
        let (repo_handle, repo_join) = Merger::<Repo, _, _, _>::spawn(
            RepoArm,
            Arc::clone(&store),
            Arc::clone(&bus),
            repos_by_key,
            Arc::clone(&next_seq),
        );
        let (webhook_handle, webhook_join) = Merger::<WebhookDelivery, _, _, _>::spawn(
            WebhookArm,
            store,
            bus,
            webhook_index,
            next_seq,
        );

        (
            MergerHandles {
                run: run_handle,
                repo: repo_handle,
                webhook: webhook_handle,
                _bus: std::marker::PhantomData,
            },
            MergerJoinHandles {
                run: run_join,
                repo: repo_join,
                webhook: webhook_join,
            },
        )
    }

    /// Test-only seam mirroring the pre-mission `with_bus_for_test`
    /// constructor (cite the original at
    /// `merger.rs:315-333` pre-Mission-H). Spawns three
    /// [`cherry_pit_merger::Merger`] tasks over an arbitrary
    /// [`EventBus`] impl `B` — typically the `FailingBus` /
    /// `NoopBus` test doubles used by the integration tests.
    ///
    /// Marked `#[doc(hidden)]` because Rust's `#[cfg(test)]` is not
    /// visible to integration tests in `tests/` (separate crate
    /// compilation), exactly as the pre-mission constructor was.
    #[doc(hidden)]
    #[must_use]
    pub fn with_bus_for_test(
        store: Arc<Store>,
        bus: Arc<B>,
        runs_by_key: Arc<Mutex<HashMap<String, AggregateId>>>,
        repos_by_key: Arc<Mutex<HashMap<String, AggregateId>>>,
        next_seq: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
    ) -> (Self, MergerJoinHandles) {
        Self::spawn_inner::<B>(store, bus, runs_by_key, repos_by_key, next_seq)
    }
}
