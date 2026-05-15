# COM-0019. Design for Observability at Every Abstraction Layer

Date: 2026-04-28
Last-reviewed: 2026-04-29
Tier: A
Status: Accepted
Parent-cross-domain: GND-0005 — observability-as-design-constraint is the COM-tier expression of GND-0005's universal directive that intent must be observable in operation

## Related

References: GND-0005, COM-0001

## Context

COM-0003 pulls errors down, COM-0005 defines errors out of existence,
COM-0007 hides details — collectively creating systems simple to use
but impossible to debug. Ousterhout is silent on operational complexity.
Sridharan (Distributed Systems Observability, 2018) establishes
observability as a design constraint, not post-implementation addition.

1. **Design-time observability** — mandate telemetry alongside interface
   design, compensating for complexity-hiding ADRs.
2. **Post-implementation** — add logging after code works. Inconsistent.
3. **No guidance** — status quo. Silent failure modes.

Option 1 chosen: compensates for deliberate opacity of COM-0003/0005/0007.

## Decision

Treat observability instrumentation as a design-time decision made
alongside the module interface, not added after implementation.

R1 [6]: Every module that absorbs, retries, or masks an error emits
  a structured trace span or metric at the decision point
R2 [6]: State transitions in stateful components (Fiber, Dragline,
  Aggregate) include tracing context sufficient to reconstruct the
  transition sequence from logs alone
R3 [4]: Observability instrumentation is reviewed alongside the trait
  interface during design, documented in interface comments
R4 [6]: Correlation identifiers from CorrelationContext flow through
  all operation boundaries for distributed trace reconstruction
R5 [6]: Telemetry fields that may contain user data use redacted
  wrapper types or explicit allowlists before logging or tracing
R6 [6]: High-cardinality telemetry labels such as aggregate_id,
  event_id, correlation_id, and causation_id stay in traces or logs
  rather than unbounded metric dimensions
R7 [6]: Retry loops in CommandGateway, EventBus, EventStore, and
  Pardosa consumers emit attempt count, terminal category, and
  correlation_id in structured telemetry

## Consequences

- **Compensates complexity-hiding.** COM-0003 (pull down) and COM-0005
  (define out) hide failures from callers; observability ensures failures
  remain visible to operators.
- **Design cost.** Every new module must consider telemetry at design
  time, adding to the COM-0001 complexity budget.
- **Runtime overhead.** Structured tracing adds measurable (though
  typically <1%) latency. Acceptable per CHE-0001 P4.
- **Distributed debugging.** Correlation propagation enables end-to-end
  trace reconstruction across NATS publish, store operations, and
  consumer processing.
Redaction and cardinality limits keep telemetry from becoming a confidentiality leak or denial-of-service vector. Correlation propagation plus retry telemetry gives operators enough context to reconstruct failures without exposing domain payloads.
