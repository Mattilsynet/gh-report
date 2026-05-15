# CHE-0013. Create/Send Split in CommandGateway and EventStore

Date: 2026-04-24
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: CHE-0004

## Context

Aggregate lifecycle has two distinct phases:

1. **Creation** — the aggregate doesn't exist yet, no ID is known.
   The store must assign an ID.
2. **Mutation** — the aggregate exists, the caller knows its ID.
   The store appends to the existing stream.

A unified method with `Option<AggregateId>` would be awkward: callers
must remember to pass `None` for creation, the store must branch
internally, and the return type must accommodate both the new ID (for
creation) and just the envelopes (for mutation).

## Decision

The API separates aggregate lifecycle into two distinct operations:

- `CommandGateway::create(cmd)` → creates a new aggregate, store
  assigns the ID. Returns `(AggregateId, Vec<EventEnvelope>)`.
- `CommandGateway::send(id, cmd)` → targets an existing aggregate
  by known ID. Returns `Vec<EventEnvelope>`.

`EventStore` mirrors this with `create(events)` and
`append(id, expected_sequence, events)`.

Asymmetries:
- `create` rejects empty events (an aggregate must have ≥1 event).
- `append` treats empty events as a no-op.
- `append` requires `expected_sequence` for optimistic concurrency.

R1 [5]: Separate aggregate creation (create) from mutation (send) as
  distinct API operations
R2 [5]: create rejects empty events because an aggregate must have at
  least one event
R3 [5]: Model aggregate termination as a domain event, not as an
  infrastructure delete operation

## Consequences

- Aggregate lifecycle states are explicit: "not yet created" vs "already exists."
- `CreateResult` and `DispatchResult` are distinct return types.
- No `delete`/`archive`/`tombstone` at the infrastructure level. Termination is modeled as a domain event (e.g. `OrderClosed`); the aggregate's `apply` tracks terminated state.
- No `load`/`query` on the Gateway — correct CQRS separation. Reads go through projections.
- The naming asymmetry (`send` on Gateway vs `dispatch` on Bus) is intentional: Gateway is external API, Bus is internal mechanism.
- In distributed deployments, `create` idempotency must be handled at a higher level (CHE-0041).
