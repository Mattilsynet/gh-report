# CHE-0012. Aggregate Default for Zero-State Construction

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: A
Status: Accepted

## Related

References: CHE-0009

## Context

Every aggregate must have an initial state before any events are applied. The framework constructs this during command handling (`create`) and replay (`dispatch`). Three approaches: `Default` trait bound (blank-slate via standard trait), custom constructor method (adds a trait method), or builder pattern (adds complexity). `Default` is idiomatic Rust, infallible, and `#[derive(Default)]` covers most cases.

## Decision

`Aggregate` requires `Default`. Aggregates start as blank slates.

R1 [4]: Aggregate trait requires Default for zero-state construction
R2 [4]: The Default instance is a construction artifact, not a valid
  domain state, that becomes valid after the first event is applied
R3 [4]: No constructor arguments are permitted on aggregates; initial
  data must arrive via the first command and event

```rust
pub trait Aggregate: Default + Send + Sync + 'static {
    type Event: DomainEvent;
    fn apply(&mut self, event: &Self::Event);
}
```

The `Default` instance represents "no events applied yet." It is not
a valid domain state — it is a construction artifact that becomes
valid after the first event is applied. The `CommandBus::create` flow
is:

1. `A::default()` — blank aggregate
2. `aggregate.handle(cmd)` — pure handler returns events (CHE-0008)
3. Events applied via `aggregate.apply(event)` for each event
4. Events persisted via `EventStore::create` (CHE-0013)

For `dispatch`, the flow adds replay:

1. `A::default()` — blank aggregate
2. `EventStore::load(id)` — load all events (CHE-0037: no snapshots)
3. For each loaded event: `aggregate.apply(event)` — rebuild state
4. `aggregate.handle(cmd)` — handle against rebuilt state
5. New events persisted via `EventStore::append`

## Consequences

- **No constructor arguments** — initial data must arrive via the first command and event.
- **Default is not a valid domain state** — fields may be `None` or zero, but no command is dispatched against a default-only aggregate: `create` always produces ≥1 event (CHE-0013), and `dispatch` loads history first.
- **Testability** — creating specific state for testing: `default()` then apply setup events. Standard event-sourcing test pattern.
- **Infallible construction** — `Default::default()` cannot fail. Combined with infallible `apply` (CHE-0009), reconstruction is guaranteed for any valid event history.
