# CHE-0040. Saga and Compensation Patterns (Deliberate Deferral)

Date: 2026-04-25
Last-reviewed: 2026-04-29
Tier: B
Status: Accepted

## Related

References: CHE-0039, CHE-0017, CHE-0024, COM-0025

## Context

`Policy` (CHE-0017) reacts to a single event — the choreography pattern. Orchestration-style sagas use a coordinator tracking step completion and issuing compensation on failure. Cherry-pit's `Policy` has no saga coordinator, step tracking, or automatic compensation. Three considerations argue for deferral: `CommandBus` is unbuilt, `cherry-pit-agent` is unbuilt (saga design without composition layer is speculative), and `CorrelationContext` (CHE-0039) must be validated first.

## Decision

Deliberate deferral. Saga orchestration is out of scope for
cherry-pit pre-1.0.

R1 [5]: Use Policy::react for choreography-style coordination only;
  no saga orchestrator exists pre-1.0
R2 [5]: Model compensation as domain events reacted to by policies,
  not as automatic framework-level rollback
R3 [5]: Failed policy output commands are recorded as dead-letter
  entries with event_id, output type, error category, and correlation_id
R4 [5]: Compensation commands carry idempotency keys derived from
  the triggering EventEnvelope event_id and policy identity
R5 [5]: Timeout-driven compensation is represented by explicit
  domain timeout events, not hidden framework timers

**What cherry-pit provides today:**

- `Policy::react` for choreography-style coordination.
- `CorrelationContext` (CHE-0039) for grouping related events across
  aggregates.
- Domain-level compensation: aggregates emit failure events (e.g.,
  `PaymentFailed`), and policies react to those events with
  compensating commands (e.g., `CancelOrder`).

**What cherry-pit does not provide:**

- No saga coordinator / process manager type.
- No step tracking or completion state machine.
- No automatic compensation on downstream command failure.
- No saga coordinator / process manager type.
- No step tracking or completion state machine.
- No automatic compensation on downstream command failure.
- No hidden timeout mechanism for steps that never complete.

## Consequences

Framework orchestration stays minimal while failed policy outputs become visible and repairable. Compensation remains domain-owned and idempotent. Revisit when `cherry-pit-agent` exists or multi-step processes cannot decompose into independent policy reactions.
