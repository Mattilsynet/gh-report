//! Test that the README usage example compiles and passes (S1-7).

#[test]
fn readme_usage_example() {
    use cherry_pit_core::{Aggregate, Command, DomainEvent, HandleCommand};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum CounterEvent {
        Incremented,
    }

    impl DomainEvent for CounterEvent {
        fn event_type(&self) -> &'static str {
            "counter.incremented"
        }
    }

    // CHE-0064:R2 — hand-rolled Encode (no derive) per PAR-0024:R5.
    impl pardosa_encoding::Encode for CounterEvent {
        fn encode(&self, out: &mut Vec<u8>) {
            match self {
                CounterEvent::Incremented => out.push(0u8),
            }
        }
    }

    #[derive(Default)]
    struct Counter {
        count: u32,
    }

    impl Aggregate for Counter {
        type Event = CounterEvent;
        fn apply(&mut self, event: &CounterEvent) {
            match event {
                CounterEvent::Incremented => self.count += 1,
            }
        }
    }

    struct Increment;
    impl Command for Increment {}

    impl HandleCommand<Increment> for Counter {
        type Error = std::convert::Infallible;
        fn handle(&self, _cmd: Increment) -> Result<Vec<CounterEvent>, Self::Error> {
            Ok(vec![CounterEvent::Incremented])
        }
    }

    let mut agg = Counter::default();
    let events = agg.handle(Increment).unwrap();
    agg.apply(&events[0]);
    assert_eq!(agg.count, 1);
}
