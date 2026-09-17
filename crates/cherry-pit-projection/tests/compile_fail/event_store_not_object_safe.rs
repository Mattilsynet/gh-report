use cherry_pit_core::{DomainEvent, EventStore};
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

fn _erase(_s: Box<dyn EventStore<Event = CounterEvent>>) {}

fn main() {}
