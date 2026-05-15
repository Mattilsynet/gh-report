# FLO-0003. Dynamic Batch Sizing for the Transport Batch

Date: 2026-05-03
Last-reviewed: 2026-05-03
Tier: B
Status: Accepted
Parent-cross-domain: CHE-0035 — dynamic-batch-sizing is the FLO-tier expression of CHE-0035's universal directive that concurrency boundaries are two-level, by adapting the transport batch within those static boundaries to current downstream economics rather than holding it constant

## Related

References: CHE-0035, FLO-0001, FLO-0004

## Context

Reinertsen B16 establishes that the transport batch — the unit moved between processes — dominates downstream latency. In an event-sourced runtime the publish batch is the canonical transport batch. CHE-0035 fixes the two-level concurrency boundary statically; nothing prescribes how the batch within that boundary adapts. B22 and W19 require batch size to track current economics — load, CoD-weighted urgency, and downstream queue depth. This ADR introduces an adaptation policy that keeps CHE-0035's static boundary intact while making the within-boundary batch a runtime variable.

## Decision

The publish batch size adapts at runtime within per-stream configurable bounds; adaptation is driven by observed downstream queue depth and cost-of-delay-weighted urgency; both minimum and maximum bounds are mandatory; and the adaptation rate is damped to prevent controller oscillation.

R1 [6]: The publish batch size is a runtime variable rather than a compile-time constant; bounds are configurable per stream and exposed through the same surface that exposes queue telemetry (FLO-0004).
R2 [6]: Batch size adaptation responds to observed downstream queue depth and cost-of-delay-weighted urgency (FLO-0001), so a saturated downstream consumer naturally throttles upstream batching without operator intervention.
R3 [5]: Batch size has explicit minimum and maximum bounds — the minimum protects against per-batch overhead death-spiral (Reinertsen B5); the maximum protects cycle time under variance (Reinertsen B8). Neither bound is omittable.
R4 [8]: Adaptation rate is bounded so the controller cannot hunt — successive batch-size changes are damped by a configurable smoothing factor that prevents oscillation under noisy queue-depth signals.

## Consequences

Becomes easier: per-stream batch policies, backpressure-aware publishers, and CoD-weighted batch ordering all gain a structural parent. CHE-0035's static boundary stays intact.

Becomes harder: every publisher acquires a controller; min/max bounds become per-stream configuration that operators must tune.

Risks and migration: a poorly-damped controller hunts under load — R4's smoothing-factor requirement is the primary mitigation. Migration path: existing static publishers acquire min=max bounds initially (no adaptation) and graduate to adaptive bounds during their next touch.
