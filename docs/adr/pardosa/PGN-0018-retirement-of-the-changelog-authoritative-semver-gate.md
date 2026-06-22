# PGN-0018. Retirement of the CHANGELOG-Authoritative Semver Gate

Date: 2026-06-22
Last-reviewed: 2026-06-22
Tier: B
Status: Accepted
Crates: pardosa, pardosa-wire, pardosa-derive, pardosa-schema, pardosa-file

## Related

References: PGN-0012, PGN-0001, PGN-0009

## Context

PGN-0012 R1/R4 and PGN-0001 R6/R7 made `docs/adr/pgn/CHANGELOG.md` authoritative for Pardosa public-surface changes. The user deliberately retired that gate during the PGN-0009 clean-break window: all five publishable crates remain `0.x` and no external dependent has stabilised. GND-0007 requires the corpus to record the transition explicitly instead of silently rewriting accepted governance.

## Decision

The changelog-authoritative semver gate is retired, not relocated. No replacement file or authoritative record gate exists. Author judgement remains the primary semver gate, and `cargo-semver-checks` remains advisory. PGN-0012 and PGN-0001 stay Accepted with reduced scope; nothing moves to `docs/adr/stale/`.

R1 [5]: The changelog-authoritative semver gate named by PGN-0012 R1/R4
  and PGN-0001 R6/R7 is retired; no ADR or PR must create, append,
  relocate, or preserve a changelog file for release governance.
R2 [5]: Semver governance is author-judgement-primary and
  tool-advisory; `cargo-semver-checks` remains a review signal, not an
  auto-merge block or authoritative substitute.
R3 [5]: PGN-0012 and PGN-0001 remain Accepted with reduced scope; only
  the named record-file clauses retire, and their unrelated rules stay
  live.
R4 [5]: The retirement is justified by PGN-0009 clean-break while the
  crates are `0.x` with zero external dependents; PGN-0012 R5 ends that
  posture at the first stabilised external dependent.
R5 [5]: Re-establishing any authoritative semver gate before the
  PGN-0012 R5 1.0 trigger requires a fresh ADR naming the authority,
  record shape, and migration from this retirement record.
R6 [5]: Partial retirement of a sub-policy embedded in an Accepted ADR
  uses an explicit transition record plus surgical host edits; the host
  ADR does not move to stale unless all rules are retired.

## Consequences

+ becomes easier: the corpus records the user decision, removes the ghost gate, and keeps live semver doctrine visible.
− becomes harder: reviewers carry the audit burden through judgement plus advisory tooling; reintroducing a gate now costs a fresh ADR.
risks/migration: retired entries remain only in git history; PGN-0012 and PGN-0001 stay Accepted; cross-references re-point to this transition record; CI wiring stays advisory.
