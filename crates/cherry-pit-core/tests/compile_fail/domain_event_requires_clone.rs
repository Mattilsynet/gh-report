use cherry_pit_core::DomainEvent;

#[derive(Debug)]
struct NotCloneable {
    _x: u32,
}

impl DomainEvent for NotCloneable {
    fn event_type(&self) -> &'static str {
        "not.cloneable"
    }
}

fn main() {}
