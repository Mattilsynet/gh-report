# FLO-0004. Queue Telemetry as Measurement Contract

Date: 2026-05-03
Last-reviewed: 2026-05-03
Tier: C
Status: Accepted
Parent-cross-domain: PAR-0023 — queue-telemetry-as-measurement-contract is the FLO-tier expression of PAR-0023's universal directive that observability cardinality is bounded, by naming the queue-specific schema every async-infrastructure component must implement to make Reinertsen's queueing chapter actionable

## Related

References: PAR-0023, COM-0019

## Context

Reinertsen's queueing chapter is unusable without instrumentation — Q9 (queue
size optimisation), Q11 (CFD monitoring), Q12 (Little's: W=L/λ), F3 (visible
congestion), W23 (visible WIP). PAR-0023 governs cardinality budgets but does
not prescribe what to measure on a queue; COM-0019 is principle-level only.
This ADR names the queue-observability contract every runtime queue must
satisfy: depth, arrival rate, departure rate, age-of-oldest, CFD-shaped
time-series. Without it, FLO-0009 (progressive throttling) and FLO-0010
(outlier aging) are unmeasurable.

## Decision

Every runtime queue exposes a fixed telemetry schema; wait-time estimates use
Little's Formula when per-item tracing exceeds PAR-0023's cardinality budget;
CFD data is retained at a rate sufficient for congestion detection; and every
queue telemetry surface is wired to at least one balancing feedback loop.

R1 [7]: Every runtime queue exposes depth, arrival-rate, departure-rate, and
  age-of-oldest-item; metric names are stable cross-component identifiers so
  dashboards and alerts compose across services.
R2 [8]: Wait-time estimates are computed from Little's Formula — queue-size
  divided by processing-rate — rather than measured per-item, when per-item
  tracing exceeds the cardinality budget set by PAR-0023.
R3 [7]: Cumulative-flow-diagram data is retained at a sample rate sufficient
  to detect a doubling of queue depth within one cadence period (per
  FLO-0002), so congestion is visible before saturation.
R4 [8]: Queue telemetry is consumed by at least one balancing feedback loop —
  for example progressive throttling (future FLO-0009) or outlier escalation
  (future FLO-0010) — so the measurement is not vanity but actuated.

## Consequences

+ becomes easier: FLO-0009 (progressive throttling), FLO-0010 (outlier aging),
  and any future queue-aware controller can rely on a uniform metric surface.
− becomes harder: queue implementers carry a fixed instrumentation cost;
  cardinality budgets get tighter.
risks/migration: poorly-bounded label cardinality can blow PAR-0023's budget —
  R1 mandates stable identifiers (low cardinality) to mitigate. Migration:
  existing queue impls add the four metrics during their next touch.
