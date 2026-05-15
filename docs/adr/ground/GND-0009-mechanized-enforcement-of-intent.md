# GND-0009. Intent Without Mechanism Drifts

Date: 2026-05-01
Last-reviewed: 2026-05-02
Tier: S
Status: Accepted

## Related

References: GND-0005

## Context

GND-0001 names an alignment gap between stated intent and acted
behaviour. A directive closes that gap only to the strength of the
mechanism that enforces it. Bungay observes that mission command
survives contact only when intent is carried by mechanisms — drills,
checklists, standing orders — not by recall under load. Cherry-pit
generalises the lesson to engineering: human-enforced rules degrade
because reviewers tire, while machine-enforced rules catch every
violation on every commit.

Three options:

1. **Trust restatement** — write the intent clearly and rely on
   readers to apply it. Drift accumulates between reviews.
2. **Audit periodically** — sample for compliance after the fact.
   Detection lags violation; the gap stays open.
3. **Bind intent to the strongest feasible mechanism** — type system,
   static check, automated gate; human review is the fallback.

Option 3 chosen: it is the only option whose closure rate matches
the rate of change.

## Decision

When an invariant declared by a directive can be enforced by machine
— type system, compiler, linter, automated test, or merge gate —
human enforcement of that invariant is a defect. Each directive
selects the strongest feasible mechanism on the enforcement-strength
ladder: type system → static check → automated gate → human review →
documentation. Weaker mechanisms are accepted only when stronger
ones are demonstrably infeasible, and the infeasibility is recorded.

R1 [3]: Bind each enforceable invariant to the strongest feasible
  rung of the enforcement ladder — type system, static check, CI
  gate, human review, documentation — and name the chosen rung in
  the ADR, because intent unbacked by mechanism degrades at the
  rate reviewers tire
R2 [3]: File any human-enforced but mechanizable invariant as a
  defect ticket with a `mechanization-gap` label; target type
  system, lint, or CI as the steady-state enforcement in place
  of human review
R3 [4]: Record in the ADR's Decision section the enforcement
  mechanism that surfaces violations earliest in the lifecycle
  — compile, then test, then CI, then review, then docs — because
  earlier signal narrows the GND-0001 effects gap
R4 [4]: Record the chosen enforcement mechanism and the reason
  every stronger rung was rejected in the ADR's Decision section,
  so the ladder choice is reviewable rather than implicit and
  survives turnover

## Consequences

- **Subsumes COM-0017.** COM-0017 becomes the COM-domain instantiation
  of GND-0009, mirroring the GND-0005 / COM-0019 pattern: the universal
  claim lives in GND, the software-design expression in COM.
- **Sharpens GND-0005.** GND-0005 requires that every directive name
  *some* observation mechanism; GND-0009 requires that the named
  mechanism be the strongest feasible one. Observability is necessary;
  mechanization is the strength dimension.
- **Retroactive cost.** Existing directives whose enforcement is "code
  review" become candidates for amendment as stronger mechanisms
  emerge. Migration is per-tier, S first, consistent with GND-0005.
- **Observation mechanism (per GND-0005 R1).** Review-gate refusal:
  ADRs that establish an invariant without naming an enforcement
  mechanism, or that select a weaker mechanism without recording why
  a stronger one was rejected, are blocked at merge.
