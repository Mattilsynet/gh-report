# RST-0001. Pinned Stable Toolchain with MSRV Contract

Date: 2026-04-27
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

Root: RST-0001

## Context

Rust releases a new stable version every six weeks. Without an
explicit toolchain pin, each developer's environment drifts to
whichever version `rustup update` last installed, causing surprise
lint failures, feature availability mismatches, and irreproducible
builds. The `rust-toolchain.toml` file solves this: `rustup` reads
it and installs the specified toolchain automatically. Cherry-pit
is an application workspace, not a published library, so the MSRV
policy is simple: latest stable. The `rust-version` field in
`Cargo.toml` declares the floor and Cargo's resolver respects it
when selecting dependency versions.

## Decision

Pin the Rust toolchain via `rust-toolchain.toml` at the workspace
root on stable channel. MSRV matches the pinned version.

R1 [5]: The `rust-toolchain.toml` specifies a stable major.minor
  channel (e.g. `"1.95"`), never `"stable"` or nightly
R2 [5]: MSRV in `[workspace.package].rust-version` matches the
  pinned toolchain channel exactly
R3 [5]: The Rust edition is declared once in `[workspace.package]`
  and inherited by all crates via `edition.workspace = true`
R4 [5]: `Cargo.lock` is committed to version control for
  reproducible dependency resolution across environments
R5 [6]: Toolchain profile is `minimal` with only `clippy` and
  `rustfmt` components explicitly included
R6 [5]: CI verifies rust-toolchain.toml, Cargo.toml rust-version,
  and clippy.toml msrv all declare the same Rust version

## Consequences

Contributors get the correct toolchain automatically on clone.
Build results are reproducible across environments. Toolchain
updates become deliberate, reviewable events (RST-0002), not
ambient drift. MSRV appears in three files (`rust-toolchain.toml`,
`clippy.toml`, `Cargo.toml`) which must stay in sync during
updates. CI enforces that `rust-toolchain.toml`, `Cargo.toml`, and
`clippy.toml` remain synchronized.
