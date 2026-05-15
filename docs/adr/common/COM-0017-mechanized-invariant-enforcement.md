# COM-0017. Mechanized Invariant Enforcement

Date: 2026-04-27
Last-reviewed: 2026-05-01
Tier: B
Status: Accepted
Parent-cross-domain: GND-0009 — mechanized-invariant-enforcement is the COM-tier expression of GND-0009's universal directive that intent without mechanism drifts

## Related

References: GND-0009, COM-0009, GND-0005

## Context

COM-0009 rule 3 frames mechanical enforcement as a fallback from convention. In practice, cherry-pit inverts this: mechanical enforcement is the *primary* strategy. Human-enforced rules degrade — reviewers are inconsistent and fatigued. Machine-enforced rules are deterministic, catching every violation on every commit.

Cherry-pit's enforcement hierarchy: type system (`AggregateId(NonZeroU64)`), compiler lints (`#[non_exhaustive]` per CHE-0021), static analysis (`clippy::pedantic` per CHE-0026), compile-fail tests (`trybuild` per CHE-0028), custom tooling (`adr-fmt`), and code review as fallback for invariants not yet mechanized.

## Decision

When an invariant can be checked by machine — compiler, linter,
formatter, type system, or test harness — it must be. Human
enforcement through code review is the fallback for rules that
cannot yet be mechanized, not the primary strategy.

R1 [5]: Prefer compile-time constraints over runtime checks;
  a type-level constraint that prevents invalid states is stronger
  than a runtime assertion that detects them
R2 [5]: Prefer a linter rule over a reviewer convention; when a
  recurring review comment identifies a mechanical pattern,
  investigate whether a lint can enforce it
R3 [6]: CI gates that block merge on violation are more reliable
  than local conventions; mechanical enforcement in CI guarantees
  no violation reaches the main branch
R4 [5]: Every ADR that establishes an invariant must state its
  enforcement mechanism — type system, lint, CI check, compile-fail
  test, or code review
R5 [6]: Enforcement escalation ladder from strongest to weakest:
  type system, compiler error, compiler lint, CI gate, code review,
  documentation — choose the strongest feasible mechanism

## Consequences

ADRs establishing rules without enforcement mechanisms are incomplete under COM-0017. Compile-fail tests (CHE-0028) sit between "compiler lint" and "CI gate" on the escalation ladder. Clippy pedantic (CHE-0026) is an enforcement strategy. New invariants must identify their enforcement mechanism — "we'll catch it in review" signals a mechanization opportunity. For distributed invariants, property-based tests and golden-file comparisons (CHE-0038) extend mechanization into runtime. Taste and judgment resist mechanization, so code review remains valid for subjective invariants. COM-0017 is the COM-domain instantiation of GND-0009, applying the enforcement-strength ladder to software-design invariants.
