//! `MergerHandles` for gh-report's single durable Repo aggregate path.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_app::InProcessEventBus;
use cherry_pit_core::{AggregateId, EventBus};
use cherry_pit_merger::{Merger, MergerHandle};

use super::arms::RepoArm;
use crate::app::state::EventStoreImpl;
use crate::domain::aggregates::repo::Repo;
use crate::domain::events::DomainEvent;

pub use super::arms::RepoCmd;

type Store = EventStoreImpl;
type Bus = InProcessEventBus<DomainEvent>;

/// Durable merger handle bundle.
pub struct MergerHandles<B = Bus>
where
    B: EventBus<Event = DomainEvent> + Send + Sync + 'static,
{
    /// [`Repo`](crate::domain::aggregates::repo::Repo) dispatch handle.
    pub repo: MergerHandle<Repo, RepoArm>,
    _bus: std::marker::PhantomData<B>,
}

/// Lifetime guard for the durable repo merger task.
pub struct MergerJoinHandles {
    pub repo: tokio::task::JoinHandle<()>,
}

impl MergerHandles<Bus> {
    /// Spawn the durable repo merger task.
    #[must_use]
    pub fn spawn(
        store: Arc<Store>,
        bus: Arc<Bus>,
        repos_by_key: Arc<Mutex<HashMap<String, AggregateId>>>,
        next_seq: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
    ) -> (Self, MergerJoinHandles) {
        Self::spawn_inner::<Bus>(store, bus, repos_by_key, next_seq)
    }
}

impl<B> MergerHandles<B>
where
    B: EventBus<Event = DomainEvent> + Send + Sync + 'static,
{
    fn spawn_inner<Be>(
        store: Arc<Store>,
        bus: Arc<Be>,
        repos_by_key: Arc<Mutex<HashMap<String, AggregateId>>>,
        next_seq: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
    ) -> (MergerHandles<Be>, MergerJoinHandles)
    where
        Be: EventBus<Event = DomainEvent> + Send + Sync + 'static,
    {
        let (repo_handle, repo_join) = Merger::<Repo, _, _, _>::spawn(
            RepoArm,
            store,
            bus,
            repos_by_key,
            next_seq,
        );

        (
            MergerHandles {
                repo: repo_handle,
                _bus: std::marker::PhantomData,
            },
            MergerJoinHandles { repo: repo_join },
        )
    }

    /// Test-only seam for arbitrary event buses.
    #[doc(hidden)]
    #[must_use]
    pub fn with_bus_for_test(
        store: Arc<Store>,
        bus: Arc<B>,
        repos_by_key: Arc<Mutex<HashMap<String, AggregateId>>>,
        next_seq: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
    ) -> (Self, MergerJoinHandles) {
        Self::spawn_inner::<B>(store, bus, repos_by_key, next_seq)
    }
}
