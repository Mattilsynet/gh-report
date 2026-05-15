# AFM-0020. Parent-Edge Tree Model

Date: 2026-05-01
Last-reviewed: 2026-05-01
Tier: B
Status: Accepted

## Related

References: AFM-0009, AFM-0001

## Context

Every ADR (other than a Root) needs an unambiguous answer to "under
which decision does this one live?" Without that answer, `--context`
cannot scope rules to a crate, `--tree` collapses into a flat list,
and reviewers cannot distinguish decisions a new ADR builds on from
ones it merely cites. This ADR pins the mechanical rule and the
diagnostics surface that enforces it. It absorbs the parent-edge
specification previously held in GOVERNANCE.md §5.

## Decision

Every non-Root ADR's structural parent is the **first** target listed
in its `References:` field. Other forward links — additional
`References:`, `Refines:`, `Supersedes:` — are secondary citations
that contribute argument and history but do not place the ADR in the
tree.

R1 [5]: Treat the first `References:` target as the structural parent;
  ignore additional References, Refines, Supersedes, and reverse verbs
  for parent-edge construction
R2 [5]: Recognize a Root ADR by its `Root:` self-reference; Roots
  have no structural parent and anchor a domain subtree
R3 [5]: Suppress L011 only when `Parent-cross-domain: PREFIX-NNNN —
  reason` matches the first References target exactly; mismatched
  declarations fire L018 and dangling targets fire L019
R4 [5]: Render `--tree` output as a per-domain parent-edge forest
  with `[also: …]` annotations for secondary citations and a per-domain
  orphan section for unreachable ADRs
R5 [5]: Emit advisory waypoint warning L012 when the structural parent
  is Draft or Proposed; emit L017 when the parent is Superseded by
  another ADR (L017 takes precedence over L012)

### Reference ordering

Specialized parents come first, foundational citations after. The
invalidation test: if removing the first target leaves the ADR
intact as a standalone decision, the first target is too weak —
promote a more constraining parent.

```
References: CHE-0006, COM-0018      ← CHE-0006 is the parent
Supersedes: CHE-0027                ← does not affect parent edge
```

### Diagnostics

| ID   | Severity | Trigger |
|------|----------|---------|
| L010 | warning  | Non-Root ADR has no `References:` (no parent) |
| L011 | warning  | First `References:` target is in a different domain |
| L012 | warning  | First `References:` target is Draft or Proposed |
| L013 | warning  | Parent-edge graph contains a cycle |
| L014 | warning  | Parent chain does not terminate at a Root |
| L015 | warning  | First reference is a Root while same-domain non-Root candidates exist |
| L016 | warning  | Structural parent's tier is lower-leverage than child's |
| L017 | warning  | First `References:` target is Superseded |
| L018 | warning  | `Parent-cross-domain` ID does not match first References target |
| L019 | warning  | `Parent-cross-domain` target ADR does not exist |

L015 and L016 are heuristics — suppression is a judgment call,
typically by reordering references rather than adding configuration.

## Consequences

Reordering `References:` re-parents the ADR; the first reference is
load-bearing. Migration from "Root first" to "specialized parent
first" is per-domain and manual: run `--lint`, fix L015 by
reordering, repeat. Some ADRs keep a Root first when both refs are
body-prose direct constraints — exceptions below.

### L015 known exceptions

| ADR | Root | Co-cited | Rationale |
|-----|------|----------|-----------|
| AFM-0014 | AFM-0001 | AFM-0003 | Stderr seam constrained by exit-code semantics |
