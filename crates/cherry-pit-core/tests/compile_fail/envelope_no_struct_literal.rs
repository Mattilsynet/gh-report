/// Verifies that `EventEnvelope` cannot be constructed via struct
/// literal syntax — fields are private, enforcing use of the
/// validated `EventEnvelope::new()` constructor.
use cherry_pit_core::{AggregateId, DomainEvent, EventEnvelope};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

#[derive(Debug, Clone, Serialize, Deserialize)]
enum TestEvent {
    Created,
}

impl DomainEvent for TestEvent {
    fn event_type(&self) -> &'static str {
        "test.created"
    }
}

fn main() {
    let _envelope = EventEnvelope::<TestEvent> {
        event_id: uuid::Uuid::now_v7(),
        aggregate_id: AggregateId::new(NonZeroU64::new(1).unwrap()),
        sequence: NonZeroU64::new(1).unwrap(),
        timestamp: jiff::Timestamp::now(),
        correlation_id: None,
        causation_id: None,
        payload: TestEvent::Created,
    };
}
