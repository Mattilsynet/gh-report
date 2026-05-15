# FLO-0011. Beneficial Variability and the 50% Failure Rate Heuristic

Date: 2026-05-03
Last-reviewed: 2026-05-03
Tier: A
Status: Accepted
Parent-cross-domain: COM-0005 — beneficial-variability-as-bounded-carve-out is the FLO-tier expression of COM-0005's universal directive that errors be defined out of existence, by carving out a narrow subsystem class — explicitly-tagged experiments — where high failure rates are the optimal information-generation strategy and the deterministic instinct is suspended within the carve-out alone

## Related

References: COM-0005

## Context

Reinertsen V1 holds that variability can create economic value when payoffs are
asymmetric. V2 identifies the condition: failure is cheap, success is valuable,
so high failure rates maximise expected return. V4 places the information-
generation peak for a typical experiment near fifty percent failure — above that
threshold each additional experiment yields diminishing signal. V15 adds that
iteration speed beats defect rate when the iteration is exploratory: the cost
of a slow, clean experiment exceeds the cost of a fast, noisy one. COM-0005
(define errors out of existence) and PAR-0022 (deterministic simulation) push
the corpus toward zero failure as the universal goal. For experiment-shaped
subsystems — A/B paths, exploratory consumers, simulation harnesses, calibration
probes — that goal is wrong: maximising pass rate suppresses the information
yield the experiment exists to produce. This ADR carves a narrow, named
exception.

## Decision

Experiment-shaped subsystems are a bounded carve-out from COM-0005. Within the
carve-out, high failure rates are acknowledged as the optimal information-
generation strategy; outside it, COM-0005 remains authoritative and the
deterministic instinct is unrestricted.

R1 [4]: Subsystems explicitly tagged as experiment-shaped MAY tolerate failure
  rates of thirty percent or higher when the payoff is asymmetric (failure cheap,
  success valuable) and information yield exceeds pass-rate yield by an explicit
  margin documented in the subsystem's own ADR.
R2 [5]: Experiment-shaped subsystems measure information yield — decisions
  enabled per experiment, hypotheses falsified per cycle, parameters narrowed per
  run — rather than pass rate, so the metric matches the economic intent of the
  carve-out.
R3 [5]: The boundary between experiment-shaped subsystems and production-shaped
  subsystems is explicit in code and configuration — for example a feature-flag
  tier or an explicit module suffix — so the carve-out cannot leak into production
  paths where COM-0005 remains authoritative.

## Consequences

Becomes easier: legitimate exploratory work — A/B harnesses, calibration probes,
simulation runs — escapes the deterministic-instinct overhead. Information-yield
metrics become first-class within the carve-out.

Becomes harder: experiment-shaped subsystems acquire an explicit boundary marker;
reviewers must verify the carve-out is not invoked outside experimental scope.

Risks and migration: the carve-out leaks into production if the boundary marker
is absent or stale — R3 is the primary mitigation. This ADR introduces no new
tag yet; the first concrete experiment-shaped subsystem (likely a future CHE or
PAR ADR) defines the marker form. Until then, the carve-out is permissive but
unused.
