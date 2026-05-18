/// Verifies that `Aggregate` exposes no `fn id()` method —
/// identity lives on `EventEnvelope.aggregate_id`, not the aggregate
/// itself (CHE-0020 R1).
use cherry_pit_core::{Aggregate, DomainEvent};
use serde::{Deserialize, Serialize};

#[derive(Default)]
struct MyAggregate;

#[derive(Debug, Clone, Serialize, Deserialize)]
enum MyEvent {
    Happened,
}

impl DomainEvent for MyEvent {
    fn event_type(&self) -> &'static str {
        "happened"
    }
}

impl pardosa_encoding::Encode for MyEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            MyEvent::Happened => out.push(0u8),
        }
    }
}

impl Aggregate for MyAggregate {
    type Event = MyEvent;
    fn apply(&mut self, _event: &Self::Event) {}
}

fn main() {
    let agg = MyAggregate;
    // This must fail: Aggregate exposes no `id()` method.
    let _ = agg.id();
}
