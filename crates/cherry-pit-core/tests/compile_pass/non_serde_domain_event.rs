use cherry_pit_core::DomainEvent;

#[derive(Debug, Clone)]
enum LedgerEvent {
    Credited { minor_units: u64 },
}

impl DomainEvent for LedgerEvent {
    fn event_type(&self) -> &'static str {
        match self {
            LedgerEvent::Credited { .. } => "ledger.credited",
        }
    }
}

fn main() {
    let event = LedgerEvent::Credited { minor_units: 1 };
    assert_eq!(event.clone().event_type(), "ledger.credited");
}
