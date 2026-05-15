# AFM-0001. Single Source of Truth Architecture for ADR Governance

Date: 2026-04-27
Last-reviewed: 2026-05-02
Tier: S
Status: Accepted

## Related

Root: AFM-0001

## Context

ADR governance faces a consistency problem: rules in prose drift
from rules enforced by tooling. A governance document says "use
kebab-case slugs" but nothing prevents violations. A template file
shows structure but cannot enforce cross-file link integrity,
lifecycle consistency, or naming conventions. Three approaches
exist: prose-only governance (drift inevitable), template-only
governance (cannot enforce cross-file invariants), and code-as-SSOT
where the validation tool is the specification.

## Decision

Adopt a layered SSOT architecture where the `adr-fmt` binary is
the authoritative specification for all invariant ADR rules.

R1 [5]: Bind all invariant rules to the `adr-fmt` binary: template
  structure, naming, relationships, lifecycle states, link integrity,
  and section ordering — at least one check per invariant class
R2 [5]: Set `adr-fmt.toml` as the owner of configurable aspects:
  domain definitions, crate mappings, stale directory path, and rule
  parameter overrides
R3 [5]: Emit `--guidelines` output as the generated reference
  document combining code invariants from `adr-fmt` and
  `adr-fmt.toml` configuration into a single authoritative output
R4 [5]: Express every enforceable rule as a validation check in
  `adr-fmt`; constraints that resist validation belong in the
  judgment layer
R5 [5]: Record rationale and judgment guidance in ADR Context and
  Consequences sections, filing them there rather than in standalone
  governance documents — exactly one location per judgment item
R6 [5]: Classify a rule as invariant when violating it produces an
  inconsistent corpus regardless of project context and map it as
  configurable otherwise — apply this classification to every new rule

## Consequences

No rule exists in prose alone — if it cannot be a validation check,
it belongs in the judgment layer (R4). The `--guidelines` flag
eliminates a separate writing guide that would drift. Adding
invariant rules requires code changes, a rule catalog entry, and
an AFM-domain ADR. The architecture is self-referential: `adr-fmt`
validates its own domain's ADRs. Per R5, rationale and judgment
that previously lived in the standalone `GOVERNANCE.md` document
have migrated to ADR narrative sections — specifically:
parent-edge tree mechanics → AFM-0020; cross-domain overlap
resolution → COM-0038; tier-classification rationale → AFM-0011
Context; domain-prefix rationale → AFM-0008 Context; quick-start
and contributor onboarding → `adr-fmt --guidelines` setup output.
The standalone governance document is retired in favor of the
discovered `adr-fmt.toml` marker plus per-ADR narrative prose.
