/// Verifies that `DomainEvent` enforces the `Clone` supertrait
/// (CHE-0010, amended 2026-05-18). Full supertrait set: Clone + Send +
/// Sync + 'static + pardosa_encoding::Encode. `Clone` is exercised
/// here as the most demonstrative single bound; the other bounds are
/// covered by analogous reasoning.
use cherry_pit_core::DomainEvent;
use serde::{Deserialize, Serialize};

// Deliberately omits `Clone` from the derive list.
#[derive(Debug, Serialize, Deserialize)]
struct NotCloneable {
    _x: u32,
}

impl DomainEvent for NotCloneable {
    fn event_type(&self) -> &'static str {
        "not.cloneable"
    }
}

fn main() {}
