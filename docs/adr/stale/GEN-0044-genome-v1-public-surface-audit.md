# GEN-0044. Genome v1 Public Surface Audit

Date: 2026-05-18
Last-reviewed: 2026-05-18
Tier: B
Status: Accepted

## Related

References: GEN-0035, GEN-0011, GEN-0013, GEN-0014, GEN-0015, GEN-0018, GEN-0026, GEN-0030, GEN-0037, GEN-0038, GEN-0005, GEN-0008

## Context

A reachability audit of 60 public symbols in `crates/pardosa-genome/src/{error,config}.rs`
(evidence bead `adr-fmt-jrd0`) found 7 symbols with zero construction sites in src/ or
tests/ and no naming ADR, 30 symbols with zero current call sites but explicit retention
by an Accepted GEN ADR, and 23 symbols that are live wire-contract surface. A separate
width drift was confirmed: `GenomeSafe::SCHEMA_HASH: u128` (GEN-0035) vs
`DeError::SchemaMismatch { expected: u64, actual: u64 }` (GEN-0002, GEN-0011 #2). v1
must commit to a single answer on each surface before the public-API freeze.

## Decision

Classify every audited symbol as **Dead** (remove), **Deferred** (retain — named by
an Accepted ADR), or **Reserved** (retain — wire/forward-compat surface). Reconcile
the `SchemaMismatch` width to `u128`. The Dead-symbol removal is executed by a
separate mission (M5) gated on this ADR's acceptance.

R1 [5]: The 7 Dead symbols enumerated in the Migration section MUST be removed from
  `pardosa-genome` by mission M5; until M5 lands they remain in source but no new
  call site may reference them.
R2 [5]: The 30 Deferred symbols are retained because each is named by an Accepted ADR
  (citations in Migration section); removing one requires either a wire-format ADR or
  a supersedes of its naming ADR.
R3 [5]: The 23 Reserved symbols form the v1 wire-contract surface (FileError variants,
  PageClass, Compression, and the serde::Error::custom bridge); they are retained as
  the public binary-format contract and may not be removed during v1.
R4 [5]: `DeError::SchemaMismatch` payload widths MUST be widened from `u64` to `u128`
  to match `GenomeSafe::SCHEMA_HASH: u128` (GEN-0035); the reconciliation lands in
  mission M5 alongside the Dead-symbol removal.
R5 [6]: Every public type, enum variant, and field exported from `pardosa-genome` MUST
  be either reachable from in-tree call sites or named by an Accepted GEN ADR; new
  public additions cite the binding ADR in a doc comment.
R6 [6]: The serde-trait-bridge symbols (`SerError::Custom`, `DeError::Custom`,
  `SerMessage`) are retained as Reserved on the strength of the `serde::ser::Error`
  and `serde::de::Error` trait contracts, not on observed call sites.

## Consequences

+ becomes easier: future audits — every public surface element has a documented
  classification and citation; M5 ships with a precise work list.

− becomes harder: adding a new public symbol — authors must show a call site or cite
  a binding ADR (R5).

risks/migration: M5 removes the 7 Dead symbols and widens `DeError::SchemaMismatch`
  to `u128`; payload-shape break for downstream pattern matches, wire-irrelevant.
  **Open question:** GEN-0005 (two-pass) is Accepted but GEN-0035 states "The
  encoder is single-pass" without `Supersedes: GEN-0005`. This ADR does NOT
  supersede GEN-0005; reconciliation is deferred to its own ADR.

## Migration

Mission **M5 — Genome v1 Dead-Symbol Cleanup** consumes this list. All file:line
references are `crates/pardosa-genome/src/…` at the audit snapshot (bead
`adr-fmt-jrd0`).

### Dead symbols to remove (7)

| Symbol | Site | Notes |
|---|---|---|
| `SerError::MessageTooLarge` | error.rs:12 | One test-only reference at `genome_safe.rs:517`; no ADR names this variant. |
| `SerError::UnsupportedAttribute` | error.rs:14 | Zero references; no ADR. |
| `SerError::CompressionFailed` | error.rs:18 | Zero references; compression handled via `FileError` on the file path. |
| `DeError::AllocRequired` | error.rs:72 | Zero references; no ADR. |
| `DeError::MessageTooLarge` | error.rs:94 | Zero references; no ADR names this specific `DeError` variant (the `SerError` sibling is also Dead). |
| `EncodeOptions` (struct) | config.rs:73 | Zero external constructions or reads. |
| `EncodeOptions::compression` (field) | config.rs:75 | Dead by containment (parent struct unused). |

### Width reconciliation (R4)

| Site | Current | Target |
|---|---|---|
| `error.rs:82` `DeError::SchemaMismatch { expected, actual }` | `u64`, `u64` | `u128`, `u128` |

M5 must update any pattern-match site (currently `genome_safe.rs:524`, test-only)
and the Display impl.

### Deferred symbols retained (30) — citations

- `SerError` (enum), `SerError::InternalSizingMismatch` — GEN-0005 (see Open question above).
- `DeError` (enum) — GEN-0011, GEN-0014, GEN-0015, GEN-0018, GEN-0026, GEN-0037.
- `DeError::BufferTooSmall` — GEN-0011 #3, #17; GEN-0014.
- `DeError::OffsetOutOfBounds` — GEN-0011 #4.
- `DeError::OffsetOverflow` — GEN-0011 #5.
- `DeError::InvalidUtf8` — GEN-0011 #7.
- `DeError::InvalidChar` — GEN-0011 #8.
- `DeError::InvalidBool` — GEN-0011 #9.
- `DeError::InvalidDiscriminant` — GEN-0037.
- `DeError::DepthLimitExceeded` — GEN-0013 (`max_depth`).
- `DeError::ElementLimitExceeded` — GEN-0013 (`max_total_elements`).
- `DeError::TrailingBytes` — GEN-0011 #11, #12; GEN-0035.
- `DeError::VersionMismatch` — GEN-0011 #1; GEN-0015.
- `DeError::SchemaMismatch` — GEN-0011 #2 (width-reconciled per R4).
- `DeError::NonZeroPadding` — GEN-0018; GEN-0011 #10, #16.
- `DeError::BackwardOffset` — GEN-0011 #6.
- `DeError::ChecksumMismatch` — GEN-0011 #14 (distinct from `FileError::ChecksumMismatch` per GEN-0026 R3).
- `DeError::DecompressionFailed` — GEN-0014.
- `DeError::UncompressedSizeTooLarge` — GEN-0014 (layers 1 & 2).
- `DeError::PostDecompressionTrailingBytes` — GEN-0014; GEN-0011 #15.
- `FileError::CompressionNotAvailable` — GEN-0030.
- `FileError::MessageError(u64, DeError)` — compositional bridge to Deferred `DeError` (GEN-0008 transport-agnostic core).
- `DecodeOptions` (struct) — GEN-0013, GEN-0014, GEN-0038.
- `DecodeOptions::max_depth` — GEN-0013.
- `DecodeOptions::max_total_elements` — GEN-0013.
- `DecodeOptions::max_uncompressed_size` — GEN-0013, GEN-0014.
- `DecodeOptions::max_message_size` — GEN-0013, GEN-0038.
- `DecodeOptions::max_event_size` — GEN-0038.
- `DecodeOptions::max_zstd_window_log` — GEN-0013, GEN-0014.
- `DecodeOptions::reject_trailing_bytes` — GEN-0013.
- `DecodeOptions::for_page_class` — constructor for the Deferred struct.

### Reserved wire-contract surface (23)

Live `FileError` variants (`InvalidMagic`, `UnsupportedVersion`,
`UnsupportedCompression`, `InvalidChecksum`, `ChecksumMismatch`, `InvalidIndex`,
`InvalidSchemaSource`, `InvalidReserved`, `IndexOverflow`, `Io`), plus `FileError`
itself, the `PageClass` enum and its 4 numeric variants + `max_elements` +
`from_byte`, the `Compression` enum and its `None`/`Zstd` variants, and the
`SerError::Custom` / `DeError::Custom` / `SerMessage` serde-trait bridge. Full
table in evidence bead `adr-fmt-jrd0`.
