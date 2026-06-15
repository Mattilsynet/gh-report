/// Verifies that a typed read-side port cannot be erased behind a trait object.
use cherry_pit_core::{DomainEvent, EventEnvelope, Projection, ReadPort};
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

#[derive(Default)]
struct CounterProjection;

impl Projection for CounterProjection {
    type Event = CounterEvent;

    fn apply(&mut self, _event: &EventEnvelope<Self::Event>) {}
}

struct CounterRead;

impl ReadPort for CounterRead {
    type Projection = CounterProjection;
    type Query = ();
    type Response = usize;

    fn resolve(_projection: &Self::Projection, _query: Self::Query) -> Self::Response {
        0
    }
}

type ErasedRead = Box<
    dyn ReadPort<Projection = CounterProjection, Query = (), Response = usize>,
>;

fn main() {
    let _erased: Option<ErasedRead> = None;
}
