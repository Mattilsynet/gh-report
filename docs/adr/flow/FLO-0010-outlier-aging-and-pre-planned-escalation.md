# FLO-0010. Outlier Aging and Pre-Planned Escalation

Date: 2026-05-03
Last-reviewed: 2026-05-03
Tier: C
Status: Accepted
Parent-cross-domain: CHE-0046 — outlier-aging-and-escalation is the FLO-tier expression of CHE-0046's universal directive that retries are bounded by timeout and cancellation, by adding per-item aging across attempts so the system detects items that survive multiple retries and escalates them through pre-declared paths rather than absorbing them silently

## Related

References: CHE-0046, FLO-0004

## Context

Reinertsen W15 — most cost in a queue lives in the long tail; watch outliers, not averages. W16 — outliers need pre-planned escalation paths, not ad-hoc heroics at the moment of breach. FF17 — controlled excursions stay within the control range only when the response is rehearsed. CHE-0046 governs per-attempt timeout and cancellation; this ADR addresses *aging across attempts* — items whose total time-in-queue (wall-clock, not per-retry) crosses a cost-of-delay-weighted threshold even though each individual retry timed out cleanly. Without aging detection, the system is blind to systemic stuckness even while individual operations look healthy.

## Decision

R1 [8]: Each queue tracks per-item age — wall-clock time since first arrival, not per-attempt time — and emits a structured event through the queue-telemetry contract (FLO-0004) when age crosses a cost-of-delay-derived threshold.

R2 [8]: Escalation responses are pre-declared per stream — rebid priority, page on-call, divert to operator queue — rather than improvised at the moment of breach; aging fires the named response, not an ad-hoc handler.

R3 [8]: Aging metrics are first-class dimensions on the queue-telemetry contract (FLO-0004) — count of items aged, age-of-oldest-aged-item, rate-of-aging — so operator dashboards make systemic stuckness visible before escalation fires.

## Consequences

- + becomes easier: detection of systemic-stuckness patterns invisible to per-attempt monitoring; pre-rehearsed escalation paths; explicit ownership of long-tail outcomes.
- − becomes harder: every queue acquires an aging clock and a per-stream escalation policy; CHE-0046 retry semantics now interact with a wall-clock budget.
- risks/migration: a wrongly-tuned threshold pages on-call too aggressively — R1 derives the threshold from CoD (FLO-0001), not from a flat number, so it scales with stream economic value. Migration: existing CHE-0046 retry deployments acquire an infinite default age threshold (no aging) and graduate per-stream as policies are written.
