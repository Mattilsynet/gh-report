# pardosa-next — Integration & Improvement Plan

Cross-crate plan for pardosa + pardosa-genome integration, incorporating findings
from SRE, ML-Ops, and distributed systems evaluation. All changes are clean breaks —
no backwards compatibility constraints.

Supersedes pardosa.md Phase 2–5 and the Distributed Systems Review section.
Phase 1 (state machine + core types) remains complete and valid.

## Table of Contents

1. [Architecture Model](#architecture-model)
2. [Resolved Decisions (Amendments)](#resolved-decisions-amendments)
3. [Genome Spec Changes](#genome-spec-changes)
4. [Pardosa Core Changes (Pre-Phase 2)](#pardosa-core-changes-pre-phase-2)
5. [Phase 2 — Dragline + Concurrency](#phase-2--dragline--concurrency)
6. [Phase 3 — Server API](#phase-3--server-api)
7. [Phase 4 — Migration (New-Stream Model)](#phase-4--migration-new-stream-model)
8. [Phase 5 — Persistence + Genome Integration](#phase-5--persistence--genome-integration)
9. [Phase 6 — NATS Consumer Lifecycle](#phase-6--nats-consumer-lifecycle)
10. [Error Taxonomy (Revised)](#error-taxonomy-revised)
11. [Cargo.toml (Revised)](#cargotoml-revised)
12. [Architecture (Revised)](#architecture-revised)
13. [Design Invariants (Revised)](#design-invariants-revised)
14. [Test Plan](#test-plan)
15. [Open Questions (Updated)](#open-questions-updated)

---

## Architecture Model

### Persistence topology

```
                       ┌──────────────────────┐
                       │    NATS KV Bucket     │
                       │   PARDOSA_REGISTRY    │
                       │                       │
                       │  key: {name}.active   │
                       │  val: {gen}:{stream}  │
                       └──────────┬───────────┘
                                  │ watch / get
       ┌──────────────────────────┼──────────────────────────┐
       │                          │                          │
       ▼                          ▼                          ▼
┌─────────────┐          ┌─────────────┐           ┌─────────────┐
│  JetStream  │          │  JetStream  │           │   Genome    │
│  Stream     │          │  Stream     │           │   File      │
│  PARDOSA_   │          │  PARDOSA_   │           │  {name}_    │
│  {name}_g1  │          │  {name}_g2  │           │  g2.pgno    │
│  (deprecated│          │  (active)   │           │  (snapshot) │
│   grace=7d) │          │             │           │             │
└─────────────┘          └─────────────┘           └─────────────┘
```

### Migration model: new stream, new file

Migrations never mutate existing streams or files. Each migration:

1. Reads the active stream/file (immutable source of truth).
2. Creates a new JetStream stream and/or genome file.
3. Writes surviving events (with optional schema upcast) to the new stream/file.
4. Atomically updates the KV registry pointer.
5. Sets `max_age` on the old stream for a configurable grace period.
6. Old genome files are retained until the operator deletes them.

Crash recovery: discard incomplete new stream/file, retry from intact old stream.
The old stream/file is never modified — idempotent retry is always safe.

### Serialization layer

Genome replaces `serde_json` as the primary serialization format. JSON is retained
behind the `json` feature for debugging, human-readable output, and configuration.

| Path | Format | Use case |
|------|--------|----------|
| NATS publish/subscribe | Genome bare message (optionally compressed) | Hot path, real-time events |
| Genome file | Genome multi-message file | Snapshots, cold storage, migration source |
| Debug / config | JSON via `serde_json` | Logging, human inspection |

---

## Resolved Decisions (Amendments)

Amendments to pardosa.md Resolved Decisions. Original decisions not listed here
remain in effect.

| Decision | Old Choice | New Choice | Rationale |
|----------|-----------|------------|-----------|
| Serialization | JSON via `serde_json` | Genome (primary), JSON (optional feature) | Zero-copy reads, compression, schema fingerprinting |
| No-precursor | `Option<Index>` (None = first event) | `Index` with `Index::NONE` sentinel | Eliminates genome heap indirection per event (4 bytes + heap saved) |
| Event ID | Not specified | `event_id: u64`, globally monotonic across generations | Idempotent publish, JetStream `Nats-Msg-Id` dedup, replay dedup |
| Event fields | `pub` fields | Private fields, constructor, accessors, `#[non_exhaustive]` | Genome layout depends on field declaration order; public fields allow reordering |
| Migration model | In-place line rebuild with index remap | New-stream / new-file per migration | Immutable source, crash-safe, old stream serves reads during migration |
| Stream discovery | Not specified | NATS KV registry (`PARDOSA_REGISTRY`) | Atomic pointer update for consumer cutover |
| Locked→Rescue gate | `acknowledge_data_loss: bool` | `LockedRescuePolicy` enum | Communicates whether audit trail is preserved vs. history destroyed |
| Schema versioning | `EventEnvelope<T>` with `schema_version` (deferred) | Stream generation IS the schema version; no per-event version field | New-stream model eliminates cross-schema deserialization within a stream |
| Fiber serde | No derives | `#[derive(Serialize, Deserialize)]` | Enables genome file snapshots for fast startup |

---

## Genome Spec Changes

Changes to `genome.md` required for pardosa integration. Each is a clean break.

### G1. Bare message version prefix

Add a 2-byte version prefix to bare messages. Prevents silent misparse across
format version bumps. Without this, bare messages (the NATS wire format) have no
version indicator — old consumers silently misparse new-format messages.

```
Offset  Size  Field           Description
──────  ────  ──────────────  ──────────────────────────────────────
 0      2     format_version  Format version (u16 LE, starts at 1)
 2      4     msg_data_size   Byte count of data that follows (u32 LE)
 6      ?     inline_data     Root type's inline fields
 ?      ?     heap_data       Strings, vec elements, option/enum data
```

Cost: 2 bytes per bare message. Benefit: consumers can reject incompatible
versions with a clear error instead of silent corruption.

### G2. Full 32-bit CRC in message index

Replace the 31-bit truncated CRC + 1-bit flag with a separate layout:

```
Per-message index entry (20 bytes):
Offset  Size  Field           Description
──────  ────  ──────────────  ──────────────────────────────────────
 0      8     offset          Absolute file offset (u64)
 8      4     size            msg_data_size or compressed_size (u32)
12      4     checksum        CRC32/ISO-HDLC, full 32 bits (u32)
16      1     flags           Bit 0: has_checksum. Bits 1-7: reserved (must be 0)
17      3     reserved        Must be all zeros
```

Rationale: 31-bit truncation degrades the CRC32 polynomial's Hamming distance
guarantee (HD=4 at full width). The 4-byte-per-entry cost is negligible.
Full 32-bit CRC preserves burst-error detection for messages up to 3.7 GiB.

File footer `index_offset` and `message_count` calculations update to reflect
20-byte index entries (was 16).

### G3. Dictionary ID in file header

Reserve space for zstd dictionary support in the file header. The dictionary
itself is distributed out-of-band (file path, NATS Object Store, etc.).

```
File Header (32 bytes, all LE):
Offset  Size  Field           Description
──────  ────  ──────────────  ──────────────────────────────────────
 0      4     magic           ASCII "PGNO"
 4      2     format_version  Format version (starts at 1)
 6      2     flags           Header flags (see below)
 8      8     schema_hash     Compile-time schema fingerprint (u64 LE, xxHash64)
16      4     dict_id         Zstd dictionary ID (u32 LE, 0 = no dictionary)
20     12     reserved        Must be all zeros
```

NATS header: `Pardosa-Dict-Id: <hex u32>` (omitted when 0).

Implementation of dictionary compression is deferred. The header field is
reserved now to avoid a future breaking change.

### G4. Expand quality_hint to 5 bits

```
Header Flags (offset 6, u16 LE):
Bit(s)  Name              Values
──────  ────────────────  ──────────────────────────────────────
 0      compressed        0 = uncompressed, 1 = compressed
 1-3    compression_algo  000 = brotli, 001 = zstd, 010-111 = reserved
 4-8    quality_hint      0 = default, 1-31 = algorithm-specific level
                          brotli: 1-11 (quality)
                          zstd:   1-22 (compression level, no clamping needed)
 9-15   reserved          Must be 0
```

Rationale: 4-bit field (0-15) cannot represent zstd levels 16-22 without
lossy mapping. 5-bit field (0-31) covers the full range of both algorithms
with room for future additions.

### G5. `NatsPublisher` additions for pardosa integration

```rust
impl NatsPublisher {
    /// Publish pre-serialized genome bytes without re-serialization.
    /// Used during migration to passthrough unchanged events.
    /// First call still sends Pardosa-* headers if not yet sent.
    pub async fn publish_raw(&mut self, bytes: &[u8]) -> Result<(), NatsGenomeError>;

    /// Set Nats-Expected-Last-Subject-Sequence for optimistic concurrency.
    /// Applied to the next publish call, then cleared.
    pub fn with_expected_sequence(&mut self, seq: u64) -> &mut Self;
}
```

### G6. `NatsConsumer` stream discovery

```rust
impl NatsConsumer {
    /// Create a consumer that watches a NATS KV key for stream changes.
    /// On stream version change, reconnects to the new stream and
    /// re-reads Pardosa-* headers.
    pub async fn with_stream_discovery(
        kv: async_nats::jetstream::kv::Store,
        key: String,
    ) -> Result<Self, NatsGenomeError>;

    /// Returns the current stream generation (from KV registry).
    pub fn generation(&self) -> Option<u64>;
}
```

### G7. `Pardosa-Generation` NATS header

Add to the first-message header set:

| Header Key | Value | Purpose |
|---|---|---|
| `Pardosa-Generation` | decimal u64 | Stream generation (migration version). Consumers verify they are on the expected generation |

### G8. Compressed + checksummed bare message API

```rust
/// Serialize, compress, then append CRC32 trailer over compressed bytes.
/// Requires `alloc` + (`brotli` or `zstd`) + `checksum`.
pub fn to_bytes_compressed_checksummed<T: Serialize>(
    value: &T,
    compression: Compression,
) -> Result<Vec<u8>, SerError>;

/// Verify CRC32 trailer, decompress, then deserialize.
pub fn from_bytes_compressed_checksummed<T: DeserializeOwned>(
    buf: &[u8],
    compression: Compression,
) -> Result<T, DeError>;
```

Layout:
```
[compressed_size:u32][uncompressed_size:u32][compressed_data][crc32:u32]
                                                              ^^^^^^^^
                                                  CRC32 of all preceding bytes
```

Detects corruption of wire bytes (post-compression), which is the useful
integrity check for network transport.

### G9. Documentation additions to genome.md

Add to Operational Guidance:

1. **CRC32 scope.** CRC32 detects accidental corruption only. It provides no
   protection against adversarial modification. For tamper detection on untrusted
   data, layer an application-level HMAC or signature over the serialized bytes.

2. **`T::SCHEMA_HASH` is a compile-time constant.** The `GenomeSafe` derive macro
   computes an 8-byte xxHash64 schema fingerprint as an associated constant on the
   `GenomeSafe` trait. No runtime cost, no `T::default()` bound. Use it at startup
   for header validation or anywhere a type identity check is needed.

3. **`set_pledged_src_size` requirement for zstd.** When using the zstd streaming
   encoder, always call `set_pledged_src_size(Some(msg_data_size as u64))` before
   writing. Without this, the zstd frame header omits `Frame_Content_Size`, and
   decompression-side size validation becomes a no-op. Assert frame content size
   presence in tests.

4. **Server-side vs. application-level compression.** JetStream's `Compression: s2`
   setting compresses data at the storage layer (transparent to clients). Genome's
   compression operates at the application layer (visible in message bytes). Both
   can be active simultaneously. They serve different purposes: genome compression
   reduces network bandwidth; JetStream s2 reduces disk usage.

5. **`BTreeMap` requirement for deduplication.** When using `Nats-Msg-Id` with
   content-derived IDs, all map types in the serialized value must be `BTreeMap`.
   `HashMap` iteration order is non-deterministic — identical logical values
   produce different byte sequences, defeating deduplication.

6. **Memory budget for concurrent serializations.** Genome's 3-6× memory
   multiplier applies per concurrent serialization. With 100 concurrent
   serializations of 500 KiB messages, peak memory is 150-300 MiB for
   serialization buffers alone. Use `NatsPublisher::with_max_concurrent` or
   application-level semaphores to bound concurrency.

7. **`max_total_elements` tuning.** Default 16M elements. For network-facing
   services processing untrusted data, consider 1M (prevents 384-512 MiB
   allocation from a single crafted message). For trusted internal pipelines,
   the default is adequate.

---

## Pardosa Core Changes (Pre-Phase 2)

Changes to existing Phase 1 code required before Phase 2 begins.

### P1. `Event<T>` — private fields, `event_id`, sentinel precursor

```rust
/// An immutable event in the append-only line.
///
/// GENOME LAYOUT: fields are serialized in declaration order.
/// Changing field order is a breaking change — SCHEMA_HASH will change.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Event<T> {
    event_id: u64,
    timestamp: i64,
    domain_id: DomainId,
    detached: bool,
    precursor: Index,
    domain_event: T,
}

impl<T> Event<T> {
    pub fn new(
        event_id: u64,
        timestamp: i64,
        domain_id: DomainId,
        detached: bool,
        precursor: Index,
        domain_event: T,
    ) -> Self {
        Event { event_id, timestamp, domain_id, detached, precursor, domain_event }
    }

    pub fn event_id(&self) -> u64 { self.event_id }
    pub fn timestamp(&self) -> i64 { self.timestamp }
    pub fn domain_id(&self) -> DomainId { self.domain_id }
    pub fn detached(&self) -> bool { self.detached }
    pub fn precursor(&self) -> Index { self.precursor }
    pub fn domain_event(&self) -> &T { &self.domain_event }
}
```

**`event_id: u64`** — globally monotonic across stream generations. The new
stream's first event continues from the old stream's last `event_id + 1`.
Used as `Nats-Msg-Id` for JetStream publish-side deduplication. Used during
replay to skip already-applied events (idempotent replay).

**`precursor: Index`** — uses `Index::NONE` sentinel instead of `Option<Index>`.
Saves 4 bytes inline + heap indirection per event in genome encoding.

### P2. `Index::NONE` sentinel

```rust
impl Index {
    pub const ZERO: Index = Index(0);
    pub const NONE: Index = Index(u64::MAX);

    /// Create a new index. Panics if `v == u64::MAX` (reserved for `NONE`).
    /// Use `Index::NONE` to construct the sentinel explicitly.
    pub fn new(v: u64) -> Self {
        assert!(v != u64::MAX, "u64::MAX is reserved for Index::NONE — use Index::NONE directly");
        Index(v)
    }

    /// Create an index without validating against the sentinel.
    /// Only for deserialization paths where the value has already been validated.
    pub(crate) fn new_unchecked(v: u64) -> Self {
        Index(v)
    }

    pub fn is_none(self) -> bool {
        self.0 == u64::MAX
    }

    pub fn is_some(self) -> bool {
        self.0 != u64::MAX
    }

    /// Returns the next index, or `IndexOverflow` if at `u64::MAX - 1`
    /// (the last valid position before the sentinel).
    pub fn checked_next(self) -> Result<Index, PardosaError> {
        if self.0 >= u64::MAX - 1 {
            return Err(PardosaError::IndexOverflow);
        }
        Ok(Index(self.0 + 1))
    }
}
```

`u64::MAX` is chosen because it can never be a valid line position — a line
with `u64::MAX` events would require ~147 exabytes of event storage.

`Index::new()` rejects `u64::MAX` to prevent accidental sentinel construction.
`checked_next()` caps at `u64::MAX - 1` so that no valid index arithmetic
can produce the sentinel value.

### P3. `Fiber` — serde derives, bounds-checked advance

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fiber {
    anchor: Index,
    len: u64,
    current: Index,
}

impl Fiber {
    /// Create a new fiber. Returns error if invariants are violated.
    pub fn new(anchor: Index, len: u64, current: Index) -> Result<Fiber, PardosaError> {
        if anchor.is_none() {
            return Err(PardosaError::FiberInvariantViolation(
                "anchor must not be Index::NONE".into(),
            ));
        }
        if current.is_none() {
            return Err(PardosaError::FiberInvariantViolation(
                "current must not be Index::NONE".into(),
            ));
        }
        if len < 1 {
            return Err(PardosaError::FiberInvariantViolation(
                "len must be >= 1".into(),
            ));
        }
        if current.value() < anchor.value() {
            return Err(PardosaError::FiberInvariantViolation(
                "current must be >= anchor".into(),
            ));
        }
        Ok(Fiber { anchor, len, current })
    }

    /// Update fiber after appending a new event at `new_current`.
    /// Returns error if `new_current` is not strictly greater than `current`
    /// or if `new_current` is the sentinel value.
    pub fn advance(&mut self, new_current: Index) -> Result<(), PardosaError> {
        if new_current.is_none() {
            return Err(PardosaError::FiberInvariantViolation(
                "new_current must not be Index::NONE".into(),
            ));
        }
        if new_current.value() <= self.current.value() {
            return Err(PardosaError::FiberInvariantViolation(
                "new_current must be > current".into(),
            ));
        }
        self.current = new_current;
        self.len += 1;
        Ok(())
    }
}
```

Replaces `debug_assert!` with fallible constructor (pardosa.md H1).
Adds bounds check to `advance` (pardosa.md H3).

### P4. `LockedRescuePolicy` enum

```rust
/// Policy for rescuing a Locked fiber.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LockedRescuePolicy {
    /// Old events remain in the deprecated stream's grace period.
    /// The audit trail is the deprecated stream itself.
    PreserveAuditTrail,
    /// Old events will be deleted when the deprecated stream expires.
    /// Caller acknowledges permanent data loss after the grace period.
    AcceptDataLoss,
}
```

Replaces `acknowledge_data_loss: bool`. Communicates that the old stream's
grace period is the only window for recovering history.

### P5. Error taxonomy additions

```rust
#[derive(Debug, thiserror::Error)]
pub enum PardosaError {
    // ... existing variants ...

    #[error("fiber invariant violation: {0}")]
    FiberInvariantViolation(String),

    #[error("serialization failed: {0}")]
    SerializationFailed(String),

    #[error("deserialization failed: {0}")]
    DeserializationFailed(String),

    #[error("migration failed: {0}")]
    MigrationFailed(String),

    #[error("stream not found: {0}")]
    StreamNotFound(String),

    #[error("registry unavailable")]
    RegistryUnavailable,
}
```

`SerializationFailed` and `DeserializationFailed` use `String` rather than
wrapping genome error types directly, keeping the dependency optional.
When the `genome` feature is enabled, `From<pardosa_genome::SerError>` and
`From<pardosa_genome::DeError>` impls convert automatically.

---

## Phase 2 — Dragline + Concurrency

Unchanged from pardosa.md except for the `Event<T>` and `Fiber` changes above.

| # | File | Action |
|---|------|--------|
| 8 | `src/dragline.rs` | `Dragline<T> { line: Vec<Event<T>>, lookup: HashMap<DomainId, (Fiber, FiberState)>, purged_ids: HashSet<DomainId>, next_id: DomainId, next_event_id: u64, migrating: bool }` |
| 9 | `src/dragline.rs` | CRUD: `create`, `update`, `detach`, `rescue(policy: LockedRescuePolicy)` — each validates transition, assigns `event_id` from `next_event_id`, appends to line, updates lookup |
| 10 | `src/dragline.rs` | Reads: `read`, `read_with_deleted`, `list`, `list_with_deleted`, `history`, `read_line` |
| 11 | Concurrency | Wrap in `tokio::sync::RwLock<Dragline<T>>` |
| 12 | Tests | Concurrent read/write, lifecycle end-to-end, purged-ID reuse, `event_id` monotonicity, `Fiber::new` and `advance` error cases |

Changes from original Phase 2:

- `next_event_id: u64` field on `Dragline`. Incremented on every append.
  Initialized from max `event_id` in replay + 1.
- `rescue` takes `LockedRescuePolicy` instead of `bool`.
- All `Fiber::new` calls handle `Result`.
- All `Fiber::advance` calls handle `Result`.
- `precursor` uses `Index::NONE` for first events instead of `None`.
- `verify_precursor_chains()` method: walks all fibers, validates each
  `precursor` points to a valid earlier event with the same `domain_id`.
  O(n) time, called on startup after replay.

---

## Phase 3 — Server API

Unchanged from pardosa.md.

| # | File | Action |
|---|------|--------|
| 13 | `src/lib.rs` | `Server<T>` struct wrapping `RwLock<Dragline<T>>`. Public API methods. Migration-in-progress rejection |
| 14 | Tests | API integration, migration rejection under concurrent load |

---

## Phase 4 — Migration (New-Stream Model)

Replaces pardosa.md Phase 4 entirely.

### Migration lifecycle

```
Phase     Old Stream     New Stream     KV Registry     Server State
─────     ──────────     ──────────     ───────────     ────────────
  0       active         -              → old           serving reads + writes
  1       active (ro)    -              → old           write-lock acquired, writes rejected
  2       active (ro)    created        → old           migration reads old, writes new
  3       active (ro)    populated      → old           new stream fully written
  4       deprecated     active         → new           KV pointer updated, server switches
  5       grace period   active         → new           old stream max_age = grace period
  6       expired        active         → new           NATS server auto-purges old stream
```

Step 1: Acquire write-lock. Set `migrating = true`. Reject all application
writes. Reads continue from the current in-memory `Dragline` (step 1 is
instantaneous — no I/O under lock).

Step 2-3: Release write-lock for the duration of the migration I/O. The
old stream is immutable (single-writer guarantee). Migration reads all events
from the old stream/file. For each fiber, applies the migration policy:

- **Defined fibers**: implicitly kept. Events re-serialized to new stream.
  Schema upcast applied if `T` changes.
- **Detached + Keep**: events re-serialized to new stream.
- **Detached + Purge**: events skipped. DomainId added to `purged_ids`.
- **Detached + LockAndPrune**: last event only, marked as Locked.
- **Locked + Purge**: events skipped. DomainId added to `purged_ids`.

**Reads during steps 2-3:** the write-lock is released, so read operations
proceed against the stale in-memory `Dragline`. This `Dragline` is
self-consistent — it reflects the state at step 1. Stale reads are
acceptable because `migrating = true` already signals to clients that the
system is in transition. Writes are rejected by the `migrating` flag
(checked under read-lock), not by the write-lock itself.

**Index generation-scoping:** `Index` values returned by read APIs during
steps 2-3 are valid only for the current generation. After step 4 (cutover),
the old `Index` values are invalidated — the new `Dragline` has remapped
indices. Consumers must not cache `Index` values across generation boundaries.
Read APIs should be documented accordingly.

New stream receives surviving events with contiguous reindexed `Index` values
starting from 0. `Fiber.anchor`, `Fiber.current`, and event `precursor` values
are remapped via an index translation table. `event_id` values are preserved
(globally monotonic — not reset).

Step 4: Acquire write-lock again. Build new `Dragline` from the new stream
(fast — events are already in memory from the migration pass). Update KV
registry pointer. Set `migrating = false`. Release write-lock.

Step 5: Set `max_age` on the old JetStream stream. Default grace: 7 days.
Within the grace period, operators can rollback by re-pointing the KV
registry to the old stream name.

### Migration function signature

```rust
pub struct MigrationConfig<F> {
    /// Per-fiber migration policies. Fibers not listed are implicitly Kept
    /// (if Defined) or produce an error (if Detached/Locked without policy).
    pub policies: HashMap<DomainId, MigrationPolicy>,

    /// Schema upcast function. Called for each surviving event.
    /// Return `None` to drop an individual event (distinct from fiber-level Purge).
    /// Identity function `|e| Some(e)` when T is unchanged.
    pub upcast: F,

    /// Grace period for the deprecated stream. Default: 7 days.
    pub grace_period: std::time::Duration,

    /// New generation number. Must be > current generation.
    pub generation: u64,
}
```

The upcast function has type `Fn(Event<T_old>) -> Option<Event<T_new>>`.
When `T_old == T_new`, the caller passes `|e| Some(e)`.

When `T_old != T_new`, the migration is a type-level operation — the caller
provides the conversion logic. Pardosa does not constrain how `T` evolves;
it only ensures the structural invariants (fiber chains, index remapping,
`event_id` preservation).

### Genome file migration

When genome file persistence is enabled, migration also writes a new genome
multi-message file:

```rust
// Pseudocode
let mut writer = pardosa_genome::Writer::new()
    .with_compression(compression);

for event in surviving_events {
    writer.push(&event)?;
}

let bytes = writer.finish()?;
std::fs::write(&tmp_path, &bytes)?;
std::fs::rename(&tmp_path, &final_path)?;  // atomic
```

The file is written to a temp path and atomically renamed. If the process
crashes before rename, the incomplete file is discarded on restart.

### Implementation

| # | File | Action |
|---|------|--------|
| 15 | `src/migration.rs` | `MigrationConfig`, migration lifecycle (steps 1-5), index remap table, `event_id` preservation, `purged_ids` updates |
| 16 | `src/migration.rs` | Genome file write (feature-gated behind `genome`) |
| 17 | `src/migration.rs` | JetStream stream creation + population (feature-gated behind `nats`) |
| 18 | `src/migration.rs` | KV registry pointer update (feature-gated behind `nats`) |
| 19 | `src/migration.rs` | Old stream deprecation (`max_age` update, feature-gated behind `nats`) |
| 20 | Tests | See [Test Plan](#migration-tests) |

---

## Phase 5 — Persistence + Genome Integration

Replaces pardosa.md Phase 5.

### Persistence adapter trait

```rust
/// Abstraction over persistence backends.
/// Genome+NATS is the primary implementation; JSON+file is the fallback.
#[async_trait]
pub trait PersistenceAdapter<T>: Send + Sync {
    /// Persist an event. Returns the durable sequence number.
    async fn publish(&self, event: &Event<T>) -> Result<u64, PardosaError>;

    /// Replay all events from the beginning. Returns events in order.
    async fn replay(&self) -> Result<Vec<Event<T>>, PardosaError>;

    /// Get the current stream generation.
    fn generation(&self) -> u64;
}
```

### Genome + NATS implementation

```rust
/// Genome-serialized events over NATS/JetStream.
pub struct GenomePersistence<T> {
    publisher: pardosa_genome::NatsPublisher,
    stream_name: String,
    generation: u64,
    compression: pardosa_genome::Compression,
    _phantom: PhantomData<T>,
    // Schema hash is available at compile time via <Event<T> as GenomeSafe>::SCHEMA_HASH
}
```

**Publish path:**

1. Serialize `Event<T>` via `pardosa_genome::to_bytes`.
2. Compress if configured.
3. Set `Nats-Msg-Id` to `pardosa-{stream_name}-{event_id}`.
4. Set `Nats-Expected-Last-Subject-Sequence` if fencing is enabled.
5. Publish via `NatsPublisher`.
6. On ACK: return durable sequence.
7. On timeout/error: return `PardosaError::NatsUnavailable`.

**Replay path:**

1. Create JetStream consumer with `DeliverPolicy::All`.
2. Read first message — parse `Pardosa-*` headers (including
   `Pardosa-Generation` for generation validation).
3. Validate schema hash against `<Event<T> as GenomeSafe>::SCHEMA_HASH`.
4. Deserialize all events via `pardosa_genome::from_bytes`.
5. Deduplicate by `event_id` (skip events already seen).
6. Return ordered event list.

**Schema hash computation:**

```rust
// At compile time, via GenomeSafe derive:
// <Event<MyDomainEvent> as GenomeSafe>::SCHEMA_HASH
// is an 8-byte xxHash64 fingerprint, available as a const.
```

The schema hash is stored in NATS first-message headers and genome file headers.
Consumers validate on connect.

### JetStream stream configuration

```
Stream: PARDOSA_{name}_g{generation}
  Subjects: pardosa.{name}.events
  Retention: Limits
  Discard: DiscardNew        # Never silently drop old events
  Storage: File
  Replicas: 3                # Production (1 for dev)
  DenyDelete: true           # Append-only
  DenyPurge: true            # No accidental purge (migration creates new stream)
  AllowRollup: false         # Not needed — migration creates new stream
  DuplicateWindow: 2m        # Match Nats-Msg-Id dedup window
  MaxMsgSize: 1MB            # Match NatsPublisher default
  Compression: s2            # Server-side storage compression (separate from genome)
```

Note: `AllowRollup` is `false` because the new-stream-per-migration model
eliminates the need for in-stream rollup. Each stream is append-only for
its entire lifetime. `DenyPurge` is `true` for the same reason.

### KV registry

```
Bucket: PARDOSA_REGISTRY
  History: 10               # Keep last 10 pointer values for debugging
  TTL: 0                    # No expiration on registry entries

Keys:
  {name}.active   → "{generation}:{stream_name}"   (e.g., "2:PARDOSA_orders_g2")
```

Single-key design ensures atomicity — no split state between generation and
stream name. Parsed by splitting on the first `:`. The generation prefix
enables numeric comparison without parsing the stream name.

On startup, the server reads `{name}.active` from the KV bucket to discover
which JetStream stream to replay from. If the key does not exist, this is a
fresh deployment — the server creates generation 1 and writes
`1:PARDOSA_{name}_g1`.

### Implementation

| # | File | Action |
|---|------|--------|
| 21 | `Cargo.toml` | Add `pardosa-genome` (optional, behind `genome` feature). Add `async-nats` (optional, behind `nats` feature). Move `serde_json` behind optional `json` feature |
| 22 | `src/persistence.rs` | `PersistenceAdapter` trait definition |
| 23 | `src/persistence/genome_nats.rs` | `GenomePersistence<T>` implementation. Publish, replay, schema ID validation, `Nats-Msg-Id` dedup |
| 24 | `src/persistence/json_file.rs` | `JsonFilePersistence<T>` — fallback for development/debugging (feature-gated behind `json`) |
| 25 | `src/registry.rs` | NATS KV registry operations: get/set `{name}.active` pointer, watch for changes, parse `generation:stream_name` format |
| 26 | `src/dragline.rs` | Inject `PersistenceAdapter`. Publish-then-apply on mutations. Replay on startup. `verify_precursor_chains()` post-replay |
| 27 | Tests | See [Test Plan](#persistence-tests) |

---

## Phase 6 — NATS Consumer Lifecycle

New phase. Handles consumer cutover across migration generations.

### Consumer cutover protocol

When the KV registry pointer changes from `g{N}` to `g{N+1}`:

1. Consumer detects the change via KV watch.
2. Consumer finishes processing any in-flight message from the old stream.
3. Consumer records its last-processed `event_id`.
4. Consumer creates a new JetStream subscription on the new stream.
5. Consumer reads `Pardosa-*` headers from the new stream's first message.
6. Consumer validates schema hash and `Pardosa-Generation`.
7. Consumer scans the new stream for the first event with
   `event_id > last_processed_event_id` and begins processing from there.

**Resumption by `event_id`, not `DeliverPolicy`.** The consumer uses
`DeliverPolicy::All` on the new stream and skips events with
`event_id <= last_processed_event_id`. This avoids the data-loss window
inherent in `DeliverPolicy::New` (events appended between migration
completion and consumer cutover would be missed). The cost is scanning
migrated events that the consumer already processed from the old stream,
but this is bounded and occurs only once per migration.

For consumers that were offline during the migration (last-known generation
< current generation), the scan starts from `event_id = 0` (full replay).
The consumer detects this by comparing the KV generation with its
last-known generation.

### Implementation

| # | File | Action |
|---|------|--------|
| 28 | `src/consumer.rs` | `PardosaConsumer<T>` — wraps `NatsConsumer` with KV watch, generation tracking, cutover logic |
| 29 | Tests | See [Test Plan](#consumer-tests) |

---

## Error Taxonomy (Revised)

Complete error enum incorporating all phases:

```rust
#[derive(Debug, thiserror::Error)]
pub enum PardosaError {
    // State machine
    #[error("invalid transition: state {state:?} + action {action:?}")]
    InvalidTransition { state: FiberState, action: FiberAction },

    // Fiber integrity
    #[error("fiber invariant violation: {0}")]
    FiberInvariantViolation(String),

    // Identity
    #[error("domain ID {0:?} is not in Purged state — cannot reuse")]
    IdNotPurged(DomainId),

    #[error("domain ID {0:?} already exists")]
    IdAlreadyExists(DomainId),

    #[error("fiber not found for domain ID {0:?}")]
    FiberNotFound(DomainId),

    #[error("index overflow")]
    IndexOverflow,

    #[error("domain ID counter overflow")]
    DomainIdOverflow,

    #[error("event ID counter overflow")]
    EventIdOverflow,

    // Server state
    #[error("migration in progress — application operations rejected")]
    MigrationInProgress,

    // Persistence
    #[error("NATS connection unavailable")]
    NatsUnavailable,

    #[error("serialization failed: {0}")]
    SerializationFailed(String),

    #[error("deserialization failed: {0}")]
    DeserializationFailed(String),

    // Migration
    #[error("migration failed: {0}")]
    MigrationFailed(String),

    #[error("generation {requested} is not greater than current {current}")]
    InvalidGeneration { current: u64, requested: u64 },

    #[error("detached fiber {0:?} has no migration policy")]
    MissingMigrationPolicy(DomainId),

    // Registry
    #[error("stream not found in registry: {0}")]
    StreamNotFound(String),

    #[error("registry unavailable")]
    RegistryUnavailable,

    #[error("schema mismatch: expected {expected}, got {actual}")]
    SchemaMismatch { expected: String, actual: String },

    // Precursor integrity
    #[error("precursor chain broken at event_id {event_id}: precursor index {precursor:?} not found")]
    BrokenPrecursorChain { event_id: u64, precursor: Index },
}
```

**Removed from original `PardosaError`:** `AcknowledgmentRequired` — replaced
by `LockedRescuePolicy` enum (P4). The rescue API now takes the policy enum
directly; no separate "acknowledgment required" error is needed.
```

---

## Cargo.toml (Revised)

```toml
[package]
name = "pardosa"
version = "0.2.0"
edition.workspace = true
rust-version.workspace = true
authors.workspace = true
description = "EDA storage layer implementing fiber semantics"
license = "MIT"

[lib]
name = "pardosa"
path = "src/lib.rs"

[features]
default = ["genome"]
genome = ["dep:pardosa-genome"]
json = ["dep:serde_json"]
nats = ["dep:async-nats", "dep:tokio", "dep:async-trait"]
brotli = ["pardosa-genome?/brotli"]       # passthrough to genome
zstd = ["pardosa-genome?/zstd"]           # passthrough to genome

[dependencies]
serde = { workspace = true }
thiserror = { workspace = true }
pardosa-genome = { path = "../pardosa-genome", optional = true }
serde_json = { workspace = true, optional = true }
async-nats = { version = "0.40", optional = true }
tokio = { version = "1", features = ["sync"], optional = true }
async-trait = { version = "0.1", optional = true }

[dev-dependencies]
proptest = { workspace = true }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
serde_json = { workspace = true }
```

---

## Architecture (Revised)

```
pardosa/
├── Cargo.toml
├── src/
│   ├── lib.rs                  # Server<T>, public API re-exports
│   ├── error.rs                # PardosaError enum
│   ├── event.rs                # Index (with NONE), DomainId, Event<T>
│   ├── fiber.rs                # Fiber (with serde, fallible constructor)
│   ├── fiber_state.rs          # FiberState, FiberAction, TRANSITIONS, transition()
│   ├── dot.rs                  # DOT visualization
│   ├── dragline.rs             # Dragline<T>, CRUD, reads, precursor verification
│   ├── migration.rs            # MigrationConfig, new-stream migration lifecycle
│   ├── persistence.rs          # PersistenceAdapter trait
│   ├── persistence/
│   │   ├── genome_nats.rs      # GenomePersistence<T> (genome + nats features)
│   │   └── json_file.rs        # JsonFilePersistence<T> (json feature, dev/debug)
│   ├── registry.rs             # NATS KV registry (nats feature)
│   └── consumer.rs             # PardosaConsumer<T> with KV watch (nats feature)
```

---

## Design Invariants (Revised)

1. **Atomic publish-then-apply.** Each mutation = one NATS publish (or one
   genome file append). Apply to in-memory state only after durable ACK.

2. **Idempotent publish via `event_id`.** `event_id` is globally monotonic
   across stream generations. Used as `Nats-Msg-Id` for server-side dedup.
   Replay skips events with `event_id` already in the in-memory `Dragline`.

3. **Single-writer per stream.** One `Server<T>` instance per NATS stream.
   Enforced by convention and optionally by `Nats-Expected-Last-Subject-Sequence`
   fencing. Multiple instances produce divergent state with no reconciliation.

4. **Immutable old streams.** Migration never mutates old JetStream streams
   or genome files. Old stream is read-only from migration step 1 onward.
   Crash recovery: discard incomplete new stream, retry from old.

5. **KV registry as the atomic cutover point.** The KV pointer update in
   step 4 of migration is the single point of truth for which stream is
   active. All consumers and the server itself discover the active stream
   via the registry.

6. **`event_id` continuity across generations.** New stream's first
   `event_id` = old stream's last `event_id` + 1. No resets. Enables
   global total ordering of events across the entire lifetime of a pardosa
   instance, including across migrations.

7. **State machine as data.** `TRANSITIONS` table drives runtime logic and
   DOT visualization. No separate encoding.

8. **Precursor chain integrity.** `verify_precursor_chains()` is called
   post-replay on startup. Each event's `precursor` (when not `Index::NONE`)
   must point to a valid earlier event with the same `domain_id`.

9. **Genome field order is frozen.** `Event<T>`, `Fiber`, `Index`,
   `DomainId` — field declaration order must not change. The genome binary
    layout and `SCHEMA_HASH` depend on declaration order. Reordering fields
   is a breaking change equivalent to a schema migration.

10. **`Index` values are generation-scoped.** An `Index` value is valid
    only within the stream generation that produced it. Migration remaps
    all indices to a new contiguous range. Code that caches `Index` values
    (e.g., for deferred reads) must invalidate the cache on generation
    change. `event_id` is the cross-generation stable identifier.

11. **`Index::NONE` is the only sentinel.** `u64::MAX` is permanently
    reserved. `Index::new()` rejects it. `checked_next()` caps at
    `u64::MAX - 1`. No valid index arithmetic can produce the sentinel.

---

## Test Plan

### Pre-Phase 2 tests (core changes)

| Test | Priority |
|------|----------|
| `Event::new` constructs with all fields, accessors return correct values | High |
| `Event` serde roundtrip via genome `to_bytes` / `from_bytes` (feature-gated) | High |
| `Event` serde roundtrip via `serde_json` (feature-gated) | High |
| `Event` genome `SCHEMA_HASH` is stable across calls | High |
| `Event` genome `SCHEMA_HASH` changes when field is added/removed/reordered | High |
| `Index::NONE` sentinel: `is_none()` returns true, `is_some()` returns false | High |
| `Index::ZERO` is not `is_none()` | High |
| `Index::NONE.checked_next()` returns `IndexOverflow` | High |
| `Index::new(u64::MAX)` panics (sentinel guard) | High |
| `Index::new(u64::MAX - 1)` succeeds (last valid position) | High |
| `Index::new(0).checked_next()` at `u64::MAX - 2` succeeds, at `u64::MAX - 1` returns `IndexOverflow` | High |
| `Fiber::new` with `len = 0` returns `FiberInvariantViolation` | High |
| `Fiber::new` with `current < anchor` returns `FiberInvariantViolation` | High |
| `Fiber::new` with `anchor = Index::NONE` returns `FiberInvariantViolation` | High |
| `Fiber::new` with `current = Index::NONE` returns `FiberInvariantViolation` | High |
| `Fiber::advance` with `new_current <= current` returns error | High |
| `Fiber::advance` with `new_current = Index::NONE` returns error | High |
| `Fiber::advance` with `new_current > current` succeeds | High |
| `Fiber` serde roundtrip via genome (feature-gated) | Medium |
| `LockedRescuePolicy` serde roundtrip | Medium |

### Phase 2 tests (dragline)

| Test | Priority |
|------|----------|
| Create assigns monotonic `event_id` | High |
| Create → Update → Detach lifecycle, `event_id` increments each time | High |
| `verify_precursor_chains` passes on valid dragline | High |
| `verify_precursor_chains` fails on broken chain (manually corrupted precursor) | High |
| Concurrent create with same `DomainId` — one succeeds, one gets `IdAlreadyExists` | High |
| Purged-ID reuse: Create → Detach → Migrate(Purge) → Create with same DomainId | High |
| `DomainId` counter overflow at `Server` level | Medium |
| `event_id` counter overflow → `EventIdOverflow` | Medium |
| `proptest`: arbitrary state-action sequences preserve valid states | Medium |
| `proptest`: arbitrary event sequences maintain fiber chain integrity | Medium |

### Migration tests

| Test | Priority |
|------|----------|
| Migration with all-Defined fibers: events re-serialized, indices contiguous | High |
| Migration with Purge: purged fiber events absent from new stream, `purged_ids` updated | High |
| Migration with LockAndPrune: only last event survives, state = Locked | High |
| `event_id` preserved across migration (not reset) | High |
| New stream's first `event_id` > old stream's last `event_id` (for post-migration appends) | High |
| `generation` must be > current, else `InvalidGeneration` | High |
| `generation == current` (boundary) → `InvalidGeneration` | High |
| Detached fiber without migration policy → `MissingMigrationPolicy` | High |
| Migration with upcast function: `T_old → T_new` transformation applied | High |
| Migration crash simulation: incomplete new stream discarded, old stream intact | High |
| Concurrent migration attempts: second migration rejected while first in progress | High |
| Genome file migration: atomic write (temp + rename) | Medium |
| Migration of empty dragline (0 fibers, 0 events) | Medium |
| Multi-generation cycle: `g1 → g2 → g3`, `event_id` continuity across all | Medium |
| `Purged→Create→Detach→Purge→Create` multi-generation reuse cycle | Medium |

### Persistence tests

| Test | Priority |
|------|----------|
| `GenomePersistence::publish` serializes via genome, publishes to NATS | High |
| `GenomePersistence::replay` deserializes all events, correct order | High |
| Replay deduplicates by `event_id` (inject duplicate) | High |
| Replay with schema hash mismatch → `SchemaMismatch` | High |
| Publish-failure rollback: in-memory state unchanged on `NatsUnavailable` | High |
| `Nats-Msg-Id` set to `pardosa-{stream}-{event_id}` on publish | High |
| Publish timeout with event persisted in JetStream (ACK-lost): replay deduplicates | High |
| `verify_precursor_chains()` runs post-replay and catches broken chain | High |
| KV registry: get/set `{name}.active` pointer, parse `generation:stream_name` | High |
| KV registry unavailable → `RegistryUnavailable` | Medium |
| Genome file persistence: write multi-message file, read back, all events match | Medium |
| Genome compressed persistence: zstd round-trip (feature-gated) | Medium |
| Genome compressed persistence: brotli round-trip (feature-gated) | Medium |

### Consumer tests

| Test | Priority |
|------|----------|
| Consumer discovers active stream from KV registry | High |
| Consumer detects KV pointer change, switches to new stream | High |
| Consumer offline during migration: replays new stream from beginning | High |
| Consumer online during migration: resumes via `event_id` tracking (skips already-processed events) | High |
| Consumer cutover with events appended between migration and cutover: no data loss | High |
| `Pardosa-Generation` header mismatch → error | Medium |
| Consumer with `Pardosa-Compression: zstd` decompresses events | Medium |

### Cross-crate integration tests

| Test | Priority |
|------|----------|
| `Event<String>` genome roundtrip: `to_bytes` → `from_bytes`, zero-copy `&str` in domain_event when borrowed | High |
| `Event<T>` with `HashMap` field in `T`: document non-determinism, verify `BTreeMap` alternative is deterministic | High |
| `<Event<Foo> as GenomeSafe>::SCHEMA_HASH` is consistent between debug and release builds | High |
| `<Event<Foo> as GenomeSafe>::SCHEMA_HASH` ≠ `<Event<Bar> as GenomeSafe>::SCHEMA_HASH` | High |
| Full lifecycle: create events → persist to genome file → read back → verify dragline state identical | High |
| Full lifecycle: create events → persist to NATS → replay → verify dragline state identical | High |
| Migration: genome file g1 → g2 with schema upcast, read g2, verify | Medium |
| Migration: NATS stream g1 → g2, consumer cutover, verify | Medium |

---

## Open Questions (Updated)

Previous open questions with updated status:

| # | Question | Status |
|---|----------|--------|
| 1 | Event ID format: `u64` vs `Uuid` | **Resolved**: `u64` monotonic. Single-writer (invariant 3) makes global uniqueness trivial. `Uuid` adds 16 bytes per event for no benefit |
| 2 | Migration downtime tolerance | **Resolved**: new-stream model allows reads from old stream during migration. Write downtime = step 1 (lock) through step 4 (cutover). Proportional to event count but I/O-bound, not lock-held |
| 3 | Schema evolution frequency | **Resolved**: stream generation IS the schema version. No per-event version field. Migration handles cross-schema reads at the application layer via the upcast function |
| 4 | Expected line size | **Open**: determines whether `Vec<Event<T>>` is viable. Migration compaction bounds growth. Monitor events-between-migrations as an operational metric |
| 5 | Multi-instance future | **Resolved**: single-writer is a hard constraint. Documented in invariant 3. Multi-instance requires leader election — fundamentally different architecture |

New open questions:

| # | Question | Notes |
|---|----------|-------|
| 6 | Grace period default: 7 days sufficient? | Determines rollback window. Shorter = less storage. Longer = safer rollback. Configurable per-migration via `MigrationConfig::grace_period` |
| 7 | Consumer cutover delivery policy | **Resolved**: `event_id`-based resumption. Consumer uses `DeliverPolicy::All` on new stream, skips events with `event_id <= last_processed_event_id`. Avoids `DeliverPolicy::New` data-loss window |
| 8 | Genome dictionary compression: when to implement? | Header field reserved (G3). Implementation deferred. Valuable for small events (<4 KiB). Evaluate after initial deployment with real payload sizes |
| 9 | `PersistenceAdapter` trait: sync or async? | Currently async. If genome-file-only deployments want sync, need a separate sync trait or `block_on` wrapper. Async is correct for NATS but heavyweight for file-only |
| 10 | Audit trail stream: separate NATS stream? | Deferred from original plan. With new-stream model, the deprecated stream IS the audit trail during the grace period. Permanent audit requires a separate stream or log sink |
