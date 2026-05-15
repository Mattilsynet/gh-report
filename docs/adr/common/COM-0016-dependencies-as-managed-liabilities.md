# COM-0016. Dependencies as Managed Liabilities

Date: 2026-04-27
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: COM-0001, RST-0004

## Context

Every external dependency imports complexity with a unique risk profile: external maintainers control breaking changes, transitive dependencies inherit security risks without explicit selection, each dependency expands the attack surface, and compile times scale with count. COM-0001 establishes that complexity requires justification; dependencies are a category where costs are invisible at adoption but compound over time. This is distinct from COM-0012 (knowledge flow direction) — COM-0016 governs whether to take a dependency at all.

Cherry-pit already practices dependency minimization: `default-features = false`, workspace-level declarations, and committed `Cargo.lock`.

## Decision

Every external dependency is a liability that must be justified
against the complexity budget. Dependencies are not free — they
trade development convenience for long-term maintenance cost,
security exposure, and coupling to external change schedules.

R1 [5]: Before adding a dependency, demonstrate the functionality
  cannot be reasonably implemented in-house or sourced from the
  standard library
R2 [5]: Disable default features and enable only the feature flags
  the project actually uses
R3 [5]: Prefer the standard library when it provides adequate
  functionality, even if not optimal, due to stronger stability
  guarantees and zero transitive cost
R4 [6]: Evaluate the full cost of a dependency — transitive count,
  maintenance activity, security record, license compatibility,
  and MSRV compatibility
R5 [6]: Audit and prune dependencies periodically as the standard
  library evolves and requirements change

## Consequences

New dependencies require explicit PR justification; reviewers cite COM-0016 to challenge additions. The `default-features = false` pattern is validated as standard posture. Workspace-level declarations (`[workspace.dependencies]`) provide a single inventory of external liabilities. Tension with velocity is resolved per COM-0001: strategic investment now reduces long-term maintenance cost. Transitive dependency awareness via `cargo tree` and auditing tools becomes part of the evaluation process.
