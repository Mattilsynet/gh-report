# COM-0035. Stabilization Ratchet from Signal to Standard

Date: 2026-04-30
Last-reviewed: 2026-05-12
Tier: A
Status: Accepted

## Related

References: COM-0019, COM-0017, COM-0034, GND-0005, GND-0007

## Context

Complex behavior becomes maintainable only after evidence exposes its
shape. COM-0019 captures runtime signal, COM-0017 prefers mechanized
invariants, and COM-0034 routes operational evidence back into ADR
review. Missing is the promotion path that converts repeated signals
into models, enforcement, and boring defaults.

Three options:

1. **Treat each signal locally** — cheap, but recurring patterns never
   become shared constraints.
2. **Mechanize immediately** — fast stabilization, but risks freezing
   noise before the pattern is understood.
3. **Ratchet by evidence** — observe recurrence, model the invariant,
   enforce mechanically, then standardize the default.

Option 3 chosen: it turns uncertainty reduction into an explicit
architecture workflow.

## Decision

Repeated operational, review, and test evidence promotes a decision
from observation to model to enforcement to standard default. Once a
constraint reaches mechanical enforcement, weakening it requires ADR
review.

R1 [6]: Record recurring production incidents, review comments, and
  property-test counterexamples as stabilization candidates in the
  relevant ADR `## Context`
R2 [5]: Convert each accepted stabilization candidate into one
  enforcement mechanism: Rust type, trait bound, compile-fail test,
  lint, CI check, or `adr-fmt` rule
R3 [5]: Promote enforced mechanisms with repeated reuse into named
  workspace defaults in `Cargo.toml`, module templates, or public
  trait APIs
R4 [6]: Link each stabilization candidate to the telemetry field,
  test fixture, or issue URL that exposed the uncertainty
R5 [5]: Reopen the relevant ADR before weakening a ratcheted constraint
  that has reached CI, type-system, or public-API enforcement

## Consequences

- **Pairs with observability.** COM-0019 supplies signal; this ADR
  defines how signal becomes structure rather than dashboard noise.
- **Pairs with mechanization.** COM-0017 provides the enforcement
  ladder; this ADR decides when evidence moves onto that ladder.
- **Reduces variance.** Recurring surprises become named constraints,
  shrinking the state space developers must reason about.
- **Preserves reversibility.** Constraints can be weakened, but only
  by reopening the evidence and trade-off record.
