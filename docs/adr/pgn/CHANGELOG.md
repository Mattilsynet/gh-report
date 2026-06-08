# PGN Domain Changelog

This file is the PGN domain's release-governance audit trail per
PGN-0001 R6 and PGN-0012 R1. It scopes to the PGN ADR corpus and
the public surface of the five publishable crates governed by PGN
(`pardosa-wire`, `pardosa-schema`, `pardosa-file`, `pardosa-derive`,
`pardosa`). PGN release-governance entries live here until a
repo-level `CHANGELOG.md` is established; at that point the
repo-level changelog supersedes this file and the rule references
in PGN-0001 R6 / PGN-0012 R1 are re-anchored in a follow-up ADR.

Format follows the [Keep a Changelog](https://keepachangelog.com)
convention with a `breaking? Y/N` line on each entry per PGN-0001 R6.

## [Unreleased]

### Added

- **PGN-0013 — Schema Vocabulary and Resource Security Policy**
  (Tier B, Accepted, Crates: pardosa-wire, pardosa-schema,
  pardosa-file, pardosa-derive). Inherits GEN-0042 bounded wrappers,
  GEN-0041 foreign-type allowlist (uuid-only in current Rust),
  GEN-0034 codec fuzzing, GEN-0013 page-class resource limits, and
  GEN-0014 decompression-bomb mitigation into the PGN tree.
  breaking? N

- **PGN release-governance audit trail** introduced as this file
  (`docs/adr/pgn/CHANGELOG.md`). Anchors PGN-0001 R6 and PGN-0012 R1
  while a repo-level `CHANGELOG.md` does not yet exist. breaking? N

### Changed

- **PGN-0001 R4** rewritten to reflect current public adopter API:
  `pardosa::store` (canonical) plus the ergonomic single-glob
  `pardosa::prelude`. Removed the claim that adopters import via
  `pardosa::reader::prelude` / `pardosa::writer::prelude` — those
  ring-specific façades are not current public API. breaking? N
  (documentation truthfulness; no code surface change).

- **PGN-0001 R6** and **PGN-0012 R1 / R4** re-anchored to point at
  `docs/adr/pgn/CHANGELOG.md` (this file) until a repo-level
  changelog is established. breaking? N.

- **PGN-0004 Context / Decision / R4** rewritten to use current
  durability terminology: `Lsn` minted by the runtime sync path and
  observed via `StoreWriter::sync` / `StoreWriter::acked_lsn`,
  `AckPosition` for backend-local positional metadata (PGN-0010 §D2).
  Removed reference to a `Durability::{InMemory, Synced { lsn }}`
  enum — no such enum exists in the runtime crate; a present `Lsn`
  is itself the durability commitment. `.pgno` byte layout,
  checksum, and decompression-bound rules preserved unchanged.
  breaking? N (documentation truthfulness; no wire or API change).

- **PGN-0007 Decision tail / R6** rewritten to reflect current
  adopter surface: capability boundary is enforced type-level via
  `StoreReader<'_, T>` / `StoreWriter<'_, T>` (PGN-0008 R3), with
  `pardosa::store` + `pardosa::prelude` as the only public modules;
  ring-specific reader/writer preludes are not current public API.
  breaking? N.

- **PGN-0008 Context / R1** rewritten to state the current public
  root surface explicitly: `pardosa::store` (sole adopter-facing
  module) plus `pardosa::prelude` (ergonomic single-glob), with
  every ring-internal module `pub(crate)`. Removed the claim that
  root-level `pardosa::reader`, `pardosa::writer`, `pardosa::event_log`
  modules are demoted to `pub(crate)` — they are not shipped as
  source files in the runtime crate at all. breaking? N.

- **PGN-0011 Context** rewritten to reflect current implementation:
  `FiberIndex<K>`, `FiberLookup<F>`, and `ExtractError` are publicly
  re-exported from `pardosa::store` and `pardosa::prelude` today
  (`crates/pardosa/src/fiber_index.rs`, `store.rs`, `prelude.rs`);
  construction is opt-in via `StoreReader::fiber_index`; a journal
  opened without that call pays no per-event indexing cost.
  Removed the claim that no public symbol ships until an
  implementation mission lands. breaking? N (the implementation
  already conforms to the ADR-0023 / PGN-0011 contract; this is a
  documentation truthfulness amendment, not a contract change).
