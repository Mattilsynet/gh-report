# CHE-0009. Infallible apply() on Aggregate and Projection

Date: 2026-04-24
Last-reviewed: 2026-04-25
Tier: A
Status: Accepted

## Related

References: CHE-0004

## Context

In an event-sourced system, aggregate state is reconstructed by
replaying all historical events through an `apply` method. If `apply`
can return an error:

- Replaying history could fail, making aggregates unloadable.
- The fundamental event-sourcing guarantee (state = f(events))
  collapses.
- Error handling during replay introduces complexity with no clear
  recovery path — the events are already persisted and immutable.

The same reasoning applies to projections, which fold events into
read-optimized views and must be rebuildable from scratch.

## Decision

Both `Aggregate::apply(&mut self, event: &Event)` and
`Projection::apply(&mut self, event: &EventEnvelope<Event>)` return
`()`. They cannot fail. If they encounter a truly unhandleable event
(corrupted data, unknown variant), the only valid response is a panic
— this represents a bug, not a runtime condition.

R1 [4]: Aggregate::apply and Projection::apply return () and cannot
  fail
R2 [4]: Panic is the only error path for truly corrupt or unknown
  event data in apply

## Consequences

- Event replay always succeeds — aggregate state reconstruction is guaranteed.
- Projections can always be rebuilt from scratch.
- Schema evolution creates pressure: new event variants force all `apply` implementations to handle them. Pardosa's planned migration-time pruning addresses this.
- Panic is the only error path for corrupt data. The CommandBus must decide how to handle panics during replay.
- The asymmetry between `Aggregate::apply(&Event)` and `Projection::apply(&EventEnvelope<Event>)` is intentional: projections need metadata (timestamp, aggregate_id) for time-based views.
