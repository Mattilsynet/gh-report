# PGN-0004. `.pgno` File Format and Durability Substrate

Date: 2026-06-08
Last-reviewed: 2026-06-08
Tier: B
Status: Accepted
Crates: pardosa-file, pardosa

## Related

References: PGN-0001, PGN-0003

## Context

Sources rescue ADR-0006 (`.pgno` byte layout frozen for v0.x) and rescue ADR-0010 (durability boundary — `Lsn`, `Syncable`). `pardosa-file` defines the on-disk container format and the `Syncable` substrate trait. The substrate owns durability primitives (`File::sync_data`, `set_len`); the runtime composes them via the writer ring and exposes durable positions to adopters through `StoreWriter::sync` (returning [`Lsn`]) and `StoreWriter::acked_lsn`. Backend-local positional metadata (`.pgno` post-fsync offset, JetStream `PubAck.seq`) is carried by [`AckPosition`] per PGN-0010 — positional only, not durability evidence on its own. The pre-publish window (PGN-0009, PGN-0012) authorises wire-format changes; once published, ADR-0006 §D2 governs them. Body-only compression rides the file-header `flags` low bits, not `page_class`.

## Decision

`.pgno` carries Magic + `FORMAT_VERSION`, `page_class`, `schema_hash: u128`, `schema_size: u32`, per-message frames with `xxh64` body checksum, footer with `xxh64` checksum, and an ascending offset index validated on `Reader::open`. `Syncable` is a sealed substrate trait providing `sync_data()` and `set_len()`; `Writer<W: Syncable>` and the runtime journal compose it. `Lsn` is opaque, `pub(crate)`-constructed, and minted only by the runtime sync path; adopters observe it via `StoreWriter::sync` / `StoreWriter::acked_lsn`. No `Durability` enum is exposed: a present `Lsn` *is* the "this event survived fsync" commitment, with no in-memory variant required at the public boundary. Backend-local position metadata is the separate `AckPosition` newtype per PGN-0010.

R1 [5]: Schema hash mismatch surfaces as a typed variant at `Reader::open`
  before any payload byte is decoded — the schema hash is the load-bearing
  gate; the format-version bump is the redundant outer guard.
R2 [5]: Per-message checksum is `xxh64` over the stored
  (post-compression) bytes. The reader enforces decompression-bomb
  mitigation in a fixed order: (1) schema-hash gate per R1; (2)
  validate stored `xxh64` on raw post-compression bytes before
  invoking the decompressor; (3) reject any `msg_data_size` header
  exceeding `ReaderOptions::max_decompressed_message_bytes` before
  allocation; (4) reject any zstd `Frame_Content_Size` exceeding the
  same cap before allocation; (5) decompress under a bounded
  `ZSTD_d_maxWindowLog` (22, 4 MiB); (6) verify post-decompression
  length matches `msg_data_size`. The default value of
  `max_decompressed_message_bytes` is 1 GiB and is owned here;
  PGN-0013 does not restate it.
R3 [5]: `Syncable` is sealed via a private `sealed::Sealed` supertrait; the
  impl set is closed in-tree (`Vec<u8>`, `Cursor<Vec<u8>>`, `std::fs::File`,
  `BufWriter<W: Syncable>`, `&mut W`).
R4 [5]: `Lsn::new` is `pub(crate)`; the runtime sync path is the sole
  authoritative producer of `Lsn` values, observed by adopters via
  `StoreWriter::sync` (mint) and `StoreWriter::acked_lsn` (observe).
  No `Durability` enum is exposed — a present `Lsn` is the durability
  commitment; backend-local positional metadata is the separate
  [`AckPosition`] newtype per PGN-0010 (positional only, not durability
  evidence on its own).
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
