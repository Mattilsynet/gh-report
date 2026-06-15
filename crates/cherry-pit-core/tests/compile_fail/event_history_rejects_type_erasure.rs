use std::num::NonZeroU64;

use cherry_pit_core::{
    AggregateId, CorrelationContext, DomainEvent, EventEnvelope, EventHistoryEventStore,
    EventStore, StoreCreateResult, StoreError,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CounterEvent {
    Counted,
}

impl DomainEvent for CounterEvent {
    fn event_type(&self) -> &'static str {
        "counter.counted"
    }
}

struct CounterStore;

impl EventStore for CounterStore {
    type Event = CounterEvent;

    async fn load(&self, _id: AggregateId) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        Ok(Vec::new())
    }

    async fn create(
        &self,
        _events: Vec<Self::Event>,
        _context: CorrelationContext,
    ) -> StoreCreateResult<Self::Event> {
        Ok((AggregateId::new(NonZeroU64::MIN), Vec::new()))
    }

    async fn append(
        &self,
        _id: AggregateId,
        _expected_sequence: NonZeroU64,
        _events: Vec<Self::Event>,
        _context: CorrelationContext,
    ) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        Ok(Vec::new())
    }
}

impl EventHistoryEventStore for CounterStore {
    async fn history(
        &self,
        id: AggregateId,
    ) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        self.load(id).await
    }

    async fn replay_until(
        &self,
        id: AggregateId,
        _upto: NonZeroU64,
    ) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        self.load(id).await
    }

    async fn causal_chain(
        &self,
        id: AggregateId,
        _event_id: uuid::Uuid,
    ) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        self.load(id).await
    }
}

type ErasedHistory = Box<dyn EventHistoryEventStore<Event = CounterEvent>>;

fn main() {
    let _erased: Option<ErasedHistory> = None;
}
