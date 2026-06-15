# CHE-0077. Explicit Domain Event Scheduler

Date: 2026-06-13
Last-reviewed: 2026-06-15
Tier: B
Status: Accepted

## Related

References: CHE-0040, CHE-0024, CHE-0018, CHE-0048, CHE-0039, CHE-0001

## Context

CHE-0040 requires timeout compensation to appear as explicit domain events.
`gh-report` currently wraps batch work in `tokio::time::timeout` and emits a
failure event on elapsed timeout (`crates/gh-report/src/app/collect.rs:665-700`),
while the daemon loop sleeps between runs
(`crates/gh-report/src/app/daemon.rs:235-255`). The substrate needs the same
minimal rule without introducing a coordinator.

## Decision

Adopt a minimal scheduler primitive whose only scheduled effect is appending
and publishing a caller-defined domain event. P1 correctness wins over timer
ergonomics: a fired timeout must leave an auditable stored fact.

R1 [5]: Treat the scheduled effect as a domain event appended and published through the normal persist-then-publish path

R2 [5]: Keep scheduling infrastructure asynchronous while the emitted event remains plain synchronous domain data

R3 [5]: Require callers to provide the fire instant, target aggregate, event payload, and explicit CorrelationContext

R4 [5]: Represent pending schedules as durable state recoverable by replay or checkpoint, not process-local sleep alone

R5 [5]: Fire each due schedule at most once per recovered pending schedule under the single-writer aggregate boundary

R6 [5]: Forbid hidden callbacks, step coordinators, retry engines, or non-event side effects inside the scheduler

R7 [5]: Keep the v0.1 shape local and statically wired; runtime-pluggable scheduler topology is out of scope

R8 [6]: Expose overdue and failed schedule outcomes as explicit domain events or dead-letter records, never silent logs only

## Consequences

+ becomes easier: consumers can replace ad hoc timeout wrappers with a
  substrate rule that still produces ordinary event history, correlation, and
  replay evidence.

− becomes harder: pending schedules need durable representation and replay
  reconciliation, so a simple sleep loop is insufficient for ratified timeout
  behaviour.

risks/migration: this Accepted ADR does not add a scheduler port or runtime
  implementation. A later implementation ADR must keep the shape Tier B and
  avoid a pluggable coordinator boundary.
