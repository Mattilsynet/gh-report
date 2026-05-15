# CHE-0023. Aggregate Lifecycle States

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: CHE-0009

## Context

Aggregates have lifecycle phases — created, active, terminated.
CHE-0013 establishes: "No delete/archive/tombstone at the
infrastructure level. Termination is modeled as a domain event."
No framework-level lifecycle trait or termination guard exists.

## Decision

Termination is a domain concern, not a framework concern.

R1 [5]: No framework lifecycle trait or termination guard exists;
  lifecycle semantics are owned by the domain
R2 [5]: Model termination as a domain event and reject post-termination
  commands via domain errors in handle

1. **No framework lifecycle trait** — no `is_terminated()` on
   `Aggregate`, no lifecycle state enum in cherry-pit-core.
2. **No `DispatchError::Terminated`** — the framework does not
   distinguish "aggregate terminated" from "command rejected."
3. **Termination as domain event** — aggregates emit events like
   `OrderClosed` and track terminated state via `apply`.
4. **Command rejection via domain error** — `handle` inspects
   terminated state and returns a domain error (e.g.,
   `Err(OrderError::AlreadyClosed)`).
5. **Terminated streams remain loadable** — event history is
   immutable and replayable regardless of lifecycle state
   (consistent with CHE-0009, infallible apply).

## Consequences

- Framework stays minimal. Users own lifecycle semantics entirely.
- No infrastructure-level guard against post-termination commands —
  enforcement is in `handle`, not in the bus or gateway.
- Different aggregates can define different lifecycle semantics
  (soft delete, archival, reopening) without framework constraints.
- HTTP adapters map domain termination errors to appropriate status
  codes (e.g., `AlreadyClosed` → 409 Conflict).
