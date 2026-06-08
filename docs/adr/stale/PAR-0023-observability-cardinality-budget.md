# PAR-0023. Observability Cardinality Budget

Date: 2026-04-29
Last-reviewed: 2026-04-29
Tier: B
Status: Accepted

## Related

References: PAR-0004, GND-0005

## Context

OpenTelemetry and Prometheus both fail catastrophically under
unbounded label cardinality — backend storage, query latency, and bill
all scale with distinct label-value combinations. Pardosa exposes
several natural identifiers: domain_id (one per fiber, unbounded),
event_id (strictly monotonic, unbounded over time), and correlation_id
(per-request, unbounded). Without an explicit policy, developers will
add these as metric labels and break the observability backend.

Honeycomb, OneUptime, and the OpenTelemetry .NET best-practices guide
all converge on the same rule: high-cardinality identifiers belong on
spans and structured events, never on metric labels. Spans are sampled
and stored as traces; metrics are aggregated.

## Decision

Pardosa enforces a fixed cardinality budget. Bounded enums and writer
identity may appear as both span attributes and metric labels.
Unbounded identifiers (domain_id, event_id, correlation_id) appear
only as span attributes. A linter scans pardosa code for forbidden
metric-label uses on every CI run.

R1 [6]: Emit pardosa metrics with labels drawn only from the bounded
  set stream, generation, action, result, fiber_state, and writer_id
R2 [6]: Record domain_id, event_id, and correlation_id as span
  attributes via tracing::field rather than as metric labels
R3 [6]: Run a CI lint that scans pardosa source for metric macro
  invocations and fails the build when any forbidden label name
  appears
R4 [12]: Budget each pardosa metric at no more than five hundred
  active label combinations measured at the metrics backend
R5 [5]: Wrap user-controlled payloads in a Redacted<T> type whose
  Debug and Display implementations emit three asterisks instead of
  the inner value

## Consequences

Observability backends remain operable at scale. Trace and metric
roles are separated cleanly — operators learn to start in metrics and
drill into traces for specific requests. Trade-offs: developers cannot
slice metrics by domain or correlation without using exemplars or
trace queries; this matches industry practice but is an unfamiliar
model for some. The CI lint adds maintenance — its allow-list must
stay current with new metrics.
