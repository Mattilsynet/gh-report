# GND-0005. Every Directive Must Define Its Observability

Date: 2026-04-30
Last-reviewed: 2026-05-02
Tier: S
Status: Accepted

## Related

References: GND-0001

## Context

The effects gap in GND-0001 — the difference between intended and
actual outcome — closes only when the system can observe the result.
A directive whose violation cannot be detected is a wish: it cannot
fail loudly, it cannot trigger review, it cannot compound into evidence
of drift. Observation is the precondition for every feedback loop in
GND-0006, GND-0007, and GND-0008.

Three options:

1. **Trust compliance** — assume directives are followed. Drift is
   discovered through unrelated incidents, late.
2. **Audit periodically** — schedule manual review. Episodic; delay
   between violation and detection is unbounded.
3. **Embed observability in the directive itself** — every directive
   names the mechanism — test, lint, type constraint, metric, review
   gate — by which a violation surfaces.

Option 3 chosen: it is the only option that closes the effects gap
on a timescale comparable to the rate of change.

## Decision

Every directive declares at least one mechanism by which a violation
of its intent becomes detectable. The mechanism is named in the
directive itself; absence of a mechanism is a defect, not an
optional enhancement.

R1 [3]: Name in each ADR's Decision or Consequences section at
  least one observation mechanism — automated test, `adr-fmt` lint
  code (T/L/N/S/F), type invariant, runtime metric, telemetry
  assertion, or review-gate checklist — that surfaces violations
  of the directive's intent
R2 [3]: Choose observation mechanisms that surface violations
  before integration — type constraints, `cargo` lints, compile-
  fail tests — over runtime-only mechanisms; pre-integration signal
  closes the GND-0001 effects gap on a tighter loop than post-
  deploy telemetry
R3 [3]: Admit an ADR to `Status: Accepted` only after the Decision
  or Consequences section names at least one observation
  mechanism — an unobservable directive is a wish, not a
  commitment that the corpus can act on

## Consequences

- **Strengthens the effects-gap response.** Without observability
  a directive cannot participate in the feedback loops downstream.
- **Retroactive cost.** Existing ADRs across COM, RST, CHE, PAR,
  GEN, SEC, and AFM that name no observation mechanism become
  candidates for amendment. Migration is per-tier, S first.
- **Aligns with COM-0019.** COM-0019 implements observability for
  software components; GND-0005 is the principle COM-0019 instantiates
  at the software-design level. COM-0019's structural parent re-points
  to GND-0005 on adoption.
- **Constrains the template.** Future evolutions of TEMPLATE.md
  may add a dedicated Observability field; out of scope here, in
  scope for AFM.
