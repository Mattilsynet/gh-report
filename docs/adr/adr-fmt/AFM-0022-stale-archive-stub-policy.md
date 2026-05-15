# AFM-0022. Stale Archive Stub Policy

Date: 2026-05-01
Last-reviewed: 2026-05-02
Tier: B
Status: Accepted

## Related

References: AFM-0003, AFM-0008, AFM-0009

## Context

The stale archive (`docs/adr/stale/`) holds ADRs that have been
superseded, deprecated, or retired. Keeping full bodies creates two
problems. First, stale bodies decay: AFM-0014 still described the
six-mode CLI surface including the removed flag after AFM-0021
superseded it, so a reader encountered claims that contradict the
current tool. Second, lint surface accumulates — template rules fire
on stale prose, References from stale ADRs pollute the citation
graph, and reviewers must mentally filter "is this still
true?" on every retired document. Git history is the authoritative
record of what an ADR said when it was authoritative; the working
copy should answer "this decision exists, here is
who replaced it, here is why it was retired" — nothing more.

## Decision

Stale ADRs reduce to a stub: the preamble fields, an optional
`## Related` section restricted to `Supersedes:` lineage edges,
and a `## Retirement` section. All other body content is deleted
in the same commit that moves the ADR to stale. A new advisory
lint rule (S007) enforces the stub structure positively, and
T007/T008/T009/T010/T016 skip stale ADRs so compliant stubs
remain lint-clean.

R1 [11]: Confine files under `docs/adr/stale/` with a terminal
  `Status:` (`Superseded by X`, `Deprecated`, `Rejected`) to
  the preamble, an optional `## Related` section limited to
  `Supersedes:` edges (the reverse direction lives in the
  `Status:` field), and a `## Retirement` section
R2 [11]: Delete `## Context`, `## Decision`, and `## Consequences`
  sections and any `References:` lines in the same commit that
  moves an ADR to stale; preserve full prior content via git history
R3 [11]: Apply lint rule S007 (severity warning) on any stale ADR
  whose section list or relationship list violates R1, with one
  diagnostic per violation, skipping T007/T008/T009/T010/T016 for
  stale ADRs so the stub form is positively defined by S007 alone

The `## Retirement` body itself is unstructured prose. A
conventional `Superseded-by:` / `Moved-to-stale:` / `Reason:`
triple appears in most stubs as a quick reference, but narrative-
only retirements (see AFM-0010) are also accepted; no rule
parses or validates the retirement-block contents.

## Consequences

The stale archive stays small, lint-clean, and accurate by
omission. Authors moving an ADR to stale follow a fixed
transformation: strip three sections, strip `References:`, write
the retirement narrative. Readers consulting a stale ADR see only
what is still true (this decision was retired, this is its
successor) and reach for git history when they need the full
historical reasoning. The trade-off is that the working copy no
longer answers "what did this ADR originally decide?" — but stale
ADRs are by definition non-authoritative, so that question
properly belongs to the version-control archive. S007 is advisory
(per AFM-0003): a non-compliant stale ADR produces warnings but
does not block any workflow, matching the rest of the lint surface.
