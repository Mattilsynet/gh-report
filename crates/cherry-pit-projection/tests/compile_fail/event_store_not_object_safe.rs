//! Verifies that `EventStore` is NOT dyn-compatible (object-safe).
//!
//! `cherry_pit_core::EventStore` declares `type Event: DomainEvent` and
//! returns `impl Future` from its methods, both of which preclude
//! dyn-compatibility. This locks the single-event-type-per-store
//! invariant from CHE-0005:R1 — every concrete store is monomorphic
//! over exactly one `DomainEvent` impl, never erased through a
//! `Box<dyn EventStore>` indirection.
//!
//! The associated `Event` is specified concretely so the rejection is
//! the dyn-incompatibility of the `impl Future` returns (E0038), not a
//! missing-associated-type diagnostic (E0191).
//!
//! If this test ever passes-compile, the trait has become dyn-safe and
//! the one-event-type-per-store contract is silently broken.
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
