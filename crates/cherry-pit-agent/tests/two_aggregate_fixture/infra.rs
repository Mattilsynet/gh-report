//! In-fixture infrastructure: `MsgpackFileStore`-backed `CommandGateway`
//! generic over an aggregate type, plus aliases binding it to `Foo` /
//! `Bar`.
//!
//! Modelled on `cherry-pit-web/tests/integration_inmem.rs::InMemGateway`.
//! No canonical `CommandGateway` impl ships in the workspace today
//! (only test-local stubs); per S7 contract `abort_if #4` fallback we
//! provide one here so the integration test exercises the real
//! `MsgpackFileStore` round-trip.

use std::sync::Arc;

use cherry_pit_core::{
    Aggregate, AggregateId, Command, CommandGateway, CorrelationContext, CreateResult,
    DispatchError, DispatchResult, EventEnvelope, EventStore, HandleCommand, StoreError,
};
use cherry_pit_gateway::MsgpackFileStore;

use super::domain::{Bar, Foo};

/// Generic `MsgpackFileStore`-backed gateway parameterised over `A`.
///
/// Each instance is bound to one aggregate type per CHE-0005:R1.
pub struct FileStoreGateway<A: Aggregate> {
    store: Arc<MsgpackFileStore<<A as Aggregate>::Event>>,
}

impl<A: Aggregate> FileStoreGateway<A> {
    pub fn new(store: Arc<MsgpackFileStore<<A as Aggregate>::Event>>) -> Self {
        Self { store }
    }
}

impl<A> CommandGateway for FileStoreGateway<A>
where
    A: Aggregate,
{
    type Aggregate = A;

    async fn create<C>(
        &self,
        cmd: C,
        context: CorrelationContext,
    ) -> CreateResult<Self::Aggregate, C>
    where
        Self::Aggregate: HandleCommand<C>,
        C: Command,
    {
        let agg = A::default();
        let events = agg.handle(cmd).map_err(DispatchError::Rejected)?;
        let (id, envelopes) = self
            .store
            .create(events, context)
            .await
            .map_err(|e| DispatchError::Infrastructure(Box::new(e)))?;
        Ok((id, envelopes))
    }

    async fn send<C>(
        &self,
        id: AggregateId,
        cmd: C,
        context: CorrelationContext,
    ) -> DispatchResult<Self::Aggregate, C>
    where
        Self::Aggregate: HandleCommand<C>,
        C: Command,
    {
        let history = self
            .store
            .load(id)
            .await
            .map_err(|e| DispatchError::Infrastructure(Box::new(e)))?;
        if history.is_empty() {
            return Err(DispatchError::AggregateNotFound { aggregate_id: id });
        }
        let mut agg = A::default();
        for env in &history {
            agg.apply(env.payload());
        }
        let last_seq = history.last().map(EventEnvelope::sequence).ok_or_else(|| {
            DispatchError::Infrastructure("loaded stream had zero sequence on tail event".into())
        })?;
        let new_events = agg.handle(cmd).map_err(DispatchError::Rejected)?;
        let envelopes = self
            .store
            .append(id, last_seq, new_events, context)
            .await
            .map_err(|e| match e {
                StoreError::ConcurrencyConflict {
                    aggregate_id,
                    expected_sequence,
                    actual_sequence,
                } => DispatchError::ConcurrencyConflict {
                    aggregate_id,
                    expected_sequence,
                    actual_sequence,
                },
                other => DispatchError::Infrastructure(Box::new(other)),
            })?;
        Ok(envelopes)
    }
}

/// Concrete gateway for `Foo`.
pub type FooGateway = FileStoreGateway<Foo>;

/// Concrete gateway for `Bar`.
pub type BarGateway = FileStoreGateway<Bar>;
