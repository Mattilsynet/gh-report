# FLO-0009. Progressive Throttling and Differential Service Classes

Date: 2026-05-03
Last-reviewed: 2026-07-13
Tier: B
Status: Accepted
Parent-cross-domain: SEC-0003 — progressive-throttling-and-service-classes is the FLO-tier expression of SEC-0003:R3's universal directive that backpressure mechanisms exist at every ingestion point to shed load, by replacing the binary open/closed circuit with a continuous admission gradient and adding service-class differentiation so high-cost-of-delay streams retain priority access under saturation

## Related

References: SEC-0003

## Context

Reinertsen W17 — admission slows continuously as the queue approaches its limit, not as a binary gate; W18 — service classes give differential quality so high-priority streams are throttled last; D19 — when response time matters, measure response time per class. PAR-0014's circuit breaker is binary: either closed (admit all) or open (refuse all). Under sustained near-limit pressure the binary form alternates between the two states, producing oscillation. A continuous gradient and per-stream service classes — a high-cost-of-delay stream throttles later than a low-cost-of-delay stream — make admission economically rational under saturation rather than discontinuous.

## Decision

R1 [5]: Admission throttling is a continuous function of queue fill — typically linear or exponential between a low-water mark and the hard limit — rather than a binary gate that flips at the limit alone, so admission rate degrades smoothly under sustained near-limit pressure.

R2 [5]: Streams declare a service class on registration; under saturation, low-class streams are throttled before high-class streams, so cost-of-delay-weighted urgency (per FLO-0001's economic surface) is preserved across the admission gradient.

R3 [6]: Service class is observable in queue telemetry — admission rate per class, drop rate per class — so operators can audit fairness and detect class-mix shifts before they become incidents.

## Consequences

- (+) becomes easier: graceful degradation under sustained load; per-stream economic prioritisation; observability of fairness and class drift.
- (−) becomes harder: every stream acquires a class declaration; PAR-0014's binary-state machine grows a gradient layer.
- risks/migration: a poorly-shaped gradient still oscillates if the smoothing window is too short — controller stability is the implementation concern. Migration: existing PAR-0014 deployments default every stream to a single class until classes are explicitly assigned, preserving current behaviour.
