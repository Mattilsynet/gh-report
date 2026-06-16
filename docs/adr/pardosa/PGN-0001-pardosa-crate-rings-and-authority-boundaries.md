# PGN-0001. Pardosa Crate Rings and Authority Boundaries

Date: 2026-06-08
Last-reviewed: 2026-06-08
Tier: S
Status: Accepted
Crates: pardosa, pardosa-wire, pardosa-schema, pardosa-file, pardosa-derive, pardosa-nats, pardosa-mint-private-tests, pardosa-test-support-harness

## Related

Root: PGN-0001

## Context

Sources rescue ADR-0002 (substrate purity / ring direction) and rescue ADR-0017 (pre-deployment clean-break posture). The Pardosa workspace partitions crates into rings — substrate, vocabulary, runtime — that may depend in one direction only. Substrate pre-dates typed event semantics and stays payload-opaque so it remains reusable outside Pardosa. The clean-break posture authorises pre-publish surface breaks when they restore a doctrinal rule. Where this consolidation conflicts with Solon PAR material, rescue precedence applies.

## Decision

Ring direction is `substrate (pardosa-wire, pardosa-derive, pardosa-file) → vocabulary (pardosa-schema) → runtime (pardosa)`. Substrate crates are sync, `thiserror`-free, `no_std`-capable where stated, and `#![forbid(unsafe_code)]`. The consumer façade is `pardosa` only. While `0.x` and zero external consumers, the workspace operates under a clean-break-preferred posture.

R1 [2]: Direct dependencies must flow substrate → vocabulary → runtime;
  a runtime concept may not be referenced from any substrate crate.
R2 [5]: Substrate crates must not depend on `thiserror`, `tokio`, `async-trait`,
  or any runtime-ring crate; substrate errors are handcrafted enum + Display.
R3 [5]: Every workspace crate sets `#![forbid(unsafe_code)]` at the crate root,
  including test-support crates; no `cfg_attr` carve-outs are permitted.
R4 [5]: External consumers depend only on `pardosa` and import items via
  the canonical `pardosa::store` module or the ergonomic single-glob
  `pardosa::prelude` (which re-exports the same items, broadening
  nothing); substrate paths are reached through
  `pardosa::__derive_support` for derive expansions. Ring-specific
  `pardosa::reader::prelude` / `pardosa::writer::prelude` façades are
  not current public API; the capability boundary is enforced
  type-level via `StoreReader`/`StoreWriter` per PGN-0008 R3.
R5 [4]: While pre-publish and no external dependent exists, prefer the smallest
  patch that restores the violated doctrinal rule exactly, even if breaking,
  over a soft-deprecation shim.
R6 [5]: Every break under the clean-break posture records `breaking? Y`
  plus a one-line scope description in `docs/adr/pgn/CHANGELOG.md` under
  `[Unreleased]`; PGN release-governance entries live there until a
  repo-level `CHANGELOG.md` is established.
R7 [5]: A PGN ADR that defines a sealed trait or codec contract carries
  the closed in-tree impl inventory for that contract in its own rule
  set (impl-inventory authority follows trait authority); a PGN ADR's
  Decision and Rules state contracts and invariants only, never
  status-of-today (which symbols ship versus which are deferred to a
  follow-up mission). Status-of-today belongs in
  `docs/adr/pgn/CHANGELOG.md` per R6.

## Consequences

+ becomes easier: substrate reuse outside Pardosa; bounded runtime dependency
  surface; correctness-first fixes pre-publish without deprecation theatre.
− becomes harder: persisting typed concerns inside `pardosa-file` (must move
  to runtime); adding a new substrate crate (requires sync, sealed-set
  discipline); carrying soft deprecations of any kind.
risks/migration: Solon PAR ADRs that assumed a different runtime shape are
  superseded by rescue precedence; PAR/GEN retirement and stale moves are
  deferred to a follow-up mission. The 1.0 trigger (PGN-0012) ends this
  posture; until then every doctrinal violation is fix-now rather than
  fix-later.
