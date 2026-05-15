# CHE-0021. Non-Exhaustive Error Types for Semver Safety

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: CHE-0015

## Context

Cherry-pit is a framework. Downstream users `match` on error types
returned by the infrastructure ports. If a new error variant is added
to a public enum in a minor version, all downstream `match` statements
break — this is a semver-breaking change.

Rust provides `#[non_exhaustive]` to address this: it forces external
callers to include a wildcard arm (`_ =>`) in their `match`, allowing
new variants to be added without breaking compilation.

## Decision

All public error types in `cherry-pit-core` are `#[non_exhaustive]`:

- `DispatchError<E>` — enum with `Rejected`, `AggregateNotFound`,
  `ConcurrencyConflict`, `Infrastructure` variants.
- `StoreError` — enum with `ConcurrencyConflict`, `Infrastructure`,
  `StoreLocked`, and `CorruptData` variants. (`StoreLocked` added by
  CHE-0043.)
- `BusError` — struct wrapping `Box<dyn Error>`. `#[non_exhaustive]`
  prevents external pattern matching on the struct fields.
- `ErrorCategory` — enum with stable `Retryable` and `Terminal`
  categories for distributed retry decisions.

New variants (e.g., `RateLimited`, `Timeout`, `SchemaVersionMismatch`)
can be added in minor versions.

R1 [5]: All public error types in cherry-pit-core are
  #[non_exhaustive]
R2 [5]: New error variants may be added in minor versions without
  breaking downstream callers
R3 [5]: Expose ErrorCategory on DispatchError, StoreError,
  BusError, and EnvelopeError so callers distinguish retryable
  failures from terminal failures without exhaustive matching

## Consequences

- Downstream callers must use wildcard arms in `match` on `DispatchError` and `StoreError`. Slightly less ergonomic but enables safe API evolution.
- Distributed callers should branch on `error.category()` before
  deciding retry, dead-letter, compensation, or operator escalation.
- Within `cherry-pit-core`, exhaustive matching is still allowed.
- `BusError` is a newtype with a private field; `#[non_exhaustive]` adds the constraint that external code cannot destructure it in patterns.
- `#[derive(Debug)]` is the only derive on error types. Manual `Display` and `Error` impls are used instead of `thiserror` (CHE-0027).
