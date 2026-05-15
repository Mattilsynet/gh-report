# FLO-0008. Operating Below the Congestion Knee

Date: 2026-05-03
Last-reviewed: 2026-05-03
Tier: B
Status: Accepted
Parent-cross-domain: PAR-0014 — operating-below-the-congestion-knee is the FLO-tier expression of PAR-0014's universal directive that backpressure protects writers, by treating capacity headroom as a first-class design parameter rather than letting components run at theoretical peak utilization where queues grow exponentially with small variance spikes

## Related

References: PAR-0014

## Context

Reinertsen's most-cited operational rule draws on four findings: Q3 (capacity utilization increases queue lengths exponentially), Q6 (high utilization amplifies variability), F1 (congestion collapse — above the knee, throughput drops catastrophically as queues saturate), and F2 (peak throughput requires occupancy control, not utilization maximisation). The knee sits near eighty percent utilization on most queueing curves; above it, small variance spikes cause runaway queue growth. PAR-0014 backpressures *after* saturation is detected; this ADR makes the design operating point a parameter the runtime declares before saturation is reached, so headroom is reserved by construction rather than discovered by collapse.

## Decision

R1 [5]: Every capacity-bounded resource — writer, consumer, NATS connection, file handle — declares a target utilization strictly less than one, with the margin justified in Context against the variance amplitude expected at that resource.

R2 [5]: Capacity planning that targets ninety percent or higher sustained utilization is a structural error; transient surge above target is permitted, sustained operation above target is not.

R3 [8]: Observed utilization above target triggers a documented response — alert, scale-out, or shed (per FLO-0005) — within the cadence period set by FLO-0002, so collapse is prevented rather than recovered from.

## Consequences

- (+) becomes easier: capacity dimensioning becomes a stated decision rather than an implicit consequence; alert routing has a target to compare against; scaling decisions have an economic anchor.
- (−) becomes harder: utilization-maximising configurations need explicit justification; cost-of-capacity becomes a first-class line item.
- risks/migration: under-utilised capacity is real cost — R1 requires margin be *justified*, not maximised. Migration: existing components acquire a target field at next config touch; default is the empirical knee from observed load tests.
