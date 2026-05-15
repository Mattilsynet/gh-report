# GND-0001. Information Systems Operate Under Irreducible Uncertainty

Date: 2026-04-30
Last-reviewed: 2026-04-30
Tier: S
Status: Accepted

## Related

Root: GND-0001

## Context

The **knowledge gap** — between plans and outcomes — means designers cannot fully observe system behavior. The **alignment gap** — between plans and actions — means teams cannot perfectly coordinate. The **effects gap** — between actions and outcomes — means even coordinated actions may not produce intended results.

Every information system is built and operated under conditions its designers cannot fully observe and cannot reliably predict. Clausewitz named this *friction*; Bungay (*The Art of Action*, 2011) generalised it to organisational endeavour as the three gaps named above.

Three options for foundational stance:

1. **Pursue certainty** — gather more information, write more detailed
   plans, impose tighter controls. Bungay shows this widens the gaps.
2. **Ignore the gaps** — treat each project as if uncertainty were
   absent. Surfaces as recurring surprise and rework.
3. **Name uncertainty as the design substrate** — treat the three
   gaps as constants every directive must respond to.

Option 3 chosen: the gaps are not a problem to solve but the medium
in which all subsequent decisions occur.

## Decision

Treat irreducible uncertainty as the substrate for all subsequent
GND principles. Every directive in any domain answers to one or more
of the knowledge, alignment, or effects gap.

R1 [2]: Name in every ADR's Context section the knowledge gap,
  alignment gap, or effects gap (per GND-0001) the decision
  addresses, identifying the gap by its short name before stating
  the problem, because unnamed gaps revert to false certainty
R2 [2]: Respond to gap-induced surprise by revising the directive's
  scope, intent, or feedback loop in the ADR text itself; reject
  responses that only add information, instructions, or controls,
  because added detail widens the three GND-0001 gaps
R3 [3]: Build every GND-derived mechanism on directed opportunism —
  high alignment on intent, high autonomy on action — and label the
  mechanism's intent and autonomy boundaries explicitly in its ADR
  Decision section

## Consequences

- **Establishes the GND vocabulary.** Subsequent GND ADRs reference
  this one as the source of "the three gaps." Domain ADRs (COM, RST,
  SEC, CHE) inherit the framing through their GND parents.
- **Reframes existing failures.** Recurring surprise, rework, and
  drift become evidence of gap-handling defects, not of weak
  individual decisions.
- **Constrains tooling.** Tools that pursue certainty (exhaustive
  specs, detailed step-by-step plans) are suspect; tools that close
  feedback loops (tests, telemetry, backbriefing) are favoured.
- **Cost.** Every ADR Context section gains a small framing burden:
  identify the gap. The cost is paid in clarity downstream.
- **Observation mechanism (per GND-0005).** Review-gate checklist:
  every new ADR's Context section is reviewed for explicit naming
  of the knowledge, alignment, or effects gap it addresses; ADRs
  that frame the problem as deterministic without naming a gap are
  refused at acceptance.
