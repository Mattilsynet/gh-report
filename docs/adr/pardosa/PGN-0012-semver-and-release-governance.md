# PGN-0012. Semver and Release Governance

Date: 2026-06-08
Last-reviewed: 2026-06-08
Tier: B
Status: Accepted
Crates: pardosa, pardosa-wire, pardosa-derive, pardosa-schema, pardosa-file

## Related

References: PGN-0001, PGN-0006, PGN-0009

## Context

Source rescue ADR-0009 (semver policy). Five crates publish to crates.io: `pardosa-wire`, `pardosa-derive`, `pardosa-schema`, `pardosa-file`, `pardosa`. The semver policy is judgement-primary and tool-advisory. `cargo-semver-checks` has documented gaps around `#[non_exhaustive]` enums (heavy in PGN-0006) and trait-associated-type changes. The pre-1.0 stance is stricter than Cargo defaults — `0.x → 0.(x+1)` is breaking and `0.x.y → 0.x.(y+1)` is non-breaking — to match user expectations during the clean-break window. The 1.0 trigger is "first external dependent stabilises", recorded in a follow-up ADR at bump time.

## Decision

`cargo-semver-checks --baseline rev:<last-tag>` runs in CI on every PR as an advisory signal, not an auto-merge block. Author judgement is the primary gate. No changelog record is required under the PGN-0009 clean-break posture. All five publishable crates ship `0.x` until the 1.0 trigger fires; at trigger time they bump simultaneously to `1.0.0`. Wire-format breakage is a major-version event for `pardosa-file` post-publish.

R1 [5]: A PR that changes public surface is gated by author judgement;
  `cargo-semver-checks` is advisory, and under PGN-0009 clean-break no
  changelog record is required.
R2 [5]: Pre-1.0, `0.x → 0.(x+1)` is reserved for breaking changes and
  `0.x.y → 0.x.(y+1)` is reserved for non-breaking changes — stricter
  than Cargo defaults but matching adopter expectations.
R3 [5]: Adding a variant to a `#[non_exhaustive]` enum that has always
  been `#[non_exhaustive]` is non-breaking by Cargo semver and by this
  policy; `cargo-semver-checks` flags are overridden by the CHANGELOG
  entry.
R4 [5]: Wire-format breakage in `pardosa-file` is a major-version event
  post-publish; pre-publish breaks under the PGN-0009 clean-break
  posture ship paired with regenerated golden fixtures.
R5 [5]: The 1.0 trigger ("first external dependent stabilises") and the
  simultaneous bump of all five publishable crates is recorded in a
  follow-up ADR at the time of the bump; the reasoning is narrative,
  not algorithmic.
R6 [4]: `cargo-semver-checks` runs in CI as an advisory job; a failure
  surfaces as a review signal, not an auto-merge block — author
  judgement is the primary gate.

## Consequences

+ becomes easier: discipline is written down before first publish; the
  `cargo-semver-checks` advisory mode catches the easy cases without
  blocking legitimate `#[non_exhaustive]` additions; the 1.0 trigger is
  unambiguous in shape (external dependent), even if narrative in detail.
− becomes harder: review depends directly on human judgement plus
  advisory-tool interpretation; silent breaking changes (rejected by
  the policy); skipping the simultaneous five-crate 1.0 bump.
risks/migration: human-judgement gating depends on reviewer discipline;
  the 1.0 trigger condition is loose by design; under the PGN-0009
  clean-break posture the pre-publish window stays open until the
  trigger fires.
