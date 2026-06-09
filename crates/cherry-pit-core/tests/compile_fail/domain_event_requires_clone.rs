/// Verifies that `DomainEvent` enforces the `Clone` supertrait
/// (CHE-0010, amended). Full supertrait set: Clone + Send + Sync +
/// 'static + serde::Serialize + serde::de::DeserializeOwned. `Clone`
/// is exercised here as the most demonstrative single bound; the
/// other bounds are covered by analogous reasoning.
use cherry_pit_core::DomainEvent;
use serde::{Deserialize, Serialize};

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
