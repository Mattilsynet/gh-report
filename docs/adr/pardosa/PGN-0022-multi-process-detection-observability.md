# PGN-0022. Multi-Process Detection Observability

Date: 2026-07-15
Last-reviewed: 2026-07-17
Tier: B
Status: Accepted
Crates: pardosa, pardosa-nats

## Related

References: PGN-0016, CHE-0048, COM-0019

## Context

PGN-0016 R7 allows transient overlapping Cloud Run writers because stale
writers fail at append, not because overlap is impossible; that overlap is
currently invisible to operators. CHE-0048 scopes its projection atomicity
to a single process and defers multi-process coordination to a future ADR.
This unblocks only the detection/observability signal for that overlap,
leaving the writer model and CHE-0048's single-process scope untouched.

## Decision

Govern the observability semantics for the multi-process OCC-fence overlap
signal only; unblock this one detection slice and leave the writer model,
including CHE-0048's single-process projection scope, unchanged.

R1 [6]: Emit a structured trace span or metric at the point PGN-0016 R7's
  fence detects an overlapping writer, keyed by the PGN-0016 R8 owner id,
  never by hostname or `K_REVISION`.
R2 [6]: Classify every emitted detection event by the PGN-0016 R9 typed
  `ConcurrencyConflict` category; never re-derive classification from NATS
  error-code text at the observability layer.
R3 [5]: Scope this ADR to the detection/observability signal only; do not
  redesign the writer model and do not reverse CHE-0048's single-process
  projection binding (its R1/R2/R7 stay intact).
R4 [6]: Keep high-cardinality identifiers — owner id, event id, correlation
  id, sequence — on trace spans and logs only, never as metric labels, per
  COM-0019 R6.
R5 [6]: Propagate the active correlation context through every detection
  emission per COM-0019 R4, so an operator can reconstruct the overlap
  episode across process boundaries.
R6 [6]: Treat a detected overlap as a surfacing mechanism, not a silent
  event, per GND-0010 R7: absence of a detection emission for a real
  overlap is itself a defect to report.

## Consequences

+ becomes easier: operators can observe transient Cloud Run writer overlap
  as a first-class, correlation-bearing signal instead of inferring it from
  raw NATS conflict codes.
− becomes harder: every fence-detection code path must carry owner id and
  correlation context through to the emission point, adding instrumentation
  surface beyond the bare conflict-reject logic.
risks/migration: no pardosa/pardosa-nats code changes ship with this ADR;
  a dedicated multi-process writer-coordination ADR (referenced by CHE-0048)
  remains separate future work this ADR does not attempt.
