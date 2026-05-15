# COM-0028. MECE Decomposition of Responsibilities

Date: 2026-04-30
Last-reviewed: 2026-05-12
Tier: B
Status: Accepted

## Related

References: COM-0007, COM-0014, COM-0027

## Context

COM-0014 aligns boundaries to rate of change; COM-0007 minimizes
leakage. Neither states the partitioning property: each
responsibility lives in exactly one place (mutually exclusive) and
together the set covers the problem (collectively exhaustive).
Overlap creates duplicated truth; gaps create implicit
responsibilities surfacing as integration bugs.

Three options evaluated:

1. **Implicit partitioning** — judgment-based; gaps accumulate.
2. **Gap detection only** — coverage tools find unreached *code*,
   not unreached *responsibilities*.
3. **Explicit MECE check at decomposition time** — review every
   split for both properties; record the partitioning axis.

Option 3 chosen: cheap at decomposition, costly to retrofit.

## Decision

Every decomposition — module split, error-variant set, state
enumeration, ADR domain partition — must justify both that the
parts do not overlap and that they cover the whole problem.
Decompositions that fail either test are revised before adoption.

R1 [5]: Decompose responsibilities so each concern has exactly one
  owning module, type, or trait; overlapping ownership is rewritten
  to assign a single authority before the change merges
R2 [5]: Demonstrate that an enumeration of error variants, states,
  or message kinds covers every reachable case; uncovered cases
  become an explicit Other or Unknown variant rather than implicit
R3 [6]: Record the partitioning axis — rate of change, audience,
  trust boundary, lifecycle — in the doc comment of the parent
  module so future additions choose the same axis
R4 [5]: When two modules answer the same question, merge them or
  redraw the boundary; parallel implementations of one
  responsibility are defects regardless of code quality

## Consequences

- **Pairs with COM-0027.** SSOT is the data-level corollary of MECE
  at the responsibility level — both forbid duplicated authority.
- **Forces exhaustiveness.** Sum types, `#[non_exhaustive]`, and
  match-arm coverage become design tools, not just safety nets.
- **Tension with COM-0002.** Deep modules consolidate; MECE may
  split. Resolution: consolidate within an axis, split across axes.
- **Refactor pressure.** Existing parallel implementations surface
  as MECE violations and must be merged or redrawn.
