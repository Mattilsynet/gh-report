# FLO-0001. Cost-of-Delay as First-Class Scheduling Input

Date: 2026-05-03
Last-reviewed: 2026-05-03
Tier: A
Status: Accepted
Parent-cross-domain: GND-0008 — cost-of-delay-as-scheduling-input is the FLO-tier expression of GND-0008's universal directive that effort be focused on a named main effort, by providing the per-unit economic ordering rule that operationalises schwerpunkt under load

## Related

References: GND-0008, PAR-0023

## Context

Reinertsen E3 holds that if you quantify only one thing, quantify cost of delay.
F15/F16/F17 give the scheduling rules that follow: shortest job first when delay
costs are homogeneous (SJF), highest cost-of-delay first when durations are
homogeneous (HDCF), and WSJF — cost-of-delay divided by duration — for the
heterogeneous case. FF1 identifies cost-of-delay as the control parameter with
the highest economic influence. The gap: PAR-0014 throttles but does not
prioritise; CHE-0046 retries but does not order; GND-0008 names the main effort
but provides no per-unit ordering rule. Without quantified cost-of-delay every
scheduler degenerates to FIFO under load.

## Decision

Cost-of-delay is made an explicit, queryable property of every queued work item;
schedulers derive execution order from it rather than from arrival time. WSJF
(cost-of-delay ÷ duration estimate) is the canonical ordering rule for the
heterogeneous case.

R1 [5]: Every queued work item carries a cost-of-delay field with units of
  currency-per-time; absence is a structural error, not a default — schedulers
  MUST refuse to admit unfielded items rather than substitute a zero or a guess.
R2 [5]: When job durations are heterogeneous, schedulers order by WSJF —
  cost-of-delay divided by duration estimate — and FIFO is permitted only as the
  explicit fallback when cost-of-delay is uniform across the queue.
R3 [6]: Cost-of-delay values are observable at runtime through the same telemetry
  surface as queue depth and arrival rate (PAR-0023), so operators can audit the
  scheduling decision.
R4 [5]: Local schedulers MAY override global WSJF when local information shows a
  higher economic option, per Reinertsen F18 — local priorities are inherently
  authoritative over global ones.

## Consequences

+ becomes easier: future CHE/PAR scheduling ADRs (priority-queue publish path,
  recovery-replay ordering, per-stream CoD policy) have a structural parent. WSJF
  becomes the canonical ordering rule rather than a per-component reinvention.
− becomes harder: every queue producer must source a cost-of-delay estimate —
  boilerplate cost. Pure-FIFO subsystems must justify the uniformity assumption.
risks/migration: CoD estimation quality dominates outcomes; bad estimates are
  worse than FIFO. Mitigation: R3's observability requirement keeps estimate
  quality auditable. Future work — CHE priority-queue ADR; per-stream CoD policy
  ADR; recovery-replay ordering ADR.
