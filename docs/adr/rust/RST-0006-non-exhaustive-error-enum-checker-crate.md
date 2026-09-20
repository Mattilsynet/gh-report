# RST-0006. Non-Exhaustive Error-Enum Checker Crate

Date: 2026-07-14
Last-reviewed: 2026-07-14
Tier: B
Status: Accepted

## Related

References: RST-0004, RST-0003, PGN-0006, CHE-0021

## Context

The closed-enumeration policy C4.5/C4.6 reverses the original open-error
policy: public error enums must not carry `#[non_exhaustive]`.
The checker scans consumer libraries and actual resolved external Cherry
library targets from locked Cargo metadata, rather than deleted embedded paths.
RST-0004 R3/R4 establish the pattern of a durable,
CI-enforced checker as the correct home for this class of invariant,
rather than ad-hoc script tooling outside the workspace.

## Decision

Promote the checker to a permanent workspace member,
`crates/non-exhaustive-check`, and enforce it as a CI hard gate.

R1 [6]: Every `pub enum` deriving `thiserror::Error` in a library
  crate MUST NOT carry `#[non_exhaustive]` (closed enumerations per C4.5/C4.6)
R2 [5]: `crates/non-exhaustive-check` is the sole enforcement point
  for R1; it is a workspace member subject to the same lint and
  format governance as any other crate (RST-0003)
R3 [5]: CI fails the build when `non-exhaustive-check` exits
  non-zero; under-flagging is the accepted failure mode, over-flagging
  is not

## Consequences

+ becomes easier: catching a forbidden `#[non_exhaustive]` at PR time
  instead of during a later semver review
− becomes harder: adding variants requires downstream exhaustive matches
  to be updated rather than hidden behind wildcard arms
risks/migration: the checker only inspects literal `pub enum` syntax
  parsed by `syn`; macro-generated enums are invisible to it and
  remain a manual-review responsibility
