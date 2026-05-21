//! CHE-0012 R1: `Aggregate` requires `Default` (zero state, no constructor
//! arguments). A type that does not implement `Default` must be rejected
//! at compile-time when it tries to `impl Aggregate`.

use cherry_pit_core::Aggregate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
enum NoopEvent {
    Tick,
}

impl cherry_pit_core::DomainEvent for NoopEvent {
    fn event_type(&self) -> &'static str {
        "noop.tick"
    }
}

// Foo lacks `#[derive(Default)]` — the `impl Aggregate` below must be
// rejected because `Aggregate: Default + Send + Sync + 'static`.
struct Foo;

impl Aggregate for Foo {
    type Event = NoopEvent;
    fn apply(&mut self, _event: &NoopEvent) {}
}

fn main() {}
