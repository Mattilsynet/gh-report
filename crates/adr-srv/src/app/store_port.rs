//! `AdrStorePort` — the persistence abstraction [`AdrService`](crate::app::service::AdrService)
//! is generic over (CHE-0098 N-R7 port seam).
//!
//! Two implementations coexist during the migration: the legacy
//! `cherry-pit-gateway` `MsgpackFileStore` (cherry-pit test-suite
//! reference `EventStore`, CHE-0098 R10) and the native pardosa
//! [`NativeAdrStore`] (CHE-0098 R1–R5, production wiring). Both are
//! keyed uniformly by [`AdrId`] at the [`AdrService`](crate::app::service::AdrService)
//! layer; only the opaque per-implementation [`AdrStorePort::Id`]
//! (needed to re-target a subsequent `append`) differs.

use std::future::Future;
use std::num::NonZeroU64;

use cherry_pit_core::{AggregateId, CorrelationContext, EventStore, ListableEventStore};
use cherry_pit_gateway::MsgpackFileStore;

use crate::domain::adr_id::AdrId;
use crate::domain::events::AdrIngested;
use crate::domain::native_event::{AdrIngestedEvent, NativeConversionError, NativeMapError};
use crate::store::{NativeAdrStore, NativeStoreError};

/// One replayed stream: its `AdrId`, opaque store handle, and events
/// in commit order. Named alias per `clippy::type_complexity`.
pub type ReplayedStream<Id> = (AdrId, Id, Vec<AdrIngested>);

/// Persistence port `AdrService` is generic over.
pub trait AdrStorePort: Send + Sync {
    /// Opaque per-aggregate handle needed to `append` a subsequent
    /// event onto the same stream. Meaningless outside this trait's
    /// own methods; [`AdrService::lookup`](crate::app::service::AdrService::lookup)
    /// exposes it verbatim for test/introspection use.
    type Id: Copy + Send + Sync;
    /// Failure surface, threaded through `AdrService`'s public
    /// `Result` types.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Persist the first event for a not-yet-known `AdrId`. Returns
    /// the opaque id plus the sequence to pass to the next `append`.
    fn create(
        &self,
        event: AdrIngested,
    ) -> impl Future<Output = Result<(Self::Id, NonZeroU64), Self::Error>> + Send;

    /// Persist a subsequent event for a known `AdrId`.
    fn append(
        &self,
        id: Self::Id,
        expected_seq: NonZeroU64,
        event: AdrIngested,
    ) -> impl Future<Output = Result<NonZeroU64, Self::Error>> + Send;

    /// Replay every persisted stream, grouped by `AdrId`, in commit
    /// order — the boot-time replay input for
    /// [`AdrService::new_with_replay`](crate::app::service::AdrService::new_with_replay).
    fn replay_all(
        &self,
    ) -> impl Future<Output = Result<Vec<ReplayedStream<Self::Id>>, Self::Error>> + Send;
}

impl AdrStorePort for MsgpackFileStore<AdrIngested> {
    type Id = AggregateId;
    type Error = cherry_pit_core::StoreError;

    async fn create(&self, event: AdrIngested) -> Result<(AggregateId, NonZeroU64), Self::Error> {
        let (id, envelopes) =
            EventStore::create(self, vec![event], CorrelationContext::none()).await?;
        let seq = envelopes
            .last()
            .expect("create returned non-empty envelopes")
            .sequence();
        Ok((id, seq))
    }

    async fn append(
        &self,
        id: AggregateId,
        expected_seq: NonZeroU64,
        event: AdrIngested,
    ) -> Result<NonZeroU64, Self::Error> {
        let envelopes = EventStore::append(
            self,
            id,
            expected_seq,
            vec![event],
            CorrelationContext::none(),
        )
        .await?;
        Ok(envelopes
            .last()
            .expect("append returned non-empty envelopes")
            .sequence())
    }

    async fn replay_all(&self) -> Result<Vec<ReplayedStream<AggregateId>>, Self::Error> {
        let ids = ListableEventStore::list_aggregates(self).await?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let envelopes = EventStore::load(self, id).await?;
            let Some(first) = envelopes.first() else {
                continue;
            };
            let adr_id = first.payload().id.clone();
            let events = envelopes.into_iter().map(|e| e.payload().clone()).collect();
            out.push((adr_id, id, events));
        }
        Ok(out)
    }
}

/// Failure surface for the [`NativeAdrStore`] [`AdrStorePort`] impl:
/// either the underlying pardosa fiber store failed, or a scrape-side
/// [`AdrIngested`] could not map onto the native durable payload
/// (CHE-0098 R3 total boundary mapping).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum NativeAdrStorePortError {
    #[error(transparent)]
    Store(#[from] NativeStoreError),
    #[error("boundary mapping to native event failed: {0}")]
    Conversion(#[from] NativeConversionError),
    #[error("boundary mapping from native event failed: {0}")]
    Reconstruct(#[from] NativeMapError),
}

impl AdrStorePort for NativeAdrStore {
    /// No opaque id is needed: the fiber key is derived from the
    /// event's own [`AdrId`] (N-R4), so `AdrService`'s `AdrId`-keyed
    /// indices carry all the identity this port needs.
    type Id = ();
    type Error = NativeAdrStorePortError;

    async fn create(&self, event: AdrIngested) -> Result<((), NonZeroU64), Self::Error> {
        let native = AdrIngestedEvent::try_from(&event)?;
        self.record(native)?;
        Ok(((), NonZeroU64::new(1).expect("1 is non-zero")))
    }

    async fn append(
        &self,
        (): (),
        _expected_seq: NonZeroU64,
        event: AdrIngested,
    ) -> Result<NonZeroU64, Self::Error> {
        let native = AdrIngestedEvent::try_from(&event)?;
        self.record(native)?;
        Ok(NonZeroU64::new(1).expect("1 is non-zero"))
    }

    async fn replay_all(&self) -> Result<Vec<ReplayedStream<()>>, Self::Error> {
        let mut grouped: std::collections::BTreeMap<AdrId, Vec<AdrIngested>> =
            std::collections::BTreeMap::new();
        for (_detached, native) in self.events()? {
            let domain: AdrIngested = AdrIngested::try_from(&native)?;
            grouped.entry(domain.id.clone()).or_default().push(domain);
        }
        Ok(grouped
            .into_iter()
            .map(|(adr_id, events)| (adr_id, (), events))
            .collect())
    }
}
