# FLO-0002. Cadence as a Flow-Control Contract

Date: 2026-05-03
Last-reviewed: 2026-05-03
Tier: B
Status: Accepted
Parent-cross-domain: PAR-0015 — cadence-as-flow-control-contract is the FLO-tier expression of PAR-0015's universal directive that consumer delivery is acknowledged on a stable schedule, by raising periodic resynchronisation from per-message correctness to a system-level architectural contract that bounds variance accumulation

## Related

References: PAR-0015, FLO-0004

## Context

Reinertsen F5 holds that periodic resynchronisation limits variance accumulation;
F7 that cadence makes waiting time predictable; F8 that cadence enables small
batches by amortising transaction cost; F14 that nested cadences must be harmonic
multiples or they beat against each other; F6 that capacity margin is the
structural cost of sustaining cadence. PAR-0015 governs per-message ack semantics
but does not raise periodic resynchronisation to a system-level concern. The
corpus default is aperiodic processing; no ADR names cadence as a first-class
runtime element with a declared period, harmonic nesting constraint, and reserved
capacity margin.

## Decision

Long-running consumers declare a cadence period; nested cadences are harmonic;
each loop reserves a capacity margin; cadence drift is observable.

R1 [5]: Long-running runtime processes that consume queued work declare a cadence
  period as a configuration value; aperiodic processing is permitted only when
  justified in Context, naming the variance-absorption mechanism that replaces
  cadence.
R2 [5]: Nested cadences are integer harmonic multiples of a base period — a
  ten-second loop nested inside a sixty-second loop is permitted; a seven-second
  loop nested inside ten is not — so adjacent cadences do not beat against each
  other.
R3 [8]: Each cadenced loop reserves a capacity margin of approximately twenty-five
  percent or more of its period, so transient load excursions do not collapse the
  cadence into aperiodic catch-up mode (per Reinertsen F6).
R4 [8]: Drift between expected and actual cadence is a first-class telemetry
  signal exposed through the queue-telemetry contract (FLO-0004), so cadence
  collapse is observable before downstream variance amplifies.

## Consequences

+ becomes easier: cadenced checkpointing, cadenced replay, and batch-aware
  publishers all gain a structural parent. Variance bounds on waiting time become
  declarable.
− becomes harder: every long-running consumer must commit to a period and reserve
  margin; aperiodic implementations need explicit justification.
risks/migration: cadence collapse under load is a classic failure mode. R3's
  margin requirement is the primary mitigation; R4's observability is the
  secondary mitigation. Existing aperiodic consumers acquire a cadence at next
  touch or document why they remain aperiodic.
