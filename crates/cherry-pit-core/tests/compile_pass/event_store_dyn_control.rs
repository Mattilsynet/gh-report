use std::num::NonZeroU64;

use cherry_pit_core::{
    AggregateId, CorrelationContext, DomainEvent, EventEnvelope, EventStore, StoreCreateResult,
    StoreError,
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

fn accepts_monomorphic_store<S: EventStore<Event = CounterEvent>>(_store: &S) {}

trait DynCompatibleAnalogue {
    type Event: DomainEvent;

    fn last_event_type(&self) -> &'static str;
}

impl DynCompatibleAnalogue for CounterStore {
    type Event = CounterEvent;

    fn last_event_type(&self) -> &'static str {
        CounterEvent::Counted.event_type()
    }
}

type ErasedAnalogue = Box<dyn DynCompatibleAnalogue<Event = CounterEvent>>;

fn main() {
    accepts_monomorphic_store(&CounterStore);

    let erased: ErasedAnalogue = Box::new(CounterStore);
    assert_eq!(erased.last_event_type(), "counter.counted");
}
