# COM-0038. Cross-Domain Overlap Resolution

Date: 2026-05-01
Last-reviewed: 2026-05-01
Tier: A
Status: Accepted

## Related

References: COM-0001

## Context

Domain-specific ADRs sometimes cover the same concern as a foundation
domain ADR (COM, RST, SEC) at a different abstraction level. Two
resolutions are possible: merge into one ADR, or keep both with
cross-references. Merging conflates abstraction levels and forces
either the principle or the implementation to compromise. Cross-
referencing preserves the rate-of-change boundaries identified in
COM-0014. This ADR pins the cross-referencing pattern and absorbs
the overlap-resolution guidance previously held in GOVERNANCE.md §4.

## Decision

When a domain ADR and a foundation ADR cover the same concern at
different abstraction levels, keep both standalone and link from
the concrete to the abstract via `References:`.

R1 [5]: Treat foundation ADRs (COM, RST, SEC) as principles and
  domain ADRs as implementations; keep both as standalone ADRs
  cross-referenced from concrete to abstract
R2 [5]: List the same-domain structural parent first in `References:`
  when the concrete ADR participates in a same-domain subtree;
  list the foundation citation later to keep the ADR rooted in
  its own domain
R3 [5]: When the concrete ADR has no same-domain parent and a
  foundation ADR is the natural parent, list the foundation ADR
  first and add `Parent-cross-domain: PREFIX-NNNN — reason` to
  suppress L011 per AFM-0020
R4 [5]: Reserve merging for ADRs in the **same domain** that
  genuinely cover the same decision space; the newer ADR then
  supersedes the older one via `Supersedes:`

## Consequences

Worked example: COM-0016 is the principle (Dependencies as Managed
Liabilities); RST-0004 implements it for Cargo. PAR-0016 references
PAR-0004 first (same-domain parent) then COM-0025 (foundation
principle). The pattern lets a domain evolve at its own rate while
preserving foundation citations for traceability. Cost: readers
must traverse cross-references to assemble the full argument; this
is the trade-off COM-0014 already accepts in exchange for clean
rate-of-change boundaries.
