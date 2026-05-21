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
