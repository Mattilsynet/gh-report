# cherry-pit-core

Foundational traits for cherry-pit: aggregates, commands, events, ports.

Every infrastructure port (`EventStore`, `EventBus`, `CommandBus`,
`CommandGateway`) is bound to a single aggregate type via associated types.
The compiler enforces end-to-end type safety from command dispatch through
event persistence and publication.

## Public API

### Domain traits (synchronous)

- **`Aggregate`** — consistency boundary, reconstructed from events via `apply`
- **`HandleCommand<C>`** — compile-time verified command→aggregate handling
- **`DomainEvent`** — immutable facts (`Serialize + DeserializeOwned + Clone + Send + Sync`)
- **`Command`** — intent marker (`Send + Sync + 'static`)
- **`Policy`** — reacts to events by producing commands
- **`Projection`** — folds events into read-optimized views

### Port traits (async, RPITIT — no dynamic dispatch)

- **`CommandGateway`** — primary entry point with retry/middleware
- **`CommandBus`** — load → handle → persist → publish lifecycle
- **`EventStore`** — persistence of aggregate event streams
- **`HashChainedEventStore`**, **`ListableEventStore`**, **`PurgeableEventStore`**, **`SingleWriterEventStore`** — capability-extension store ports
- **`EventBus`** — fan-out of persisted events

### Types

- **`AggregateId`** — `NonZeroU64` stream partition key
- **`EventEnvelope<E>`** — metadata wrapper (UUID v7 id, sequence, timestamp, correlation)
- **`CorrelationContext`** — explicit correlation/causation propagation
- **`ErrorCategory`** — `Retryable` | `Terminal` retry guidance
- **`DispatchError<E>`**, **`StoreError`**, **`BusError`**, **`EnvelopeError`** — typed errors
- **`CreateResult`**, **`StoreCreateResult`** — create-or-open outcomes
- **`DomainKey`**, **`JobSource`**, **`JobOutcome<R>`** — work-queue domain value types

## Minimal usage

```rust
use cherry_pit_core::{Aggregate, HandleCommand, Command, DomainEvent};
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CounterEvent { Incremented }

impl DomainEvent for CounterEvent {
    fn event_type(&self) -> &'static str { "counter.incremented" }
}

#[derive(Default)]
struct Counter { count: u32 }

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
```

## Dependencies

Only `serde`, `uuid`, and `jiff` — no async runtime, no framework deps.

## Status

Implemented. All domain and port traits are exported and stable for v0.1.

Part of the [cherry-pit](../../README.md) workspace.
