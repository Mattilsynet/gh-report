# CHE-0015. Error Type Per Command, Not Per Aggregate

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: CHE-0005

## Context

When a command is rejected by an aggregate, the caller receives a
domain error. Two design approaches exist for typing this error:

1. **Error on `Aggregate`** — `Aggregate::Error` as a single
   associated type. All commands on the aggregate share one error
   enum. Simple but coarse: error enums accumulate variants from
   every command, and callers matching on errors from a specific
   command must handle irrelevant variants.
2. **Error on `HandleCommand<C>`** — each command handler defines its
   own error type. `HandleCommand<CreateOrder>::Error` can be
   `CreateOrderError`, while `HandleCommand<ShipOrder>::Error` can
   be `ShipOrderError`. Precise but more types.

## Decision

`HandleCommand<C: Command>` defines `type Error: Error + Send + Sync`
as an associated type on the handler, not on the aggregate.

The error type flows losslessly through the dispatch chain:
- `HandleCommand<C>::Error` → `DispatchError<E>` → `DispatchResult<A, C>`
- Callers receive `Result<..., DispatchError<ShipOrderError>>` — they
  can match on `Rejected(ShipOrderError::NotConfirmed)` without
  downcasting.

R1 [5]: Define the error type as an associated type on
  HandleCommand<C>, not on the Aggregate trait
R2 [5]: Preserve the domain error type losslessly through the
  dispatch chain without Box<dyn Error> downcasting

## Consequences

- Each command-aggregate pair has an independent error type. An aggregate handling 5 commands has up to 5 error types.
- `DispatchError<E>` preserves the domain error type through the gateway and bus. No `Box<dyn Error>` downcasting.
- Callers know at compile time which domain errors are possible for a given command. HTTP adapters can map specific variants to status codes.
- Aggregates wanting a shared error type for all commands can use the same type for all `HandleCommand` impls.
