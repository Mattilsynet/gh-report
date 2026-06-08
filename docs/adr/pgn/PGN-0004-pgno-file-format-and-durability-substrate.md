# PGN-0004. `.pgno` File Format and Durability Substrate

Date: 2026-06-08
Last-reviewed: 2026-06-08
Tier: B
Status: Accepted
Crates: pardosa-file, pardosa

## Related

References: PGN-0001, PGN-0003

## Context

Sources rescue ADR-0006 (`.pgno` byte layout frozen for v0.x) and rescue ADR-0010 (durability levels — `Lsn`, `Durability`, `Syncable`). `pardosa-file` defines the on-disk container format and the `Syncable` substrate trait. The substrate owns durability primitives (`File::sync_data`, `set_len`); the runtime composes them via `Journal<T, W>` and exposes `Lsn` / `Durability` on `AppendResult`. The pre-publish window (PGN-0009, PGN-0012) authorises wire-format changes; once published, ADR-0006 §D2 governs them. Body-only compression rides the file-header `flags` low bits, not `page_class`.

## Decision

`.pgno` carries Magic + `FORMAT_VERSION`, `page_class`, `schema_hash: u128`, `schema_size: u32`, per-message frames with `xxh64` body checksum, footer with `xxh64` checksum, and an ascending offset index validated on `Reader::open`. `Syncable` is a sealed substrate trait providing `sync_data()` and `set_len()`; `Writer<W: Syncable>` and `Journal<T, W: Syncable + Seek>` compose it. `Lsn` is opaque, `pub(crate)`-constructed, and minted only by `Journal::sync_data`. `Durability` is `#[non_exhaustive]` with `InMemory` and `Synced { lsn }`.

R1 [5]: Schema hash mismatch surfaces as a typed variant at `Reader::open`
  before any payload byte is decoded — the schema hash is the load-bearing
  gate; the format-version bump is the redundant outer guard.
R2 [5]: Per-message checksum is `xxh64` over the stored (post-compression)
  bytes; the reader validates the stored checksum before decompression and
  bounds decompressed output by `ReaderOptions::max_decompressed_message_bytes`.
R3 [5]: `Syncable` is sealed via a private `sealed::Sealed` supertrait; the
  impl set is closed in-tree (`Vec<u8>`, `Cursor<Vec<u8>>`, `std::fs::File`,
  `BufWriter<W: Syncable>`, `&mut W`).
R4 [5]: `Lsn::new` is `pub(crate)`; the journal layer is the sole
  authoritative producer of `Lsn` values. `Durability` is `#[non_exhaustive]`
  with `InMemory` and `Synced { lsn }`.
R5 [5]: Payload-body compression is gated by the file-header `flags` low
  three bits (`ALGO_NONE = 0x00`, `ALGO_ZSTD = 0x01`); native structures
  (header, footer, index, schema source) stay raw.
R6 [4]: `xxh64` is an accidental-corruption detector, not a MAC; tamper
  resistance against an active adversary requires an external keyed
  construction layered over the container.

## Consequences

+ becomes easier: auditing the wire format from one substrate crate;
  type-system-observable durability boundary; sink expansion via one
  explicit `Syncable` impl per sink type.
− becomes harder: adding a field to `IndexEntry` (a major-version event
  post-publish); shipping `tokio::fs::File` as a sink (requires a
  `pardosa-file` PR rather than a downstream impl).
risks/migration: an `xxh64` swap to a cryptographic checksum requires an
  explicit product threat-model ADR and is wire-affecting; macOS is
  dev-only (no `F_FULLFSYNC` hardening planned).
