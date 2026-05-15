# COM-0009. Consistency as Complexity Reducer

Date: 2026-04-26
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: COM-0001

## Context

Ousterhout (Ch. 17) identifies consistency as one of the most powerful tools for reducing cognitive complexity. When similar things are done similarly, developers leverage knowledge across contexts without re-reading code. Consistency spans naming, coding patterns, design patterns, and invariants. The cost of inconsistency is invisible but cumulative.

Cherry-pit applies consistency through uniform vocabulary (`create`, `load`, `append`, `apply`, `handle`), infallible `apply()` on every aggregate (CHE-0009), error-per-command types (CHE-0015), and store-owned envelope construction (CHE-0016).

## Decision

When a pattern is established, apply it uniformly. Inconsistency is
permitted only when the difference reflects a genuine semantic
distinction, not convenience or historical accident.

R1 [5]: Establish canonical names for recurring concepts and use them
  everywhere; synonyms are inconsistencies
R2 [5]: When a new module solves a problem an existing module has
  solved, use the same structural approach; deviation requires
  justification that the case is genuinely different
R3 [6]: Document patterns in ADRs and enforce mechanically where
  possible through trait signatures, compile-fail tests, or lints
R4 [5]: Inconsistency in a PR requires the author to explain why the
  difference is semantically necessary
R5 [5]: When a pattern changes, update all instances; partial
  migration creates the worst form of inconsistency

## Consequences

Trait method naming follows a consistent vocabulary; new traits must follow it or justify deviation. The infallibility invariant (CHE-0009) eliminates "which aggregates can fail during apply?" questions — learn once, apply everywhere. Error-per-command (CHE-0015) is applied uniformly. The ADR system itself is a consistency tool: same template, same vocabulary, same linter. In distributed systems, consistency across crate boundaries is critical because developers reason about interactions between components they cannot see running simultaneously. Consistency creates intentional inertia — changing established patterns requires updating all instances, raising the bar for high-tier changes while allowing local variation at lower tiers.
