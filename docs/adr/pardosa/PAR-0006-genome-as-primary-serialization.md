# PAR-0006. Genome as Primary Serialization

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: PAR-0003

## Context

The initial design used `serde_json` for all serialization. JSON is
human-readable and widely supported, but has significant costs for an
event storage system:

- No zero-copy reads — every deserialization allocates.
- No schema fingerprinting — type mismatches are silent.
- No compression integration — requires a separate layer.
- Larger wire size than binary formats.

pardosa-genome provides fixed-layout binary serialization with zero-copy reads,
compile-time schema hashing (xxHash64), optional zstd compression, and full
serde integration via `#[derive(GenomeSafe)]`.

## Decision

pardosa-genome replaces `serde_json` as the primary serialization format.
JSON is retained behind an optional `json` feature flag for debugging,
human-readable output, and configuration.

| Path | Format | Use case |
|------|--------|----------|
| NATS publish/subscribe | Genome bare message | Hot path, real-time events |
| Genome file | Genome multi-message file | Snapshots, cold storage, migration source |
| Debug / config | JSON via `serde_json` | Logging, human inspection |

The dependency is gated:

```toml
[features]
default = ["genome"]
genome = ["dep:pardosa-genome"]
json = ["dep:serde_json"]
```

Compression features (`brotli`, `zstd`) are passthrough to genome.

R1 [5]: Use pardosa-genome as the primary serialization format for
  NATS publish and genome file storage paths
R2 [6]: Retain JSON behind an optional json feature flag for debugging
  and human-readable output only
R3 [5]: Gate the genome dependency and compression features via Cargo
  feature flags in the pardosa crate manifest

## Alternatives Considered

**rkyv** — Closest zero-copy Rust-native competitor. Rejected for three
capabilities genome requires that rkyv cannot provide:

1. *Schema-migration-aware file format.* Genome's file header, trailing index,
   and per-message checksums support the new-stream migration model (PAR-0005)
   with message deletion during migration. rkyv has no file format — it
   serializes individual values into byte buffers with no higher-level
   structure for multi-message storage or selective deletion.
2. *Canonical encoding.* Same value → identical bytes, enabling `Nats-Msg-Id`
   content-based deduplication (PAR-0007). rkyv does not guarantee deterministic
   encoding — HashMap ordering, padding bytes, and resolver ordering can vary.
3. *xxHash64 schema fingerprint in wire format.* O(1) type identity check
   before any field access. rkyv has no schema versioning — a version mismatch
   produces unsound reads or opaque deserialization failures.

Lessons adopted from rkyv:

- *Validate during decode, not as separate pass.* rkyv's `check_archived_root`
  is an O(n) upfront walk; genome validates inline during deserialization,
  which is strictly cheaper for full-deserialize workloads. See
  [GEN-0006](../genome/GEN-0006-zero-copy-deserialization-with-forbid-unsafe-code.md).
- *Unaligned reads are sufficient.* rkyv 0.8 added an `unaligned` feature flag,
  acknowledging that natural alignment is unnecessary on modern x86-64 and
  ARMv8. Genome uses `from_le_bytes` on byte slices everywhere — no alignment
  requirements, no `unsafe`. See
  [GEN-0012](../genome/GEN-0012-little-endian-wire-encoding-no-pointer-casts.md).
- *`#![forbid(unsafe_code)]` is validated by rkyv's safety history.* rkyv's
  relative pointer implementation fails under Miri's Stacked Borrows model
  (rkyv#436). Genome avoids this class of issues entirely.

Lessons explicitly rejected:

- *Mirror types (`ArchivedFoo`).* rkyv generates a parallel type hierarchy for
  zero-copy access. Ergonomic cost is high — archived types spread through the
  entire codebase, every function must be generic over owned vs. archived.
  For 100–500 byte event structs with mostly scalar fields, the performance
  gain over serde-based deserialization is negligible. Genome's serde-native
  approach means event types work unchanged with JSON, bincode, postcard, or
  any future serde backend.
- *Relative pointers.* rkyv uses offsets from the pointer's own position for
  position-independent archives. Events are standalone — never embedded inside
  other events. Absolute offsets from buffer start enable safe
  `&buf[offset..offset+len]` slice operations with no `unsafe`.
- *Full struct zero-copy.* rkyv zero-copies entire struct trees via pointer
  cast. Genome zero-copies only `&str` and `&[u8]` via serde's
  `visit_borrowed_str`/`visit_borrowed_bytes`. For small events where
  variable-length data (strings, byte slices) dominates the copy cost, partial
  zero-copy is the 80/20 solution without mirror types.

**FlatBuffers** — Genome's offset-based layout is inspired by FlatBuffers.
Rejected because: requires external `.fbs` schema files and codegen (genome
uses serde derives), vtables add read-path branching (genome's fixed layout
has zero branching), no serde integration, and cross-language support is not
needed (Rust-only, see
[GEN-0031](../genome/GEN-0031-rust-only-cross-language-read-deferred.md)).

**Cap'n Proto** — Same codegen and vtable objections as FlatBuffers. Cap'n
Proto has a canonical mode close to genome's canonical encoding, but requires
inline defaults for schema evolution — overhead genome doesn't need.

**bincode** — Simplest serde-native binary format. No zero-copy reads (full
deserialization to owned types), no schema fingerprinting, no compression
integration, no file format. Genome types already work with bincode via serde
trait compatibility — usable as a fallback for non-hot-path serialization.

**postcard** — Smallest wire size of any serde format (varint encoding,
minimal framing). No zero-copy, no schema fingerprinting. Viable for
non-hot-path serialization: debug snapshots, human-inspectable dumps,
embedded or constrained targets. Genome types work with postcard via serde —
dual-format serialization requires no code changes to domain event types.

## Consequences

- **Positive:** Zero-copy reads for event deserialization — no allocation
  for `&str` and `&[u8]` fields.
- **Positive:** Compile-time schema hash validates type identity at
  deserialization time. Type confusion across services is detected.
- **Positive:** Integrated compression reduces NATS bandwidth and file size.
- **Negative:** Binary format is not human-readable. Debugging requires the
  `json` feature or a genome inspection tool.
- **Negative:** `GenomeSafe` derive adds a build dependency on the
  `pardosa-derive` proc-macro crate.
