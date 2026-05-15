# FLO-0012. U-Curve Tradeoff Discipline

Date: 2026-05-03
Last-reviewed: 2026-05-03
Tier: B
Status: Accepted
Parent-cross-domain: COM-0011 — u-curve-tradeoff-discipline is the FLO-tier expression of COM-0011's universal directive that trade-offs are made first-class, by requiring numeric-parameter ADRs to argue both sides of the cost curve rather than treating a corner solution as the natural answer

## Related

References: COM-0011, COM-0034

## Context

Reinertsen E6: most flow trade-offs are U-curves — cost rises on both sides of
an optimum. B11 (batch size), Q9 (queue depth), and the retry-count U-curve all
follow this structure. E16 requires comparing marginal cost to marginal value,
not levels; a corner solution minimises one cost while the opposing cost grows
unchecked. ADRs that fix a numeric parameter — batch size, queue depth, retry
count, timeout, cadence period, utilisation target — frequently argue only one
side and treat the other as "obviously avoided." This produces corner solutions
that optimise the argued dimension and pay an unmeasured cost on the unargumented
one. This ADR makes both-sides-of-the-curve reasoning a structural obligation for
parametric ADRs.

## Decision

Parametric ADRs — those that set a numeric trade-off parameter — are required to
reason about both sides of the cost curve before declaring a value.

R1 [5]: ADRs that set a numeric trade-off parameter — batch size, queue depth,
  retry count, timeout, cadence period, utilisation target, or similar — identify
  both the cost that rises with an increase and the cost that rises with a
  decrease, not only one side, so the chosen value is defensibly an interior
  optimum rather than a corner solution.

R2 [5]: When a U-curve cannot be quantified, the ADR documents the qualitative
  argument — which costs dominate at low values, which costs dominate at high
  values — and the observation that would shift the optimum, so future re-tuning
  has a starting point rather than a blank slate.

R3 [8]: Parameter values that have not been re-evaluated within the staleness
  window set by COM-0034 are flagged for review; an unrevisited numeric parameter
  is presumed wrong, not presumed right, because the cost structure that justified
  the original value may have shifted.

## Consequences

Becomes easier: tuning conversations have a shared structure; corner-solution
arguments are surfaced explicitly at authorship time; future re-tuning has
documented starting hypotheses rather than reverse-engineering the original
intent.

Becomes harder: parametric ADRs grow a both-sides-of-the-curve obligation;
quick "the obvious answer is X" framings require explicit cost-of-the-other-side
rationale before they are accepted.

Risks and migration: existing parametric ADRs — CHE-0035 batch boundary,
PAR-0014 queue size, CHE-0046 retry count, FLO-0008 utilisation target, FLO-0002
cadence period — acquire amendment debt in the form of a structured U-curve
argument retroactively applied. Mitigation: this ADR is forward-binding; existing
ADRs amend at their next staleness review per COM-0034 rather than requiring
immediate retrofit.
