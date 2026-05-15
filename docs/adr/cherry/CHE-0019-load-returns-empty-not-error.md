# CHE-0019. Load Returns Empty Vec, Not NotFound Error

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: CHE-0013

## Context

When `EventStore::load` is called for an aggregate that has never been
created, two semantics are possible:

1. **Return error** — `StoreError::NotFound` or
   `StoreError::AggregateNotFound`. The store treats a missing
   aggregate as an error condition. The bus and gateway must handle
   this error.
2. **Return empty vec** — the store sees an empty event stream. A
   never-created aggregate is indistinguishable from one with zero
   events. The bus decides whether "empty stream" is an error.

## Decision

`EventStore::load` returns `Ok(Vec::new())` for unknown aggregates.
`StoreError` has no `NotFound` variant. `AggregateNotFound` is a
`DispatchError` variant at the `CommandBus` level — the bus maps
"empty load before dispatch" to `AggregateNotFound`.

This creates a clean semantic boundary:

- **Store layer:** an aggregate is an event stream. An unknown
  aggregate is an empty stream. Not an error.
- **Bus layer:** dispatching a command to an empty stream means the
  aggregate was never created. This IS an error — the bus returns
  `DispatchError::AggregateNotFound`.

R1 [5]: EventStore::load returns Ok(Vec::new()) for unknown
  aggregates, not an error
R2 [5]: AggregateNotFound is a DispatchError variant at the
  CommandBus level, not a StoreError variant
R3 [5]: EventStore::append to a never-created aggregate returns
  StoreError::Infrastructure

`EventStore::append` to a never-created aggregate (file does not
exist) is an error: `StoreError::Infrastructure`. The aggregate must
be created via `create()` first. This prevents bypassing the `create`
path, which assigns the aggregate ID and guarantees ≥1 event.

## Consequences

- The store trait is simpler — `load` has only two outcomes: events or infrastructure error.
- The bus owns the semantic decision: empty stream before dispatch → `DispatchError::AggregateNotFound`.
- HTTP adapters handle `NotFound` at the `DispatchError` level (→ 404), not the store level.
- `append` to a never-created aggregate returns `StoreError::Infrastructure`, enforcing the `create`→`append` lifecycle.
