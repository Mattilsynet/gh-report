# COM-0027. Single Source of Truth Across Representations

Date: 2026-04-30
Last-reviewed: 2026-05-12
Tier: B
Status: Accepted

## Related

References: COM-0009, COM-0017

## Context

COM-0009 establishes consistency; COM-0017 mechanizes invariants.
Neither addresses duplication of *truth* — the same fact in two
representations that can drift. Examples: event schemas restated in
storage, ADR rules duplicated in doc comments, version numbers in
both Cargo.toml and code. Each duplication creates an invisible
synchronization liability.

Three options evaluated:

1. **Comments-as-discipline** — relies on humans; comments rot.
2. **Drift detection** — reactive; second representation persists.
3. **Canonical authority + derivation** — codegen, macro, or build
   step makes drift structurally impossible.

Option 3 chosen: derivation moves the problem to build-time, where
errors fail loud.

## Decision

For every fact expressed in code or configuration, designate a
single authoritative location and derive all other representations
from it. Duplicated facts without a derivation relationship are
treated as defects.

R1 [5]: Designate one authoritative location for each constant,
  schema, version number, or configuration value; record the
  authority in a doc comment on the canonical definition
R2 [6]: Derive all secondary representations of a fact via macros,
  build scripts, or codegen so drift is impossible by construction
  rather than caught by test
R3 [5]: When two locations express the same fact and neither
  derives from the other, mark one canonical and convert the other
  to a derived view in the same change set
R4 [6]: Reject pull requests that introduce a second hand-maintained
  representation of a fact that already has a canonical owner

## Consequences

- **Codegen surface grows.** Build-time derivation expands
  `build.rs`, proc-macros, and ADR tooling like `adr-fmt --context`.
- **Pairs with COM-0017.** Mechanized enforcement is the natural home
  for SSOT — type system encodes the authority, codegen propagates.
- **Cost.** Derivation infrastructure has its own complexity; small
  duplications may not justify it. R3 is the threshold trigger.
- **Failure mode prevented.** Schema drift between event payload and
  storage layout, version skew between Cargo.toml and code, and ADR
  rules contradicting their referenced types.
