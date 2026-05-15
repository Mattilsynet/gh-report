# CHE-0047. Operational Recovery Runbooks

Date: 2026-04-29
Last-reviewed: 2026-04-29
Tier: B
Status: Accepted

## Related

References: CHE-0042, GND-0001, GND-0004, GND-0007

## Context

Event-sourced systems recover by replaying durable facts, but operators still need safe procedures when the durable substrate contains orphan temp files, corrupt streams, stale locks, dead letters, or failed migrations. Ad hoc repair tools are more dangerous than explicit runbooks because they can bypass invariants. Three options were evaluated: manual operator judgment, hidden automatic repair, or documented quarantine-and-repair workflows. Option 3 preserves safety and auditability.

## Decision

Operational recovery is a first-class architecture concern. Recovery paths quarantine first, repair second, and resume only after validation succeeds.

R1 [5]: MsgpackFileStore recovery removes orphaned .msgpack.tmp files before create or append mutates aggregate streams
R2 [5]: EventStore implementations classify malformed bytes, aggregate_id mismatches, and sequence gaps as StoreError::CorruptData
R3 [5]: Corrupt EventEnvelope streams are quarantined before repair tools rewrite or replace persisted data
R4 [5]: Dead-letter repair records preserve event_id, aggregate_id, sequence, correlation_id, causation_id, error category, and operator action
R5 [5]: Stale lock recovery documents filesystem, process identity, and ownership evidence before deleting lock sentinels or forcing failover
R6 [5]: Migration recovery records durable phase, source stream, target stream, last copied sequence, and cleanup ownership before resuming

## Consequences

Recovery becomes auditable rather than heroic. The safe default is to stop serving the affected stream and preserve evidence. Repair tooling must re-run stream validation before making recovered data visible.
