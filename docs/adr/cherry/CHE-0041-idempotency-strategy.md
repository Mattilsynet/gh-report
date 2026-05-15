# CHE-0041. Idempotency Strategy

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: CHE-0008, CHE-0002, CHE-0017, CHE-0033, CHE-0038, GND-0001, GND-0005

## Context

Cherry-pit has two idempotency requirements enforced by convention only: command handling (`Ok(vec![])` for duplicate detection per CHE-0008) and policy reactions (same event twice must produce same outputs per CHE-0017). Neither has framework-level enforcement. CHE-0002 says "make illegal states unrepresentable," but idempotency is a behavioral invariant the type system cannot structurally enforce. Three layers matter: command dispatch (user retries), policy reactions (at-least-once delivery), and event persistence (optimistic concurrency via `expected_sequence`). UUID v7 `event_id` (CHE-0033) provides a natural deduplication key.

## Decision

Convention-based idempotency with infrastructure-level support
planned for the `CommandBus`.

R1 [5]: Aggregates are the authority on duplicate detection using
  state rebuilt from events
R2 [5]: Policy::react must be a pure function where same
  EventEnvelope input produces the same Vec<Output>
R3 [5]: Optimistic concurrency via expected_sequence prevents
  duplicate appends at the store level
R4 [5]: Commands entering from retried external boundaries carry a
  stable domain idempotency key in their command payload
R5 [5]: The `IdempotencyKey` carrier type lives in cherry-pit-core;
  only `pub fn from_header_value(&str) -> Option<Self>` is public,
  while `new_unchecked` is `pub(crate)` so cherry-pit-web cannot
  synthesize a key — the never-synthesize invariant (CHE-0046:R3,
  CHE-0049:R6) becomes type-system-enforced across the crate
  boundary, preserving the core dep budget (CHE-0029:R4)

### Command-level idempotency

Aggregates are the authority on whether a command is a duplicate.
Commands entering from retried external boundaries (HTTP APIs,
webhooks, message queues, scheduled jobs) carry a stable domain
idempotency key in the command payload. In-process commands that are
not retried across a boundary may omit this key.

The aggregate's state (rebuilt from events) contains enough
information to detect duplicates. Examples:

- `CreateOrder` on an already-created aggregate → `handle` inspects
  state, returns `Err(AlreadyCreated)` or `Ok(vec![])`.
- `ConfirmPayment { payment_id }` when `payment_id` is already
  recorded → `handle` returns `Ok(vec![])`.
- `CreateOrder { request_id, customer_id, ... }` retried by an HTTP
  client → `handle` records `request_id` or a natural unique key and
  returns the original effect or `Ok(vec![])` instead of creating a
  second aggregate.

This approach is correct because the aggregate is the single source
of truth for its own state. External deduplication tables can go
stale; the aggregate cannot.

### Policy-level idempotency

`react` must be a pure function: same `EventEnvelope` input
produces the same `Vec<Output>`. At-least-once delivery is safe
when:

1. `react` is pure (same input → same output), AND
2. Downstream command handling is idempotent (duplicate commands
   produce zero events).

Together, replaying an event through a policy and re-dispatching
the resulting commands produces no additional state changes. The
system converges to the same state regardless of delivery count.

No deduplication table or processed-event tracking is required at
the policy level if both conditions hold.

### Store-level idempotency

Optimistic concurrency (`expected_sequence` on `append`) prevents
duplicate appends. If two concurrent attempts to append after the
same load race, one succeeds and the other receives
`StoreError::ConcurrencyConflict`. The caller retries with a fresh
load.

UUID v7 `event_id` uniqueness (CHE-0033) prevents exact-duplicate
events: two events produced by different `handle` calls will always
have different `event_id` values. This is not deduplication — it is
identity.

### Future: CommandBus idempotency key cache

When the `CommandBus` is implemented, it may add optional result-cache
support for commands entering from external sources (webhooks, message
queues). Design sketch:

```rust
// Optional idempotency key on CommandGateway
fn send_idempotent<C>(
    &self,
    id: AggregateId,
    cmd: C,
    idempotency_key: IdempotencyKey,
    context: CorrelationContext,
) -> impl Future<Output = DispatchResult<Self::Aggregate, C>> + Send
where ...;
```

The bus would check a deduplication store before dispatching. If the
key has been seen, it returns the original result without re-executing
the command. This is infrastructure-level optimization layered on top
of the domain-level idempotency key, not a replacement for aggregate
duplicate detection.

This is deferred until `CommandBus` is built and the external
command ingestion pattern is concrete.

## Consequences

- **Type-system boundary acknowledged.** Idempotency is behavioral; the compiler cannot verify it. Convention + documentation is the pragmatic answer.
- **Aggregate is the idempotency authority.** Retried external commands carry stable idempotency keys, and the aggregate's state remains the authoritative deduplication state.
- **Policy purity is the key invariant.** If `react` is pure AND downstream handling is idempotent, at-least-once delivery is safe without policy-level deduplication. Testing should verify purity (CHE-0038).
- **`event_id` (UUID v7) is a natural deduplication key** for external systems receiving events.
- **No automatic retry** — `Command` has no `Clone` (CHE-0014), so callers must reconstruct and re-dispatch.
