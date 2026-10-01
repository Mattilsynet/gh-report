# RST-0003. Workspace Lint and Format Governance

Date: 2026-04-27
Last-reviewed: 2026-10-01 — refined — clarified R5 item-targeted suppression scope and RFC-2119 keywords, eliminating overlap with R6 (COM-0027, ghr-gnx6o)
Tier: B
Status: Accepted

## Related

References: RST-0001, COM-0017, GND-0005

## Context

Rust provides `clippy` (800+ static analysis lints) and `rustfmt`
(canonical formatting). Lint governance has three dimensions: what
to enforce (lint groups from correctness to pedantic), where to
configure (per-crate vs workspace-level), and how to enforce
(local development, CI, or both). Workspace-level configuration
ensures all crates share the same standard (COM-0009). CI
enforcement with `-D warnings` makes compliance a merge gate.
COM-0017 (Mechanized Invariant Enforcement) establishes that
machine-enforced rules do not degrade; linters and formatters are
the primary mechanized enforcement tools.

## Decision

Lint and formatting rules are configured at workspace level,
enforced in CI as merge gates, and evolved intentionally with
toolchain updates.

R1 [5]: All clippy lint levels are set in
  `[workspace.lints.clippy]` in root `Cargo.toml`; crates inherit
  via `[lints] workspace = true` with no per-crate overrides
R2 [5]: `clippy.toml` at workspace root sets `msrv` to match the
  project MSRV, suppressing suggestions for unavailable APIs
R3 [5]: `rustfmt.toml` at workspace root defines formatting;
  only stable rustfmt options are used
R4 [5]: CI runs `cargo clippy -- -D warnings` and
  `cargo fmt --check`; both must pass for PR merge
R5 [6]: Lint suppressions MUST be targeted to specific items using
  `#[expect(lint, reason = "...")]` with an explicit justification;
  `#[allow]` MAY be used only where an expectation cannot be fulfilled.
  Blanket module-level inner suppressions (`#![allow(...)]` or
  `#![expect(...)]`) MUST NOT be used
R6 [5]: Rust sources under `crates/*/src` MUST NOT carry an inner
  attribute `#![allow(dead_code)]` or `#![expect(dead_code)]`,
  including `cfg_attr`-wrapped forms; outer item-level
  `#[expect(dead_code, reason = "...")]` MAY suppress a specific item.
  Enforced by the `dead-code-inner-suppression-tripwire` job in CI
  (COM-0017:R4)

## Consequences

All crates compile under the same lint standard. Clippy's MSRV
awareness prevents false suggestions. `rustfmt` eliminates
formatting debates — there is no preferred style, only the
formatted style. CI enforcement guarantees no gradual degradation
on the main branch. New clippy lints arrive with toolchain bumps
(RST-0002) and are addressed systematically.
