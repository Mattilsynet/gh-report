# FLO-0015. Relief Valves and Saturation-Time Degradation Modes

Date: 2026-05-03
Last-reviewed: 2026-05-03
Tier: B
Status: Accepted
Parent-cross-domain: COM-0037 — relief-valves-and-saturation-time-degradation-modes is the FLO-tier expression of COM-0037's universal directive that systems degrade gracefully under load, by naming three concrete relief-valve mechanisms — load shedding by economic value, saturation-time mode switches, and pre-declared sacrifice ordering — that operationalise the policy at the queue substrate

## Related

References: COM-0037, FLO-0001

## Context

Reinertsen FF11 — saturating systems need relief valves: explicit drop paths that fire before the system collapses. FF12 — the difference between graceful and ungraceful degradation is whether the response was designed in advance. FF18 — saturation-time modes are distinct operating regimes, not extreme points on the normal regime; the system *behaves* differently when the relief valve is open. W19 — when shedding load, shed the lowest-economic-value work first, not the most recently arrived. COM-0037 sets the policy at the corpus level; this ADR names the three FLO-tier mechanisms that realise it on the queue substrate.

## Decision

R1 [5]: Saturation-time load shedding follows the cost-of-delay ranking from FLO-0001 — lowest-CoD streams shed first, highest-CoD streams shed last — so the economic surface is preserved through the degradation regime rather than discarded at the moment it matters most.

R2 [6]: Saturation-time operating modes are explicit named regimes — green, yellow, red — with declared admission policies per regime, not a continuous degradation curve, so operators and downstream consumers know which regime is active and adjust correspondingly.

R3 [6]: The transition between regimes is hysteresis-bounded — entry threshold differs from exit threshold by a documented margin — so a system at the boundary does not oscillate between regimes and produce an observability storm.

## Consequences

Becomes easier: saturation behaviour becomes designed rather than emergent; operators have named regimes to reason about; lowest-CoD work absorbs the shed cost rather than highest-CoD.

Becomes harder: every queue subsystem grows three named regimes plus hysteresis margins; admission paths gain regime-conditional logic.

Risks and migration: hysteresis margins set wrongly either oscillate (too narrow) or strand the system in red mode (too wide) — R3 requires the margin be documented so it is tunable. Migration: COM-0037 deployments default to single-regime (green only) until per-stream regime policies are written.
