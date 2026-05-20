# AFM-0029. ADRs Are Atomic

Date: 2026-05-20
Last-reviewed: 2026-05-20
Tier: B
Status: Accepted

## Related

References: AFM-0022

## Context

An ADR is the unit of architectural commitment. It is either in force
or retired; there is no partial state. Allowing some rules of an ADR
to be superseded while others remain in force splits the unit, makes
authority transfer ambiguous, and forces every reader to cross-check
which clauses still apply. The corpus is easier to reason about when
the ADR is the atom of supersession.

## Decision

R1 [5]: `Supersedes:` names whole ADR identifiers only. Clause-level
  forms are prohibited.

R2 [5]: When only some rules of an ADR become obsolete, the ADR is
  amended in place: obsolete rules are deleted, survivors renumbered
  to remove gaps, `Last-reviewed:` bumped. Prior wording is preserved
  by git history. No supersession edge is created.

R3 [5]: When an ADR is fully replaced, the predecessor moves to
  `docs/adr/stale/` per AFM-0022 and the successor declares whole-ADR
  `Supersedes:`. This is the only supersession path.

R4 [5]: `References:` lines may cite clause-level form. Citation
  carries no replacement semantics; only `Supersedes:` is constrained.

## Consequences

+ becomes easier: reading the citation graph — every `Supersedes:`
  edge is a complete authority transfer.

− becomes harder: nothing material — the amend-in-place path was
  already convention for narrow obsolescence.

risks/migration: none. Lint enforcement is deferred to a future
adr-fmt parser change.
