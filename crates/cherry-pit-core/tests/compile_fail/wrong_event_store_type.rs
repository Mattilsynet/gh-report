/// Verifies that an EventStore typed for one event cannot be used
/// to store events of a different type.
use cherry_pit_core::{AggregateId, CorrelationContext, DomainEvent, EventEnvelope, EventStore, StoreError};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::num::NonZeroU64;

#[derive(Debug, Clone, Serialize, Deserialize)]
enum OrderEvent {
    Placed,
}

impl DomainEvent for OrderEvent {
    fn event_type(&self) -> &'static str {
        "order.placed"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum UserEvent {
    Registered,
}

impl DomainEvent for UserEvent {
    fn event_type(&self) -> &'static str {
        "user.registered"
    }
}

struct OrderStore;

impl EventStore for OrderStore {
    type Event = OrderEvent;

    fn load(
        &self,
        _id: AggregateId,
    ) -> impl Future<Output = Result<Vec<EventEnvelope<Self::Event>>, StoreError>> + Send {
        async { Ok(vec![]) }
    }

    fn create(
        &self,
        _events: Vec<Self::Event>,
        _context: CorrelationContext,
    ) -> impl Future<Output = Result<(AggregateId, Vec<EventEnvelope<Self::Event>>), StoreError>>
           + Send {
        async { Ok((AggregateId::new(NonZeroU64::new(1).unwrap()), vec![])) }
    }

    fn append(
        &self,
        _id: AggregateId,
        _seq: NonZeroU64,
        _events: Vec<Self::Event>,
        _context: CorrelationContext,
    ) -> impl Future<Output = Result<Vec<EventEnvelope<Self::Event>>, StoreError>> + Send {
        async { Ok(vec![]) }
    }
}

fn main() {
    let store = OrderStore;
    // This should fail: OrderStore produces OrderEvent, not UserEvent.
    // The future's Output type won't match.
    let future = store.load(AggregateId::new(NonZeroU64::new(1).unwrap()));
    let _: std::pin::Pin<
        Box<dyn Future<Output = Result<Vec<EventEnvelope<UserEvent>>, StoreError>>>,
    > = Box::pin(future);
}
