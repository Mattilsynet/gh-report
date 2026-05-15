# CHE-0024. Event Delivery Model

Date: 2026-04-25
Last-reviewed: 2026-04-29
Tier: C
Status: Accepted

## Related

References: CHE-0004, COM-0025, GND-0001, GND-0005

## Context

Event delivery systems offer three guarantee levels: at-most-once (fire and forget, risk of lost events), at-least-once (ACK + retry, risk of duplicates), and exactly-once (distributed transaction, high complexity). `EventBus` defines `publish()` but no `subscribe()`. No code wires policies or projections to the bus. The `CommandBus` (unbuilt) owns the persist-then-publish lifecycle. Cherry-pit's position: the `EventStore` provides durable persistence, the `EventBus` provides best-effort notification, and consumers catch up by replaying from the store — effectively at-least-once at the system level.

## Decision

Persist-then-publish with non-fatal delivery:

R1 [7]: Persist events before publishing; publication failure is
  non-fatal because events are safely stored
R2 [7]: No subscribe method on the EventBus port trait; subscription
  is implementation-specific
R3 [7]: Consumers that miss a publication catch up by replaying from
  EventStore::load using their durable checkpoint
R4 [8]: Consumer checkpoints record aggregate_id, last sequence, and
  handler identity after side effects complete successfully
R5 [8]: Failed policy outputs are routed to a dead-letter workflow
  containing event_id, correlation_id, causation_id, and error category

1. **CommandBus calls `EventBus::publish()`** after successful
   persistence. Events are the source of truth once stored — publish
   is notification, not commit.
2. **Publication failure is non-fatal** — events are safely stored.
   Tracking-style processors catch up by replaying from the store.
3. **Checkpointed catch-up** — a tracking consumer records its handler
   identity and the last processed `(aggregate_id, sequence)`. On
   restart it replays from the store and skips already processed events.
4. **No `subscribe` on the port trait** — subscription is inherently
    implementation-specific (in-process channels vs NATS subjects vs
    polling).
5. **Dead-letter policy outputs** — a policy-triggered command that
   cannot be dispatched after bounded retry is written to an operational
   dead-letter workflow, not retried forever or silently dropped.
6. **`cherry-pit-agent` wires the graph** — the planned composition layer
    registers policies and projections with concrete `EventBus`
    implementations. Manual wiring until `cherry-pit-agent` exists.
7. **In-process delivery is synchronous within `publish`** — the bus
    calls each registered handler before returning.

## Consequences

The `EventBus` remains notification-only; delivery safety comes from persisted events plus checkpointed replay. Policy/projection authors must make handlers idempotent. The wiring layer must provide checkpoint storage and dead-letter visibility.
