# GND-0004. Deviation From a Directive Is Permitted and Must Be Reported

Date: 2026-04-30
Last-reviewed: 2026-04-30
Tier: S
Status: Accepted

## Related

References: GND-0002

## Context

Truppenführung (1933): *"a failure to act or a delay is a more
serious fault than making a mistake in the choice of means."* Under
GND-0001 uncertainty, the executor at the point of contact has
information the directive's issuer did not. Forbidding deviation
forces the executor to choose between violating intent and obeying
form — both fail. Permitting silent deviation lets the system drift
out of alignment.

Three options:

1. **Forbid deviation** — directives are absolute. Executor stalls
   or violates intent to preserve form.
2. **Permit silent deviation** — executor adjusts without report.
   Drift accumulates undetected.
3. **Permit deviation; require report** — executor adjusts when
   intent demands it, then surfaces the deviation so the directive
   itself can be revised.

Option 3 chosen: it pairs autonomy at the point of contact with
the feedback signal needed to keep directives current.

## Decision

Deviation from a directive in service of its higher intent is
permitted. The deviation is reported through a defined channel so
the directive's issuer can confirm, refine, or revise.

R1 [3]: Permit deviation from a directive's literal text whenever
  the executor preserves the directive's stated intent or its
  parent intent; intent outranks form under GND-0001 uncertainty,
  and deviation in service of intent is a feature of mission
  command, not a violation
R2 [3]: Record every deviation in a monitored channel — git commit
  message, ADR amendment proposal, or incident report — citing the
  directive ID `PREFIX-NNNN:RN`, the action taken, and the intent
  served, because silent deviation accumulates as drift
R3 [3]: When three or more deviations are logged against one
  directive, open a GND-0007 lifecycle review for it and either
  reaffirm, refine, or supersede; do not let the count grow past
  three without action

## Consequences

- **Pairs with GND-0002.** Intent-stated directives make principled
  deviation possible; this ADR establishes the reporting half.
- **Cost.** The reporting channel is overhead. Without it, deviation
  is silent and the corpus drifts. With it, deviations become
  signal: the directive learns from its own friction.
- **Replaces blame with evidence.** Deviation is not failure; it is
  data about where the directive's scope no longer matches reality.
- **Connects to lifecycle.** The supersession trigger in GND-0007
  consumes the deviation log as one of its inputs.
- **Observation mechanism (per GND-0005).** Review-gate checklist:
  deviations are recorded in commit messages or ADR amendments
  citing the directive ID; periodic corpus review correlates
  recurring deviations against directives and feeds the GND-0007
  lifecycle trigger.
