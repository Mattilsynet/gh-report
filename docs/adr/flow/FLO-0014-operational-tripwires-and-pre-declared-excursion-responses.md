# FLO-0014. Operational Tripwires and Pre-Declared Excursion Responses

Date: 2026-05-03
Last-reviewed: 2026-05-03
Tier: C
Status: Accepted
Parent-cross-domain: COM-0034 — operational-tripwires-and-pre-declared-excursion-responses is the FLO-tier expression of COM-0034's universal directive that decisions be revisited on a cadence, by adding tripwire thresholds whose breach is itself a scheduled revisit event with a pre-declared response rather than an ad-hoc reaction at the moment of breach

## Related

References: COM-0034, FLO-0004

## Context

Reinertsen FF14 establishes that operational systems need tripwires: pre-defined thresholds whose breach signals a controlled excursion. FF15 and FF20 add that the response must be rehearsed and pre-declared, not improvised at breach time. FF17 closes the loop: controlled excursions stay controlled only when the response is named in advance. COM-0034 makes ADR revisits a scheduled event; this ADR extends the same discipline to runtime parameters — a tripwire breach is a scheduled revisit event whose cadence is data-driven rather than calendar-driven, and whose response is pre-declared so the excursion stays inside the control range.

## Decision

R1 [7]: Each governance-relevant runtime parameter — utilization target, queue depth, retry budget, batch size, age threshold — declares a tripwire pair (warning, critical) on the queue-telemetry contract from FLO-0004 so breaches surface through the same observability path as the parameter itself.

R2 [8]: Each tripwire declares a pre-rehearsed response — page on-call, divert traffic, reduce admission, escalate to operator queue — rather than leaving the response to the moment of breach; ad-hoc handlers at breach time are an explicit anti-pattern.

R3 [8]: A breach is logged as a revisit event against the owning ADR per COM-0034 — recurring breaches indicate the ADR's parameter is wrong, not that the system is misbehaving — closing the loop between runtime evidence and decision-record staleness.

## Consequences

Becomes easier: runtime breaches feed back into ADR re-evaluation; on-call response is rehearsed, not improvised; controlled excursions stay controlled.

Becomes harder: every governance-relevant parameter acquires a tripwire pair and a named response; the queue-telemetry contract grows tripwire dimensions.

Risks and migration: too-tight tripwires generate noise that erodes response discipline — R1's warning/critical pair gives operators a graded response. Migration: parameters acquire tripwires opportunistically as their owning ADRs hit COM-0034 staleness review; no retro-active sweep required.
