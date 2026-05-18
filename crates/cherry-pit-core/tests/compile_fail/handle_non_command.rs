/// Verifies that HandleCommand cannot be implemented for a type
/// that does not implement Command.
use cherry_pit_core::{Aggregate, DomainEvent, HandleCommand};
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

// NotACommand does NOT implement Command.
struct NotACommand;

impl HandleCommand<NotACommand> for MyAggregate {
    type Error = std::io::Error;
    fn handle(&self, _cmd: NotACommand) -> Result<Vec<Self::Event>, Self::Error> {
        Ok(vec![])
    }
}

fn main() {}
