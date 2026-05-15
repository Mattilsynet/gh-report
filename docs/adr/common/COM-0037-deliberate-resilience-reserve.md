# COM-0037. Deliberate Resilience Reserve

Date: 2026-04-30
Last-reviewed: 2026-05-03
Tier: B
Status: Accepted

## Related

References: COM-0019, COM-0013, COM-0025, GND-0001, GND-0004

## Context

Standardization makes the system easier to understand, but excessive
constraint can make it brittle against failure modes outside the
model. COM-0013 keeps design evolutionary, COM-0019 makes behavior
observable, and COM-0025 recognizes distributed failure. The corpus
needs a rule for preserving deliberate adaptation points while still
bounding their cost.

Three options:

1. **Eliminate all variation** — simplest until the first unmodeled
   failure arrives.
2. **Allow ad-hoc escape hatches** — flexible, but unbounded and hard
   to audit.
3. **Reserve explicit variation points** — constrained flexibility
   with ownership, telemetry, and review.

Option 3 chosen: resilient systems need bounded slack, not accidental
looseness.

## Decision

Standardized subsystems preserve explicit variation points for known
classes of uncertainty. Each variation point is owned, bounded,
observable, and reviewed for continued justification.

R1 [5]: Record the preserved variation point for each standardized
  subsystem as a trait, enum variant, configuration key, or operator
  runbook
R2 [6]: Instrument each preserved variation point with structured
  telemetry that shows activation count, caller, and outcome category
R3 [5]: Bound each variation point with resource limits, capability
  checks, or owner approval recorded in code comments
R4 [5]: Review each unused variation point during ADR lifecycle review
  with its current failure-mode justification

## Consequences

- **Balances simplification.** Defaults shrink routine state space;
  resilience reserves preserve capacity for black-swan adaptation.
- **Avoids hidden escape hatches.** Variation becomes named and
  observable rather than scattered through conditionals.
- **Costs attention.** Unused reserves require review and may be
  deleted under COM-0026 when their failure mode no longer justifies
  them.
- **Supports operations.** Runbooks and telemetry show operators when
  the system leaves the standard path.
