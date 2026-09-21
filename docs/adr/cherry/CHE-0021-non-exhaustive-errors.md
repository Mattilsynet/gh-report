# CHE-0021. Closed Error Enums and Typed Categories

Date: 2026-04-25
Last-reviewed: 2026-09-21 - amended - reconcile open-enum prose with settled closed policy and RST-0006 scope
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

Public error enums in `cherry-pit-core` are closed. This supersedes the original open-enum semver decision; the historical motivation above remains context. The following list records the original surface, not an exhaustive inventory of today's variants:

- `DispatchError<E>` — enum with `Rejected`, `AggregateNotFound`,
  `ConcurrencyConflict`, `Infrastructure` variants.
- `StoreError` — enum with `ConcurrencyConflict`, `Infrastructure`,
  `StoreLocked`, and `CorruptData` variants. (`StoreLocked` added by
  CHE-0053:R13.)
- `BusError` — struct wrapping `Box<dyn Error>`. `#[non_exhaustive]`
  prevents external pattern matching on the struct fields.
- `ErrorCategory` — enum of typed categories for distributed retry decisions;
  uncertainty must remain distinct from a safe retry decision.

Adding a variant requires downstream review and the applicable release-version policy; no blanket minor-version compatibility is promised. This enum policy does not require removing a struct's `#[non_exhaustive]` attribute.

R1 [5]: Public error enums in cherry-pit-core MUST NOT carry `#[non_exhaustive]`. Struct field extensibility is a separate contract.
R2 [5]: Error-variant additions require explicit downstream adaptation under the release policy; beta flexibility is not a claim of minor-version compatibility.
R3 [5]: Expose ErrorCategory on DispatchError, StoreError,
  BusError, and EnvelopeError so callers distinguish retryable
  failures from terminal failures without exhaustive matching

## Consequences

- Downstream callers can exhaustively match closed enums and must adapt when their variant set changes.
- Distributed callers should branch on `error.category()` before
  deciding retry, dead-letter, compensation, or operator escalation.
- Within `cherry-pit-core`, exhaustive matching is still allowed.
- `BusError` is a newtype with a private field; `#[non_exhaustive]` adds the constraint that external code cannot destructure it in patterns.
- The original implementation used manual `Display` and `Error` implementations (CHE-0027); current derive choices do not change the closed-enum decision or source-chain obligations.

RST-0006 mechanically checks only its literal public thiserror-derived enum
predicate. That checker is not proof of universal `Error` trait coverage or
the broader policy for manually implemented errors; those remain reviewed.
