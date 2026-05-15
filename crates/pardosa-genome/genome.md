# pardosa-genome — Design & Implementation Plan

Binary serialization format combining FlatBuffers' zero-copy read performance with RON's
full algebraic data model. Standard serde with a lightweight `GenomeSafe` marker derive.

## Table of Contents

1. [Motivation](#motivation)
2. [Design Principles](#design-principles)
3. [Landscape Analysis](#landscape-analysis)
4. [Data Model](#data-model)
5. [Binary Format Specification](#binary-format-specification)
6. [Serialization Algorithm](#serialization-algorithm)
7. [Deserialization Algorithm](#deserialization-algorithm)
8. [Verification](#verification)
9. [Serde Integration](#serde-integration)
10. [Compression](#compression)
11. [Schema Export](#schema-export)
12. [Feature Flags & `#[no_std]`](#feature-flags--no_std)
13. [Public API](#public-api)
14. [Crate Structure](#crate-structure)
15. [Error Catalog](#error-catalog)
16. [Implementation Steps](#implementation-steps)
17. [Test Plan](#test-plan)
18. [Determinism](#determinism)
19. [Safety Contract](#safety-contract)
20. [Operational Guidance](#operational-guidance)
21. [Limitations & Non-Goals](#limitations--non-goals)
22. [Quality Tooling](#quality-tooling)
23. [Future Scope (v2)](#future-scope-v2)

---

## Motivation

No existing Rust crate combines serde-native serialization with zero-copy reads:

| Crate       | Serde-native | Zero-copy reads | Full algebraic types | Codegen burden |
|-------------|:------------:|:---------------:|:--------------------:|:--------------:|
| rkyv        | ✗ (own traits) | ✓             | ✓                    | Heavy (Archive + custom derive) |
| flatbuffers | ✗ (generated) | ✓              | ✗ (no tuples, limited enums) | Heavy (flatc codegen) |
| bincode 2   | optional     | ✗              | ✓                    | Medium (own traits) |
| postcard    | ✓            | ✗              | ✓                    | None |
| **pardosa-genome** | **✓** | **✓†**        | **✓**                | **Minimal‡** |

pardosa-genome fills this gap: standard `#[derive(Serialize, Deserialize)]` plus a
lightweight `#[derive(GenomeSafe)]` marker trait. Zero-copy `&'de str` and `&'de [u8]`
on uncompressed reads. No mirror types, no external schema files. Genome files embed
the Rust type definition in the header for self-describing inspection.

- **†** Zero-copy applies to uncompressed messages. Compressed messages require owned
  types (`DeserializeOwned`) because decompression allocates a new buffer. For network
  workloads where compression is typical, the primary benefit is fixed-layout
  random-access reads, not zero-copy string borrowing.
- **‡** `GenomeSafe` is a marker trait with no methods. It enforces deterministic
  serialization at compile time (rejects `HashMap`, `HashSet`, `#[serde(untagged)]`,
  `#[serde(tag)]`, `#[serde(flatten)]`, `#[serde(skip_serializing_if)]`).
  It also enables compile-time schema hash computation and schema export.

---

## Design Principles

1. **Correctness over everything.** Every offset bounds-checked. Every string UTF-8 validated.
   No `unsafe`. Verification available for untrusted data.
2. **Read performance.** Zero-copy string/byte access on uncompressed messages. Inline scalars
   at natural alignment. All reads via `from_le_bytes` — no pointer casts, no alignment
   requirement on the buffer. Compressed messages require owned types but retain fixed-layout
   random-access read patterns.
3. **Serde-native with compile-time safety.** Standard `serde::Serializer` /
   `serde::Deserializer<'de>`. Works with any `#[derive(Serialize, Deserialize)]` type.
   The additional `#[derive(GenomeSafe)]` marker trait enforces deterministic serialization
   at compile time (rejects `HashMap`, `HashSet`, `#[serde(untagged)]`, `#[serde(tag)]`,
   `#[serde(flatten)]`, `#[serde(skip_serializing_if)]`) and enables schema export. No mirror
   types, no external schema files, no code generation beyond these two derives.
4. **RON data model.** All of Rust's algebraic types: enums with data, tuples, newtypes,
   `Option`, `char`, non-string map keys, unit structs.
5. **Fixed layout.** No schema evolution, no vtables. Struct layout is determined entirely by
   the Rust type's serde representation. Maximum read speed at the cost of wire compatibility
   across type changes. A compile-time schema hash detects incompatible types at
   deserialization time.
6. **Self-describing files.** Genome files embed the Rust type definition as plain UTF-8
   text in the file header. A developer can inspect a file and understand its structure
   without the original source code. The Rust type IS the schema — the `GenomeSafe` derive
   macro generates the embedded source text from the type definition at compile time.

---

## Landscape Analysis

### Why not rkyv?

rkyv achieves total zero-copy by creating `Archived*` mirror types. Users work with
`ArchivedFoo`, not `Foo`. Requires `#[derive(Archive, Serialize, Deserialize)]` (rkyv's
traits, not serde's). Every type needs a custom implementation. pardosa-genome avoids this
by using serde's visitor model for navigation — the Rust type IS the schema.

### Why not FlatBuffers?

FlatBuffers requires `.fbs` schema files and the `flatc` code generator. The generated Rust
code is verbose and non-idiomatic. Enums are limited to unions (no tuple variants, no struct
variants). No maps. No tuples. No `Option` as a first-class type. pardosa-genome adopts
FlatBuffers' offset-based layout and alignment strategy while supporting RON's full type system
through serde.

### Why not postcard?

Postcard is serde-native (the gold standard for ergonomics) but uses varint encoding — every
read requires sequential parsing from the buffer start. No field skipping, no zero-copy struct
access. pardosa-genome matches postcard's serde ergonomics while adding FlatBuffers-class
read performance.

### Why not bincode?

Bincode 2.x moved to its own `Encode`/`Decode` traits with serde as a second-class citizen.
No alignment. Sequential format. Effectively unmaintained (3.0 is a tombstone release).

---

## Data Model

pardosa-genome supports the full serde data model, which maps 1:1 to RON's type system:

| Serde Type | RON Syntax | pardosa-genome Representation |
|------------|------------|-------------------------------|
| `bool` | `true` / `false` | 1 byte: `0x00` / `0x01` |
| `i8`..`i64` | `-42` | LE, naturally aligned |
| `i128` | `-42` | 16 bytes LE, 8-byte aligned |
| `u8`..`u64` | `42` | LE, naturally aligned |
| `u128` | `42` | 16 bytes LE, 8-byte aligned |
| `f32` | `3.14` | LE IEEE 754 (exact NaN bits preserved) |
| `f64` | `3.14` | LE IEEE 754 (exact NaN bits preserved) |
| `char` | `'a'` | 4 bytes LE u32 (validated on read) |
| `String` / `&str` | `"hello"` | 4B stub: `[offset:u32]`, heap: `[len:u32][UTF-8 data]` |
| `&[u8]` / `Vec<u8>` | (bytes) | 4B stub: `[offset:u32]`, heap: `[len:u32][raw data]` |
| `Option::None` | `None` | 4B stub: `[offset:u32 = 0xFFFFFFFF]` |
| `Option::Some(v)` | `Some(v)` | 4B stub: `[offset:u32]`, value in heap |
| `()` / unit struct | `()` / `UnitName` | 0 bytes |
| Newtype struct | `Meters(42.0)` | Transparent: inner type's layout |
| Tuple | `(1, 2, 3)` | Elements inline with alignment padding |
| Sequence / `Vec<T>` | `[1, 2, 3]` | 4B stub: `[offset:u32]`, heap: `[count:u32][elements]` |
| Map | `{ key: val }` | 4B stub: `[offset:u32]`, heap: `[count:u32][entries]` |
| Named struct | `Player(name: "Ada", hp: 100)` | Fields inline in declaration order |
| Enum (unit) | `North` | 8B: `[discriminant:u32][offset:u32 = 0]` |
| Enum (newtype) | `Some(42)` | 8B: `[discriminant:u32][offset:u32]`, data in heap |
| Enum (tuple) | `Move(1.0, 2.0)` | 8B: `[discriminant:u32][offset:u32]`, data in heap |
| Enum (struct) | `Attack(target: "g", dmg: 5)` | 8B: `[discriminant:u32][offset:u32]`, data in heap |

### Non-string map keys

Following RON, map keys can be any serializable type — not just strings. Map keys must
implement both `GenomeSafe` and `GenomeOrd` (ADR-033). `GenomeOrd` restricts keys to owned
value types with deterministic, total ordering. Map entries are serialized in `BTreeMap`
iteration order (see [Determinism](#determinism)).

### NaN handling

Exact bit patterns are preserved. No canonicalization. `f64::NAN.to_bits()` round-trips
exactly.

### Char validation

On deserialization, char values are validated: `u32 ≤ 0x10FFFF` and not in
`0xD800..=0xDFFF` (surrogate range). Invalid values produce `DeError::InvalidChar`.

---

## Binary Format Specification

### Overview: Two Wire Formats

| Format | API | Starts with | Use case |
|--------|-----|-------------|----------|
| Bare message | `encode` / `decode` | `format_version: u16` + `schema_hash: u64` + `algo: u8` + `msg_data_size: u32` | IPC, network, embedding |
| File | `Writer` / `Reader` | `"PGNO"` magic | Persistent storage, multi-message |

No auto-detection. Consumers must use the correct API.

**Forward compatibility contract**: The first 2 bytes of any pardosa-genome bare message
are always `format_version: u16 LE`. The first 4 bytes of any pardosa-genome file are
always `"PGNO"` magic, followed by `format_version: u16 LE` at bytes 4–5. Future format
versions will never change the position, size, or encoding of these fields. Readers must
read these bytes first and reject unknown versions with `DeError::VersionMismatch` (bare)
or `FileError::UnsupportedVersion` (file) before interpreting any subsequent fields.

A file is an array of messages sharing the same schema. A single message is the degenerate
case (array of one).

### File Layout

```
┌──────────────────────────────────┐  offset 0
│  File Header (32 bytes)          │
├──────────────────────────────────┤  offset 32
│  Schema Block (variable, opt.)   │  UTF-8 Rust source, padded to 8B
├──────────────────────────────────┤  offset 32 + padded(schema_size)
│  Message 0 (size-prefixed)       │
├──────────────────────────────────┤
│  Message 1 (size-prefixed)       │
├──────────────────────────────────┤
│  ...                             │
├──────────────────────────────────┤
│  Message N-1                     │
├──────────────────────────────────┤  = index_offset
│  Message Index (N × 24 bytes)    │
├──────────────────────────────────┤
│  File Footer (32 bytes)          │
└──────────────────────────────────┘
```

### File Header (32 bytes, all LE)

```
Offset  Size  Field           Description
──────  ────  ──────────────  ──────────────────────────────────────────
 0      4     magic           ASCII "PGNO" (0x50 0x47 0x4E 0x4F)
 4      2     format_version  Format version (starts at 1)
 6      2     flags           See "Header Flags" below
 8      8     schema_hash     Compile-time schema fingerprint (u64 LE, xxHash64)
16      4     dict_id         Reserved for future zstd dictionaries; must be 0 in v1
20      1     page_class      Page class hint (0–3). See "Page Classes" below
21      4     schema_size     Byte count of embedded schema source (u32 LE, 0 = none)
25      7     reserved        Must be all zeros
```

When `schema_size > 0`, a schema block of that many UTF-8 bytes follows the header
at offset 32. The block is padded to an 8-byte boundary with zeros. Messages begin at
offset `32 + pad_to_8(schema_size)`. When `schema_size == 0`, messages begin at offset 32
(backward compatible).

The schema block contains the Rust type definition as plain text, generated by the
`GenomeSafe` derive macro's `SCHEMA_SOURCE` constant. It serves human inspection — a
developer can read the type structure from the file without access to the original source.
The schema hash remains the authoritative compatibility check; the embedded source text
is informational.

### Single Schema Per File

All messages in a file share the same schema. Schema never changes inside a file — a new file is written as part of a pardosa migration if schema needs to evolve. Enum variants provide in-schema flexibility (adding a variant is not a schema break; the discriminant-based layout handles it).

Schema identity is enforced at two levels: the 8-byte schema hash in the file header
establishes the expected type for all messages in the file, and each bare message carries
its own schema hash for independent type validation. The pardosa migration model enforces
one schema per file.

### Header Flags

```
Bit     Description
──────  ──────────────────────────────────────────
 0-2    compression_algo: 000 = none, 001 = zstd (others reserved)
 3-15   reserved (must be zero; reject if non-zero)
```

Compression state is derived from `compression_algo ≠ 000` — no separate "compressed"
flag. In v1, only `001` (zstd) is defined. Readers must reject unknown `compression_algo`
values with `FileError::UnsupportedCompression`.

### Bare Message (variable size)

Bare messages are self-contained and self-describing. Used for IPC, network transport,
and embedding in other formats. The `algo` byte is always present, enabling auto-detection
of compression without out-of-band coordination.

**Uncompressed bare message** (`algo = 0x00`):

```
Offset  Size  Field           Description
──────  ────  ──────────────  ──────────────────────────────────────────
 0      2     format_version  Format version, u16 LE (starts at 1)
 2      8     schema_hash     Compile-time schema fingerprint (u64 LE, xxHash64)
10      1     algo            Compression algorithm (u8: 0x00 = none)
11      4     msg_data_size   Byte count of data that follows (u32 LE)
15      ?     inline_data     Root type's inline fields, naturally aligned
 ?      ?     heap_data       Strings, vec elements, option/enum data
                              Padded to 8-byte boundary at the end
```

**Compressed bare message** (`algo ≥ 0x01`):

```
Offset  Size  Field               Description
──────  ────  ──────────────────  ──────────────────────────────────────────
 0      2     format_version      Format version (u16 LE)
 2      8     schema_hash         Compile-time schema fingerprint (u64 LE, xxHash64)
10      1     algo                Compression algorithm (u8: 0x01 = zstd)
11      4     compressed_size     Byte count of compressed data (u32 LE)
15      4     msg_data_size       Original uncompressed payload size (u32 LE)
19      ?     compressed_data     Compressed message payload
```

The first 11 bytes (through `algo`) are identical for both formats. `decode` reads the
`algo` byte to determine the layout of the remaining fields — no caller coordination
required.

The `schema_hash` is an 8-byte fingerprint computed at compile time by the `GenomeSafe`
derive macro from the type's serde structure (field names, types, ordering, root type
name) using xxHash64. On deserialization, the hash in the message is compared against
the expected type's hash. Mismatch produces `DeError::SchemaMismatch` — preventing
silent type confusion in multi-service deployments. The 8-byte width pushes the birthday
bound to ~4 billion types, making accidental collisions practically impossible.

All u32 offsets within a message are **absolute from the start of inline_data** (byte 15 of
an uncompressed bare message; byte 0 of the data region in file messages). Special offset
values:

- **`Option<T>`**: offset `0xFFFFFFFF` means `None`. Any other offset means `Some(v)` at
  that position. The sentinel `0xFFFFFFFF` is chosen because it is an invalid offset — no
  single message can reach 4 GiB (see [size limit](#4-gib-per-message-limit)).
- **Enum unit variants**: offset field is 0 (variant data is empty, offset ignored on read).
- **String/bytes with len=0**: a `[len:u32 = 0]` heap entry is always allocated. The offset
  points to a valid heap position containing `0x00000000`. No special-case for empty strings.
- **Vec/Map with count=0**: a `[count:u32 = 0]` heap entry is always allocated. The offset
  points to a valid heap position containing `0x00000000`. No special-case for empty containers.
- **Offset to `buf.len()`**: valid when the referenced item has 0 inline size (e.g.,
  `Option<()>` pointing to the end of the buffer). A 0-byte read at `buf.len()` is a no-op.

Offset 0 always means "data at position 0" — it is never overloaded as a sentinel.
This eliminates ambiguity in hex dumps and debugging tools.

Maximum message size: 4 GiB (u32 offset limit).

### File Message (variable size)

File messages omit `format_version` and `schema_hash` — these are stored once in the
file header. This saves 10 bytes per message and enables schema validation before reading
any messages.

**Uncompressed file message** (`compression_algo = 000`):

```
Offset  Size  Field           Description
──────  ────  ──────────────  ──────────────────────────────────────────
 0      4     msg_data_size   Byte count of data that follows (u32 LE)
 4      ?     data            Message payload (inline_data + heap_data)
```

**Compressed file message** (`compression_algo ≠ 000`):

```
Offset  Size  Field               Description
──────  ────  ──────────────────  ──────────────────────────────────────────
 0      4     msg_data_size       Original uncompressed payload size (u32 LE)
 4      4     compressed_size     Byte count of compressed data (u32 LE)
 8      ?     compressed_data     Compressed message payload
```

The compression algorithm is determined by the file header's `compression_algo` field,
not per-message. All messages in a file use the same algorithm. `msg_data_size` serves
as the pre-allocation hint for decompression (capped by
`DecodeOptions::max_uncompressed_size`). Decompression produces the original message
payload: `[inline_data][heap_data]`.

Checksum coverage: for both compressed and uncompressed file messages, the per-message
xxHash64 checksum in the index covers the entire stored record (from `msg_data_size` through end of
data/compressed_data).

### Message Index (N × 24 bytes, all LE)

```
Per message:
Offset  Size  Field           Description
──────  ────  ──────────────  ──────────────────────────────────────────
 0      8     offset          Absolute file offset to message's msg_data_size field (u64)
 8      4     size            Total stored record size in bytes (u32)
12      4     reserved        Must be zero (alignment padding)
16      8     checksum        xxHash64 of stored record bytes (u64 LE)
```

The per-message checksum is mandatory. It covers the stored record bytes (starting at the
message's `msg_data_size` field through end of data). xxHash64 is always verified before
deserialization.

### File Footer (32 bytes, all LE)

```
Offset  Size  Field           Description
──────  ────  ──────────────  ──────────────────────────────────────────
 0      8     index_offset    Absolute file offset to message index (u64)
 8      8     message_count   Number of messages (u64)
16      4     reserved        Must be all zeros
20      4     footer_magic    ASCII "PGNO" (validation sentinel)
24      8     checksum        xxHash64 of footer bytes [0..24) (u64 LE)
```

xxHash64 (seed 0) is used for all file-level integrity checksums — both per-message
and footer. xxHash64 is a non-cryptographic hash; see ADR-016 for the full threat model.

### Edge Case: 0 Messages

A file with 0 messages is valid. The message index has 0 entries (0 bytes). `index_offset`
equals the file offset immediately after the header (byte 32). The footer follows directly
after the (empty) index. Total file size: 32 (header) + 0 (messages) + 0 (index) + 32
(footer) = 64 bytes.

### Type Inline Sizes

Every type has a fixed inline size determined by its serde representation:

| Type | Inline Size (bytes) | Alignment (bytes) | Inline Content |
|------|:-------------------:|:-----------------:|----------------|
| `bool` | 1 | 1 | `0x00` or `0x01` |
| `u8` / `i8` | 1 | 1 | Value |
| `u16` / `i16` | 2 | 2 | LE |
| `u32` / `i32` | 4 | 4 | LE |
| `u64` / `i64` | 8 | 8 | LE |
| `u128` / `i128` | 16 | 8 | LE |
| `f32` | 4 | 4 | LE IEEE 754 |
| `f64` | 8 | 8 | LE IEEE 754 |
| `char` | 4 | 4 | LE u32 (Unicode scalar) |
| `String` / `&str` | 4 | 4 | `[offset:u32]` → heap: `[len:u32][data]` |
| `&[u8]` / bytes | 4 | 4 | `[offset:u32]` → heap: `[len:u32][data]` |
| `Option<T>` | 4 | 4 | `[offset:u32]` (0xFFFFFFFF = None) |
| `Vec<T>` / seq | 4 | 4 | `[offset:u32]` → heap: `[count:u32][elements]` |
| `Map<K,V>` | 4 | 4 | `[offset:u32]` → heap: `[count:u32][entries]` |
| `()` / unit struct | 0 | 1 | Nothing |
| Struct | Σ(fields + padding) | max(field alignments) | Fields in decl order |
| Enum | 8 | 4 | `[discriminant:u32][offset:u32]` |
| Tuple | Σ(elements + padding) | max(element alignments) | Elements in order |
| Newtype | inner size | inner alignment | Transparent |

### Alignment Rules

1. Before writing each field, pad cursor to the field's natural alignment.
2. Padding bytes are always `0x00`.
3. Struct alignment = maximum alignment of its fields (minimum 1).
4. `i128`/`u128` align to 8 (not 16) to avoid excessive padding on 32-bit targets.
5. All heap entries are aligned to 4 bytes.

### Heap Region

The heap starts immediately after all inline data. Heap items are self-describing:
each variable-length item carries its own length or count prefix (FlatBuffers-style).
For honestly-serialized data, heap regions do not overlap by construction — each item's
boundaries are determined by its own prefix, not by external metadata. For untrusted input,
overlapping regions are possible but do not cause undefined behavior due to the read-only,
no-unsafe design. Overlapping heap regions are not rejected during verification.

Heap items:

- **Strings**: `[len:u32][UTF-8 bytes]`. No null terminator. Length prefix is the byte
  count. UTF-8 validated on read.
- **Byte slices**: `[len:u32][raw bytes]`. Length prefix is the byte count.
- **Vec elements**: `[count:u32][elem₀][elem₁]...` — count prefix followed by contiguous
  elements, each at its natural alignment.
- **Map entries**: `[count:u32][key₀][val₀][key₁][val₁]...` — count prefix followed by
  key-value pairs. Each key and each value individually aligned to its own natural
  alignment (not the pair as a unit).
  Serialized in container iteration order (see [Determinism](#determinism)).
- **Option::Some(v)**: Inner value's layout (no length prefix — the type determines the
  size). For zero-size inner types (`Option<()>`), the offset points to a valid position
  but 0 bytes are read. The `None` sentinel (`0xFFFFFFFF`) is never a valid heap offset.
- **Enum variant data**: Variant's inline layout (no length prefix). Unit variants have
  0 bytes (offset field is set to 0 on write and ignored on read; non-zero offset on a
  unit variant produces `DeError::NonZeroPadding`).

---

## Serialization Algorithm

Two-phase approach. Correctness over write speed.

### Memory Usage

**Peak memory during serialization is approximately 1× the final message size** — the
output `Vec<u8>` buffer. The two-pass approach avoids building an intermediate tree.

For large payloads, split data across multiple messages using `Writer`. Each message's
buffer is independent.

### Two-Pass Architecture

Serialization uses two serde traversals of the input value, avoiding intermediate
allocation:

**Pass 1 — Sizing (`SizingSerializer`)**: Implements `serde::Serializer`. Walks the type
tree computing total buffer size (inline region + heap region, recursively). Cost: one
serde traversal, O(depth) stack frames, zero heap allocation.

**Pass 2 — Writing (`WritingSerializer`)**: Implements `serde::Serializer`. Pre-allocates
the exact buffer. Writes inline data sequentially from position 0. For each heap reference
(String, Vec, Map, Option::Some, Enum with data), writes the 4-byte offset stub inline and
appends the heap item (with its length/count prefix) at the heap cursor.

Heap items are written in breadth-first order: all items directly referenced by the inline
region first, then items referenced by those heap items, and so on. This ensures all
offsets are forward-pointing into the heap region.

```
Two-pass pseudocode:

// Pass 1: compute sizes
let sizing = SizingSerializer::new();
value.serialize(&sizing)?;
let total_size = sizing.inline_size() + sizing.heap_size();

// Pass 2: write bytes
let mut buf = Vec::with_capacity(total_size);
let writer = WritingSerializer::new(&mut buf, sizing.inline_size());
value.serialize(&writer)?;
```

**Termination**: Both passes traverse the type tree, which is finite by construction.
Pass 1 visits every node exactly once. Pass 2 writes every node exactly once. The
breadth-first heap ordering terminates because each level processes nodes that are strictly
closer to leaves than the previous level, and leaf nodes (scalars, strings, bytes) produce
no further heap items.

**Double traversal safety**: `value.serialize()` is called twice. This is safe for any
type that implements `Serialize` correctly (immutable observation of the value). Types
that mutate state during serialization (extremely rare, arguably a bug) may produce
inconsistent sizing and writing. The `WritingSerializer` tracks actual bytes written;
after writing, if the actual length differs from the pre-computed size, serialization
returns `SerError::InternalSizingMismatch` in all build profiles (debug and release).
An additional `debug_assert!` fires in debug builds for stack trace diagnostics. The
`GenomeSafe` restricted type subset eliminates structural divergence for all types that
pass the derive macro.

**Compile-time and runtime detection of unsupported serde attributes:**

The `GenomeSafe` derive macro catches the following at compile time via syntactic analysis
of the derive input:
- `#[serde(untagged)]` — serde silently bypasses variant serialization. `GenomeSafe`
  rejects this at compile time. For types that bypass `GenomeSafe` (manual `Serialize`
  impls, transitive dependencies), `verify_roundtrip` in CI is the defense-in-depth.
  See [Limitations](#limitations--non-goals).
- `#[serde(tag = "...")]` (internally tagged enums) — serde emits `serialize_map` with a
  type discriminator field. This is **not compatible** with pardosa-genome's fixed
  discriminant-based enum layout. Only externally tagged enums (serde's default, no
  attribute) are supported. `GenomeSafe` rejects `#[serde(tag)]` at compile time.
- `#[serde(tag = "...", content = "...")]` (adjacently tagged enums) — serde emits
  `serialize_struct` with tag and content fields. `GenomeSafe` rejects at compile time.
- `#[serde(flatten)]` — causes serde to call `serialize_map` instead of
  `serialize_struct`. `GenomeSafe` rejects at compile time. Runtime detection in the
  serializer remains as defense-in-depth for types that bypass `GenomeSafe`.
- `#[serde(skip_serializing_if)]` — conditionally omits fields at runtime based on a
  predicate, causing data-dependent layout breakage in a fixed-layout format. `GenomeSafe`
  rejects at compile time.

**Invariant**: All offsets point into `[inline_size..buffer.len()]`. Verified by
`debug_assert!` during writing (diagnostic). The deserializer independently enforces this
via the backward offset check on every read.

**Invariant**: Message total size ≤ `u32::MAX`. Checked after writing — produces
`SerError::MessageTooLarge` on overflow.

---

## Deserialization Algorithm

Zero-copy. Zero allocation for borrow-only types. The hot path.

### Deserializer State

```rust
struct MessageDeserializer<'de> {
    buf: &'de [u8],    // entire message data (after msg_data_size) — NEVER re-sliced
    cursor: usize,     // current read position (absolute within message)
}
```

**Critical invariant**: `buf` always refers to the full message buffer. Sub-deserializers
for heap data (Options, Vecs, Enums, Maps) share the same `buf` reference with a different
`cursor` value. This ensures all u32 offsets are interpreted consistently as absolute
positions within the message. **Never re-slice `buf`** — doing so would invalidate nested
absolute offsets.

### Read Operations

Every read follows the same pattern:

1. **Align** cursor to the type's natural alignment.
2. **Bounds-check**: `cursor + size ≤ buf.len()` (using `checked_add`).
3. **Read** bytes via `from_le_bytes` (no pointer casts, no alignment requirement on buffer).
4. **Advance** cursor by the inline size.

### Type-Specific Behavior

**Scalars** (`deserialize_u32`, `deserialize_f64`, etc.):
Read inline bytes, convert via `from_le_bytes`, call `visitor.visit_*`.

**Char** (`deserialize_char`):
Read u32 LE. Validate: `≤ 0x10FFFF` and not in `0xD800..=0xDFFF`. Convert via
`char::from_u32`. Call `visitor.visit_char`.

**String** (`deserialize_str`):
Read `[offset:u32]` from inline (4 bytes). Jump to heap at `offset`, read `[len:u32]`.
Bounds-check `offset + 4 + len ≤ buf.len()` (with `checked_add`). Validate UTF-8:
`core::str::from_utf8(&buf[offset+4..offset+4+len])`. Call `visitor.visit_borrowed_str`.
**Zero-copy: the returned `&'de str` points directly into the input buffer.**

**Bytes** (`deserialize_bytes`):
Same as String but no UTF-8 validation. Call `visitor.visit_borrowed_bytes`.

**Option** (`deserialize_option`):
Read `[offset:u32]`. If `0xFFFFFFFF` → `visitor.visit_none()`. Otherwise, create a
sub-deserializer with the **same `buf`** and `cursor = offset` →
`visitor.visit_some(&mut sub_de)`. Original cursor advances by 4.

**Seq/Vec** (`deserialize_seq`):
Read `[offset:u32]` from inline (4 bytes). Jump to heap at `offset`, read `[count:u32]`.
Create sub-deserializer with the **same `buf`** and `cursor = offset + 4`. Return
`SeqAccess` that yields `count` elements, each deserialized from the sub-deserializer's
advancing cursor. Original cursor advances by 4.

**Map** (`deserialize_map`):
Same as Seq but yields key-value pairs via `MapAccess`. Original cursor advances by 4.

**Struct** (`deserialize_struct`):
Visitor calls `next_key_seed` / `next_value_seed` for each field. Each field is deserialized
at the current cursor position. Cursor advances through all fields in order. No offset
indirection — struct fields are inline.

**Enum** (`deserialize_enum`):
Read `[discriminant:u32][offset:u32]`. Return `EnumAccess` with variant index =
discriminant. For unit variants (offset = 0): `visitor.visit_unit()`. For data variants:
create sub-deserializer with the **same `buf`** and `cursor = offset`, deserialize variant
data. Original cursor advances by 8.

**Tuple** (`deserialize_tuple`):
Elements deserialized inline in order (same as struct fields but positional).

**Newtype** (`deserialize_newtype_struct`):
Transparent — deserialize inner type at current cursor.

**Unit** (`deserialize_unit`, `deserialize_unit_struct`):
No bytes consumed. Call `visitor.visit_unit()`.

### Deserialization Limits

To prevent denial-of-service via crafted inputs, deserialization enforces configurable
resource limits:

```rust
pub struct DecodeOptions {
    /// Maximum nesting depth for recursive types (structs, enums, options, vecs,
    /// maps, newtype structs).
    /// Default: 128. Exceeding this produces `DeError::DepthLimitExceeded`.
    /// Depth increments on entry to: struct, enum variant, option, seq, map, and
    /// newtype struct deserialization — and decrements on exit. Newtype structs are
    /// transparent in layout (0 extra bytes) but NOT transparent in recursion depth.
    pub max_depth: usize,

    /// Maximum total number of elements deserialized across all sequences and maps.
    /// Counting unit: each `SeqAccess::next_element` call counts as 1; each
    /// `MapAccess::next_entry` call counts as 1 (not 2). For a `BTreeMap<String,
    /// Vec<u32>>` with 100 entries, each containing 10 elements: map entries = 100,
    /// inner vec elements = 100×10 = 1000, total = 1100.
    /// Determined by page class (see below). Checked BEFORE calling `Vec::with_capacity`
    /// to prevent OOM from crafted count fields (see serde-rs/serde#744).
    pub max_total_elements: usize,

    /// Maximum uncompressed message size in bytes. Applies to the `msg_data_size`
    /// field in compressed messages — rejects before allocation to prevent DoS via
    /// crafted size headers. Default: 268_435_456 (256 MiB).
    /// Also validated against the `Frame_Content_Size` in the zstd frame header, when
    /// present. If the frame content size exceeds this limit, decompression is rejected
    /// before allocation.
    pub max_uncompressed_size: usize,

    /// Maximum message size in bytes for uncompressed bare messages. `msg_data_size`
    /// is checked against this limit before deserialization begins. Prevents unbounded
    /// processing on crafted bare messages with `msg_data_size = u32::MAX`.
    /// Default: 268_435_456 (256 MiB, same as `max_uncompressed_size`).
    pub max_message_size: usize,

    /// Maximum zstd window log. Limits decompressor memory to 2^max_zstd_window_log bytes.
    /// Default: 22 (4 MiB). Zstd frames requesting larger windows are rejected with
    /// `DeError::DecompressionFailed`. Only relevant with the `zstd` feature.
    /// This prevents decompression bomb attacks via crafted window sizes (up to 3.75 TiB
    /// per the zstd spec).
    pub max_zstd_window_log: u32,

    /// When true (default), reject buffers with trailing bytes after the message.
    /// When false, extra bytes after the message are ignored. Compressed bare decode
    /// APIs reject trailing bytes unconditionally regardless of this setting.
    pub reject_trailing_bytes: bool,
}

/// Page classes define per-message element budgets. The page class is stored in
/// the file header (`page_class` byte at offset 20) and conventionally echoed
/// in the file extension (`.PAGE0`, `.PAGE1`, etc.).
///
/// **Both sources are optimization hints, not security boundaries.** The file
/// header is not signed — an attacker who controls the file can set any page
/// class. The reader treats the stored page class as a default for
/// `max_total_elements` and may override it with caller-supplied limits.
/// If a message exceeds the reader's element limit, deserialization aborts
/// with `DeError::TotalElementsExceeded`.
///
/// Formula: `256 × 16^N` where N is the page class number.
///
/// | Extension | N | Max Elements | Scale |
/// |-----------|---|-------------|-------|
/// | `.PAGE0`  | 0 | 256         | Small config |
/// | `.PAGE1`  | 1 | 4,096       | Moderate struct |
/// | `.PAGE2`  | 2 | 65,536      | Standard dataset |
/// | `.PAGE3`  | 3 | 1,048,576   | Large batch |
///
/// PAGE3 is the maximum page class. A `.PAGE0` file can contain many messages,
/// but each individual message has at most 256 elements. This lets the reader
/// free memory aggressively between messages.
pub enum PageClass {
    Page0,  // 256 elements
    Page1,  // 4,096 elements
    Page2,  // 65,536 elements
    Page3,  // 1,048,576 elements
}

impl PageClass {
    pub const fn max_elements(&self) -> usize {
        match self {
            Self::Page0 => 256,
            Self::Page1 => 4_096,
            Self::Page2 => 65_536,
            Self::Page3 => 1_048_576,
        }
    }
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            max_depth: 128,
            max_total_elements: PageClass::Page0.max_elements(), // 256
            max_uncompressed_size: 268_435_456,
            max_message_size: 268_435_456,
            max_zstd_window_log: 22,
            reject_trailing_bytes: true,
        }
    }
}

impl DecodeOptions {
    pub fn for_page_class(page: PageClass) -> Self {
        Self {
            max_total_elements: page.max_elements(),
            ..Self::default()
        }
    }
}
```

`decode` and `decode_with_options` use `DecodeOptions::default()` and a custom
configuration respectively. Setting a field to `usize::MAX` effectively disables that limit.
Use `DecodeOptions::for_page_class(PageClass::Page3)` for large-batch workloads.

**Compressed message decompression** checks `msg_data_size` against
`max_uncompressed_size` *before* allocating the decompression buffer. This prevents a
crafted message with `msg_data_size = u32::MAX` and a few bytes of compressed data from
causing a 4 GiB allocation. The zstd `Frame_Content_Size` from the zstd frame header
is also validated when present; `ZSTD_d_maxWindowLog` is set to cap decompressor memory.
`DecodeOptions` limits (depth, elements) are enforced post-decompression during
deserialization of the uncompressed message.

**Post-decompression verification**: After decompression, the decompressed buffer size is
verified against `msg_data_size`. The decompressed message is then parsed normally.
Any mismatch produces `DeError::BufferTooSmall` or `DeError::TrailingBytes`.

A `Vec<T>` with `count = u32::MAX` in the heap prefix will be rejected by the element limit
**before** calling `Vec::with_capacity` — the count is checked against
`max_total_elements` first, then allocation occurs only if the check passes. This prevents
OOM from crafted count fields (the class of bug documented in serde-rs/serde#744). Depth
tracking increments on entry to struct, enum variant, option, seq, map, and newtype struct
deserialization, and decrements on exit. Newtype structs are transparent in layout but add
a stack frame per nesting level — without depth tracking, a chain of 200+ newtypes causes
stack overflow.

### Trailing Bytes Policy

`decode` reads exactly `format_version (2 bytes) + schema_hash (8 bytes) + algo (1 byte) + msg_data_size (4 bytes) + msg_data_size` bytes from the buffer for uncompressed bare messages (15-byte header + data). For compressed bare messages, the header is 19 bytes (`compressed_size` and `msg_data_size` replace the single `msg_data_size`). By default (`DecodeOptions::reject_trailing_bytes = true`), any trailing bytes after the message produce `DeError::TrailingBytes`. This can be disabled via `decode_with_options` for callers that deliberately embed genome bytes inside larger buffers. Compressed bare decode APIs reject trailing bytes unconditionally.

---

## Verification

All deserialization always verifies. There is no unverified `decode` or standalone `verify` function — every call to `decode` performs the full set of structural checks.

### Checks Performed (on every deserialization)

1. Format version matches expected.
2. Schema hash matches the target type's compile-time hash.
3. Buffer length ≥ minimum size for the root type.
4. All u32 offsets point within `[0..buf.len()]` (excluding the `0xFFFFFFFF` None sentinel).
5. All `offset + len` computations do not overflow (checked arithmetic).
6. Backward offset check: offsets from inline stubs must point into the heap region
   (`offset ≥ inline_size(root_type)`). This is a structural hygiene measure — the actual
   cycle and DoS defense is provided by depth and element limits (see below).
   **Exception**: enum unit variant offset field is treated as padding — must be
   `0x00000000` (`NonZeroPadding` error otherwise), backward offset check does not apply.
7. String data is valid UTF-8.
8. Char values are valid Unicode scalar values.
9. Bool values are `0x00` or `0x01`.
10. Alignment padding bytes are `0x00`.
11. Message size matches `msg_data_size` (for bare messages).
12. Trailing bytes rejected by default (configurable via `DecodeOptions::reject_trailing_bytes`).
13. File header magic, version, footer magic, xxHash64 checksum (for file format).
14. Per-message xxHash64 in index entries (mandatory, always verified for file format).
15. Post-decompression: decompressed size matches `msg_data_size` (for compressed messages).
16. Reserved bytes in file header (bytes 25–31) and file footer (bytes 16–19) are all zeros.
17. `compressed_size` bounds-checked: `message_offset + header_size + compressed_size ≤ buf.len()`.
    Violation produces `DeError::BufferTooSmall`.
18. Message index validation (file format): each offset ≥ 32, offset + record_size ≤
    index_offset, offsets monotonically non-decreasing, ranges non-overlapping.
19. All offset arithmetic widened to u64 before comparison to prevent overflow on 32-bit
    platforms.
20. `message_count × 24` overflow check: `message_count.checked_mul(24).is_some()` and
    `index_offset + message_count × 24 + 32 ≤ file_size`.

**Cycle and DoS defense**: Depth and element limits (`DecodeOptions::max_depth`,
`max_total_elements`) are the primary defense against crafted inputs that exploit recursive
or repetitive type structures. The backward-offset check (item 6) prevents heap offsets
from pointing into the inline region but does not prevent all cycles within the heap —
the depth and element limits terminate any such path.

### Usage

```rust
// Deserialize with default options (always verifies — no unverified path)
let foo: Foo = pardosa_genome::decode(buf)?;

// Deserialize with custom limits
let config = DecodeOptions::for_page_class(PageClass::Page3);
let foo: Foo = pardosa_genome::decode_with_options(buf, &config)?;
```

`decode` is always safe on arbitrary input — it bounds-checks every offset, validates
UTF-8, checks padding zeros, validates bool values, verifies the schema hash, and rejects
backward offsets. There is no unverified path; verification runs on every decode and is
branch-predicted-away on well-formed input.

---

## Serde Integration

### Serializer (write path)

Two `serde::Serializer` implementations share the same structural logic:

- **`SizingSerializer`** — computes buffer size without allocation.
- **`WritingSerializer`** — writes bytes into a pre-allocated buffer.

Both handle serde's type methods identically:

```
serialize_bool       → 1 byte inline
serialize_i8..i128   → LE inline
serialize_u8..u128   → LE inline
serialize_f32, f64   → LE inline
serialize_char       → 4 bytes inline
serialize_str        → 4B offset inline; heap: [len:u32][utf8_bytes]
serialize_bytes      → 4B offset inline; heap: [len:u32][raw_bytes]
serialize_none       → 4B offset inline (0xFFFFFFFF)
serialize_some       → 4B offset inline; heap: inner value
serialize_unit       → 0 bytes
serialize_unit_struct       → 0 bytes
serialize_unit_variant      → 8B [discriminant:u32][offset:u32 = 0]
serialize_newtype_struct    → transparent (inner type's layout)
serialize_newtype_variant   → 8B [discriminant:u32][offset:u32]; heap: inner
serialize_seq               → 4B offset inline; heap: [count:u32][elements]
serialize_tuple             → elements inline with alignment
serialize_tuple_struct      → elements inline with alignment
serialize_tuple_variant     → 8B [discriminant:u32][offset:u32]; heap: elements
serialize_map               → 4B offset inline; heap: [count:u32][entries]
serialize_struct            → fields inline in declaration order
serialize_struct_variant    → 8B [discriminant:u32][offset:u32]; heap: fields
```

### Deserializer (read path)

Implements `serde::Deserializer<'de>` for `&'a mut MessageDeserializer<'de>`.

Key trait implementations:
- `SeqAccess<'de>` — for Vec/seq deserialization, tracks remaining count.
- `MapAccess<'de>` — for Map deserialization, tracks remaining count.
- `EnumAccess<'de>` — for enum deserialization, provides variant index.
- `VariantAccess<'de>` — for enum variant data deserialization.

### Zero-Copy Borrowing

When `T: Deserialize<'de>` contains `&'de str` or `&'de [u8]`, the deserialized value
borrows directly from the input buffer. This is serde's standard zero-copy mechanism
(same as `serde_json::from_slice`).

```rust
#[derive(Deserialize)]
struct Entry<'a> {
    name: &'a str,        // zero-copy: points into buffer
    data: &'a [u8],       // zero-copy: points into buffer
    count: u32,           // copied (4 bytes)
}

let buf: &[u8] = /* ... */;
let entry: Entry<'_> = pardosa_genome::decode(buf)?;
assert!(entry.name.as_ptr() >= buf.as_ptr());  // proves zero-copy
```

---

## Compression

Optional per-message zstd compression, gated behind the `zstd` feature flag.
Enabled in the file format via the `flags` field in the file header. Bare messages
use the `encode` API with `EncodeOptions { compression: Compression::Zstd { .. } }`.
Only one compression algorithm is active per file — mixed compression within a single
container is not supported.

Brotli compression is deferred to v2. Zstd covers the primary use case (real-time
network/MLOps pipelines) with ≈4× faster decompression and ≈25× faster compression
than brotli at comparable ratios.

### Design

Each message is independently compressed. The message index stores the compressed
size, enabling random access without decompressing the entire file. Decompression
is transparent to the deserializer — the `Reader` decompresses each message into a
temporary buffer before passing it to `MessageDeserializer`.

### Compressed Message Layout

Compressed layouts are defined in §Bare Message and §File Message above. Summary:

- **Bare messages**: always self-describing via the `algo` byte at offset 10.
  Compressed bare: `[format_version:u16][schema_hash:u64][algo:u8][compressed_size:u32][msg_data_size:u32][compressed_data]` (19-byte header).
- **File messages**: compression algorithm from file header `compression_algo` field.
  Compressed file: `[msg_data_size:u32][compressed_size:u32][compressed_data]`.

Decompression produces the original message payload: `[inline_data][heap_data]`.
`msg_data_size` enables pre-allocation of the decompression buffer (capped by
`DecodeOptions::max_uncompressed_size`), avoiding realloc during streaming decompression.

**Empty messages**: a message with `msg_data_size=0` is valid. Compressed empty input
produces a few bytes of compressed data. Decompression yields an empty buffer, and
`decode` on the result returns `DeError::BufferTooSmall`.
To represent "no data," use an empty multi-message file (0 messages) or `Option::None` in
the payload type.

### Zero-Copy Impact

**Decompression allocates a new buffer.** Zero-copy `&'de str` borrows point into
the decompressed buffer, not the original input. The `Reader` manages decompressed buffer
lifetimes internally. For bare messages, the caller must hold the decompressed buffer:

```rust
// Uncompressed: borrows from input
let msg: Entry<'_> = decode(input_buf)?;  // msg borrows input_buf

// Compressed: decode auto-detects from algo byte, returns owned values
let msg: OwnedEntry = decode(compressed_buf)?;
// Types with &'de str or &'de [u8] cannot be used with compressed messages
```

### Zstd Configuration

Level 3 is the default (best speed/ratio tradeoff for real-time workloads). Levels 1–19
are supported; levels 20–22 (`--ultra`) are allowed but the `quality_hint` in the file
header clamps to 15 (informational only — decompressors derive parameters from the
zstd frame header).

**Content size is always written** into the zstd frame header
(`ZSTD_c_contentSizeFlag = 1`). This enables pre-allocation validation: the decompressor
reads `Frame_Content_Size` from the frame and validates it against
`DecodeOptions::max_uncompressed_size` *before* allocating. This prevents
decompression bomb attacks where a small zstd payload expands to terabytes.

**Window log cap**: decompression enforces `ZSTD_d_maxWindowLog = 22` (4 MiB) to bound
decompressor memory. Zstd frames requesting larger windows (up to 3.75 TiB per spec)
are rejected with `DeError::DecompressionFailed`. This is critical for untrusted data —
CVEs in Movement Network, urllib3, and OTel Collector demonstrate real-world zstd bomb
exploitation via crafted window sizes.

**Dictionary compression** (future consideration): zstd dictionaries can improve
compression 2–5× on small messages (<4 KiB) with repeated schema structures. Not
included in v1 but the file header has reserved bytes for future use.

Total decompressor memory per message: ≈4 MiB (capped window) + output buffer size.

### Performance Expectations

On binary serialized data (struct layouts with numeric fields and string pointers):

| Level | Compression speed | Decompression speed | Size reduction |
|-------|:-----------------:|:-------------------:|:--------------:|
| 1     | ≈500 MB/s         | ≈1500 MB/s          | ≈45–55%        |
| 3     | ≈400 MB/s         | ≈1500 MB/s          | ≈50–60%        |
| 9     | ≈80 MB/s          | ≈1500 MB/s          | ≈55–65%        |
| 19    | ≈5 MB/s           | ≈1500 MB/s          | ≈60–70%        |

Padding bytes (alignment zeros) compress extremely well. Messages with many small
aligned fields will see better ratios than tightly packed scalar arrays.

---

## Schema Export

The `GenomeSafe` derive macro enables two schema capabilities:

### Compile-Time Schema Hash

An 8-byte schema fingerprint is computed at compile time from the type's serde structure:
field names, field types, type ordering, enum variant names and shapes, and the root type
name. The hash is embedded in every serialized message and verified on deserialization.

```rust
trait GenomeSafe {
    /// 8-byte xxHash64 fingerprint of the type's serde structure.
    const SCHEMA_HASH: u64;

    /// Human-readable Rust source text for file header embedding.
    const SCHEMA_SOURCE: &'static str;
}
```

#### `GenomeOrd` — Deterministic Map Key Marker (ADR-033)

Types used as `BTreeMap` keys or `BTreeSet` elements must additionally implement
`GenomeOrd`:

```rust
/// Marker: type has a deterministic, total Ord suitable for BTreeMap keys.
trait GenomeOrd: GenomeSafe {}
```

`GenomeOrd` is implemented for owned value types with deterministic ordering:
`bool`, integer primitives, `char`, `()`, `String`, `Option<T: GenomeOrd>`,
`[T: GenomeOrd; N]`, and tuples (1–16 elements). Runtime wrappers (`Box`, `Arc`, `Cow`)
and borrowed types (`&str`, `&[u8]`) are excluded — see [Determinism](#determinism)
for the full table.

The `#[derive(GenomeSafe)]` macro automatically adds `GenomeOrd` bounds for generic
parameters detected in `BTreeMap` key or `BTreeSet` element position.

The hash algorithm is xxHash64 over a canonical representation of the type's serde
structure. The 8-byte width pushes the birthday bound to ~4 billion types — practically
collision-free for accidental mismatches in multi-service deployments. Including the root
type name in the hash input distinguishes newtypes with different semantics (e.g.,
`Meters(f64)` vs `Seconds(f64)` produce different hashes despite identical inner layout).
This catches type confusion (Service A sends `Foo { x: u32, y: u64 }`,
Service B expects `Foo { x: u32, y: u32 }`) at deserialization time with
`DeError::SchemaMismatch`.

### Embedded Schema Source

The `GenomeSafe` derive macro generates `SCHEMA_SOURCE` — a cleaned Rust type definition
as plain UTF-8 text. This is embedded in genome file headers (see §File Header,
`schema_size` field) for human inspection.

**Purpose:** A developer can inspect a genome file and understand its structure without
access to the original Rust source. The schema hash remains the authoritative
compatibility check; the embedded source text is informational.

**What gets embedded:** The cleaned struct/enum definition — field names, types, variant
shapes, generic parameters. No imports, no `impl` blocks, no doc comments, no serde
attributes.

**Example:** For a type defined as:

```rust
#[derive(Serialize, Deserialize, GenomeSafe)]
struct Event<T> {
    event_id: u64,
    timestamp: i64,
    domain_id: DomainId,
    detached: bool,
    precursor: Index,
    domain_event: T,
}
```

`SCHEMA_SOURCE` produces:

```
struct Event<T> {
    event_id: u64,
    timestamp: i64,
    domain_id: DomainId,
    detached: bool,
    precursor: Index,
    domain_event: T,
}
```

**File format integration:** The `Writer` embeds `T::SCHEMA_SOURCE` in the schema block
between the file header and the first message. The `Reader` and `genome-dump` CLI expose
it. `schema_size = 0` is valid (no embedded source, backward compatible). Bare messages
do not carry embedded source — they are already small and self-contained via the hash.

**Not a schema registry.** The embedded source is local to the file. Cross-service schema
coordination uses the schema hash. A future schema registry (if needed) would index by
hash and store the source text as metadata.

### Exportable Schema Descriptor (Future)

A separate schema descriptor format for cross-language code generation may be defined
in a future version. The embedded `SCHEMA_SOURCE` provides the foundation — external
tools can parse the Rust type syntax or a future structured format derived from it.

### NATS/JetStream Integration

NATS and JetStream integration is provided by the **`pardosa-genome-nats`** companion crate,
not by `pardosa-genome` itself. The serialization crate is transport-agnostic — its job is
`bytes ↔ types`. The companion crate depends on `pardosa-genome` for encode/decode and owns
the NATS header protocol, metadata messages, stream lifecycle, JetStream rollup, and KV-based
stream discovery. See `pardosa-genome-nats` documentation for details.

### Hash Stability Contract

The schema hash is the primary compatibility mechanism for pardosa-genome. Every input
to the hash computation is **frozen** — changing any of the following invalidates all
existing schema hashes, making all existing genome files and bare messages unreadable.

| Input | Frozen value | Location |
|-------|-------------|----------|
| Hash algorithm | xxHash64 via `xxhash_rust::const_xxh64::xxh64` | `genome_safe.rs` |
| Seed value | `0` (all calls) | `schema_hash_bytes`, `schema_hash_combine` |
| Combine method | LE-concatenate two u64 → 16 bytes → xxh64(bytes, 0) | `schema_hash_combine` |
| Struct prefix | `"struct:Name"` | derive macro `build_hash_expr` |
| Enum prefix | `"enum:Name"` | derive macro `build_hash_expr` |
| Variant prefix | `"variant:Name"` | derive macro `build_hash_expr` |
| Field name hashing | Field name bytes hashed and combined per field | derive macro `build_field_hash_exprs` |
| Primitive names | `stringify!($ty)` (e.g., `"u32"`, `"bool"`) | `impl_genome_safe_primitive!` macro |
| Array length | `N as u64` included in hash | `impl GenomeSafe for [T; N]` |
| PhantomData | Always hashes as `"PhantomData"`, ignores `T` | blanket impl |

**Pinned hash tests:** The test suite includes pinned expected hash values for
regression detection. Any change to the above inputs will cause these tests to fail.

### String Type Identity

String-like types are hashed with strict Rust-type-identity, creating two
equivalence classes:

| Type | Hash input | Equivalence class |
|------|-----------|-------------------|
| `str` | `"str"` | Class A |
| `&str` | delegates to `str` | Class A |
| `Cow<'_, str>` | delegates to `str` | Class A |
| `Box<str>` | delegates to `str` (via Box transparency) | Class A |
| `String` | `"String"` | Class B |

**Schema-compatible field changes** (same equivalence class):
- `&str` → `Cow<'_, str>` ✅
- `Cow<'_, str>` → `Box<str>` ✅
- `&str` → `Box<str>` ✅

**Schema-breaking field changes** (different equivalence class):
- `String` → `&str` ❌
- `&str` → `String` ❌
- `String` → `Cow<'_, str>` ❌

**Rationale:** Serde serializes all string types identically, but the owned vs.
borrowed distinction affects zero-copy deserialization semantics. A message
serialized with `String` fields and deserialized with `&'de str` fields would
silently change memory management behavior. The schema hash detects this.

---

## Feature Flags

**`no_std` status: deferred.** The crate currently requires `std`. The design supports
a future `core` → `alloc` → `std` tiered model, but the implementation has not been
gated. `no_std` support will be added when an actual consumer needs it. Until then,
all APIs require `std`.

### Cargo.toml

```toml
[features]
default = ["std", "derive"]
std = []
derive = ["dep:pardosa-derive"]
zstd = ["std", "dep:zstd"]             # zstd requires std (C libzstd uses std::io)
```

### Feature Descriptions

| Feature | Default | Description |
|---------|:-------:|-------------|
| `std`   | ✓       | Required. All APIs depend on `std`. |
| `derive`| ✓       | Enables `#[derive(GenomeSafe)]` via the `pardosa-derive` proc-macro crate. |
| `zstd`  | ✗       | Enables zstd compression/decompression. Implies `std`. |

### Future: `no_std` Support

When a concrete `no_std` consumer exists, the following tiered model will be implemented:

- `core` only: `decode` for borrow-only types (`&str`, `&[u8]`, primitives)
- `alloc`: `decode` for owning types, `encode`, `Reader`/`Writer`
- `std`: File I/O, compression

This requires: `#![no_std]` attribute, conditional `std::error::Error` impls,
conditional `SerMessage` storage (`String` under `alloc`, `&'static str` under `core`),
and feature-gated blanket impls for `BTreeMap`, `BTreeSet`, `Arc`, `Cow`.

### Dependencies

```toml
[dependencies]
serde = { version = "1", default-features = false }
crc = { version = "3", default-features = false }       # REMOVED — xxHash64 used for all checksums (ADR-016)
xxhash-rust = { version = "0.8", default-features = false, features = ["const_xxh64"] }  # no_std-compatible schema hash
zstd = { version = "0.13", optional = true }             # C libzstd wrapper, requires std

[dev-dependencies]
serde = { version = "1", features = ["derive"] }
proptest = "1"
trybuild = "1"                                           # compile-fail tests
bolero = "0.11"                                          # unified fuzz + property testing (optional)
```

### Quality Tooling Dependencies

These are external tools, not Cargo dependencies. Install as needed:

```sh
# Code coverage
cargo install cargo-llvm-cov

# Bounded model checking (requires nightly)
cargo install --locked kani-verifier
cargo kani setup

# Deterministic benchmarks (optional, if criterion CI variance > 5%)
# cargo install iai-callgrind

# API stability (add at 0.9 or first external consumer)
# cargo install cargo-semver-checks
```

---

## Public API

### Options Structs

```rust
/// Options for encoding. All fields have sensible defaults.
#[derive(Clone, Debug)]
pub struct EncodeOptions {
    /// Compression algorithm. Default: None.
    pub compression: Compression,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self { compression: Compression::None }
    }
}

/// Options for decoding. See [Deserialization Limits](#deserialization-limits)
/// for full field documentation, page classes, and defaults.
#[derive(Clone, Debug)]
pub struct DecodeOptions { /* see detailed definition above */ }

pub enum Compression {
    None,
    #[cfg(feature = "zstd")]
    Zstd { level: i32 },      // 1–22, default 3
}
```

### Single Message (bare format)

```rust
/// Serialize with default options (requires `alloc`).
pub fn encode<T: Serialize + GenomeSafe>(value: &T) -> Result<Vec<u8>, SerError>;

/// Serialize with options (requires `alloc`; compression requires `std` + `zstd`).
pub fn encode_with_options<T: Serialize + GenomeSafe>(
    value: &T,
    options: &EncodeOptions,
) -> Result<Vec<u8>, SerError>;

/// Deserialize with default options (zero-copy, always-verify, works in core-only for borrow types).
pub fn decode<'de, T: Deserialize<'de> + GenomeSafe>(buf: &'de [u8]) -> Result<T, DeError>;

/// Deserialize with custom options.
pub fn decode_with_options<'de, T: Deserialize<'de> + GenomeSafe>(
    buf: &'de [u8],
    options: &DecodeOptions,
) -> Result<T, DeError>;
```

When `EncodeOptions::compression` is set, the output is a compressed bare message
with the 1-byte algorithm identifier in the header (see [Compression](#compression)).
When decoding a compressed message, the algorithm is read from the header byte —
the caller does not need to specify it.

**Note on compressed decode**: `decode` auto-detects compression from the `algo` byte at
offset 10 in bare messages. For uncompressed messages (`algo = 0x00`), `T` can borrow from
the input buffer (zero-copy). For compressed messages (`algo ≥ 0x01`), `decode` decompresses
into an internal buffer; types with `&'de str` or `&'de [u8]` fields will fail with a
lifetime error. Use owned types (`String`, `Vec<u8>`) when compression is expected.

### Multi-Message File (requires `alloc`)

`Reader` and `Writer` require `alloc` because index parsing allocates a `Vec` of message
offsets, and `Writer` accumulates serialized message bytes in a `Vec<u8>`.

```rust
pub struct Writer<T: GenomeSafe> { /* ... */ }

impl<T: Serialize + GenomeSafe> Writer<T> {
    pub fn new() -> Self;
    pub fn with_compression(self, compression: Compression) -> Self;
    pub fn push(&mut self, value: &T) -> Result<(), SerError>;
    pub fn finish(self) -> Result<Vec<u8>, SerError>;
}

pub struct Reader<'de> { /* ... */ }

impl<'de> Reader<'de> {
    pub fn new(buf: &'de [u8]) -> Result<Self, FileError>;
    pub fn message_count(&self) -> u64;
    pub fn compression(&self) -> Compression;
    pub fn read_message<T: Deserialize<'de> + GenomeSafe>(&self, index: u64) -> Result<T, DeError>;
    pub fn messages<T: Deserialize<'de> + GenomeSafe>(&self) -> impl Iterator<Item = Result<T, DeError>>;
}
```

`Reader` auto-detects compression from the file header flags and decompresses transparently.
When compression is enabled, `read_message` returns owned values (decompression allocates).
Per-message xxHash64 in the index is verified before decompression — a checksum mismatch
short-circuits without spending CPU on decompression of corrupted data.

### Roundtrip Verification Helper

```rust
/// Serialize, deserialize, and compare. For CI use — catches `#[serde(untagged)]`
/// and other silent-corruption scenarios. Requires T: PartialEq + Debug.
#[cfg(feature = "alloc")]
pub fn verify_roundtrip<T>(value: &T) -> Result<(), RoundtripError>
where
    T: Serialize + for<'de> Deserialize<'de> + GenomeSafe + PartialEq + core::fmt::Debug;
```

---

## Crate Structure

```
crates/pardosa-genome/
├── Cargo.toml
├── genome.md             This document
└── src/
    ├── lib.rs            Public API: encode, decode, encode_with_options, decode_with_options,
    │                     verify_roundtrip, re-exports, GenomeSafe/GenomeOrd derive re-export
    ├── genome_safe.rs    GenomeSafe + GenomeOrd traits, blanket impls, schema hash helpers
    ├── format.rs         Magic bytes, version, header layout (incl. schema_size), constants
    ├── config.rs         EncodeOptions, DecodeOptions, Compression, PageClass
    ├── error.rs          SerError, DeError, FileError
    ├── sizing_ser.rs     SizingSerializer — first pass: computes total buffer size (Phase 2)
    ├── writing_ser.rs    WritingSerializer — second pass: writes into pre-allocated buffer (Phase 2)
    ├── de.rs             serde::Deserializer<'de> (zero-copy reads, always-verify) (Phase 2)
    ├── compress.rs       Compression dispatch + compressed message framing (Phase 3)
    ├── compress/
    │   └── zstd.rs       Zstd compress/decompress (feature-gated) (Phase 3)
    ├── reader.rs         Multi-message file reader (incl. schema block parsing) (Phase 3)
    ├── writer.rs         Multi-message file writer (incl. schema block embedding) (Phase 3)
    └── bin/
        └── genome-dump.rs  CLI: annotated hex dump, embedded schema display (Phase 3)

crates/pardosa-derive/
├── Cargo.toml
└── src/
    └── lib.rs            #[derive(GenomeSafe)] proc-macro: schema hash computation,
                          schema source generation, serde attribute rejection
```

### `genome-dump` CLI

A built-in diagnostic binary that reads genome files or bare messages and prints
annotated hex dumps with structural annotations: header fields, message boundaries,
inline/heap regions, offset resolution chains, checksum verification results.

Always in sync with the implementation — no external format description to maintain.
Available when the `std` feature is enabled.

```sh
# Dump file structure
cargo run --bin genome-dump -- file events.pgno

# Dump bare message from stdin
cat message.bin | cargo run --bin genome-dump -- bare --type MyStruct
```

Output format: byte offset, hex bytes, field name, decoded value. Example:

```
FILE HEADER (32 bytes)
  00..04  50 47 4E 4F           magic           "PGNO"
  04..06  01 00                 format_version  1
  06..08  01 00                 flags           compression_algo=zstd
  08..10  A3 7B 2E 91 04 F1 C8 schema_hash     0xC8F10491_2E7BA3.. (type-dependent)
  10..14  00 00 00 00           dict_id         0 (reserved v1)
  14      02                    page_class      PAGE2 (65536 elements)
  15..20  00 00 00 00 00 00 ... reserved        (all zeros ✓)
```

---

## Error Catalog

### `SerError` (serialization errors)

| Variant | Description |
|---------|-------------|
| `MessageTooLarge` | Serialized message exceeds u32::MAX bytes |
| `UnsupportedAttribute(attr)` | `#[serde(flatten)]`, `#[serde(tag)]`, `#[serde(tag+content)]` detected |
| `InternalSizingMismatch { expected, actual }` | `WritingSerializer` produced a different byte count than `SizingSerializer` predicted |
| `CompressionFailed` | Zstd compression returned an error |
| `Custom(msg)` | serde's `ser::Error::custom()` |

### `DeError` (deserialization errors)

| Variant | Description |
|---------|-------------|
| `BufferTooSmall` | Buffer shorter than expected for the type |
| `OffsetOutOfBounds(offset, buf_len)` | Offset points past buffer end |
| `OffsetOverflow` | `offset + len` overflows u32/usize |
| `InvalidUtf8` | String data is not valid UTF-8 |
| `InvalidChar(u32)` | Char value is not a valid Unicode scalar |
| `InvalidBool(u8)` | Bool value is not 0x00 or 0x01 |
| `InvalidDiscriminant(u32)` | Enum discriminant exceeds variant count |
| `AllocRequired` | Owning type requested in core-only mode |
| `DepthLimitExceeded` | Nesting depth exceeds `DecodeOptions::max_depth` |
| `ElementLimitExceeded` | Total elements exceed page class limit (`DecodeOptions::max_total_elements`) |
| `TrailingBytes { expected, actual }` | Buffer has trailing bytes after message (when `reject_trailing_bytes = true`) |
| `VersionMismatch { expected, actual }` | `format_version` is not supported |
| `SchemaMismatch { expected, actual }` | Schema hash in message header does not match expected type |
| `NonZeroPadding { offset }` | Padding byte is not 0x00 (hard error) |
| `BackwardOffset { offset }` | Offset points into inline region |
| `ChecksumMismatch` | Per-message xxHash64 mismatch (file format index verification) |
| `DecompressionFailed` | Zstd decompression returned an error (includes window size rejection) |
| `UncompressedSizeTooLarge(u32)` | `msg_data_size` or zstd `Frame_Content_Size` exceeds `DecodeOptions::max_uncompressed_size` |
| `MessageTooLarge(u32)` | `msg_data_size` in bare message exceeds `DecodeOptions::max_message_size` |
| `PostDecompressionTrailingBytes` | Decompressed buffer has trailing bytes beyond message boundary |
| `Custom(msg)` | serde's `de::Error::custom()` |

### `FileError` (file-level errors)

| Variant | Description |
|---------|-------------|
| `InvalidMagic` | Header or footer magic is not "PGNO" |
| `UnsupportedVersion(u16)` | Format version not supported |
| `UnsupportedCompression(u8)` | Unknown compression algorithm in header flags |
| `InvalidChecksum` | Footer xxHash64 mismatch |
| `InvalidIndex` | Index offset or entry is inconsistent |
| `CompressionNotAvailable` | File uses zstd compression but the `zstd` feature is not enabled |
| `MessageError(index, DeError)` | Error in specific message |

### `RoundtripError` (roundtrip verification)

| Variant | Description |
|---------|-------------|
| `SerializationFailed(SerError)` | Serialization step failed |
| `DeserializationFailed(DeError)` | Deserialization step failed |
| `ValueMismatch(String)` | Deserialized value ≠ original (debug-formatted diff) |

---

## Implementation Steps

**Status:** Phase 1 implemented. Crate scaffold, `GenomeSafe` trait with `SCHEMA_HASH` and
`SCHEMA_SOURCE`, derive macro, format constants (including embedded schema block in file
header), config types, and error catalog are complete. Both crates (`pardosa-genome`,
`pardosa-derive`) are workspace members with 24 passing tests.

All phases are strictly sequential. Continuous testing runs alongside every phase.

```
Phase 1 ──→ Phase 2 ──→ Phase 3 ──→ Phase 4
                │                      │
                └── Continuous Testing ─┘
```

### Phase 1 — Scaffold, Constants, GenomeSafe

**Goal:** Crate exists, compiles, all format constants and error types defined, `GenomeSafe`
compile-time gate operational. No serialization yet.

#### 1A — Scaffold

| # | Task | Output |
|---|------|--------|
| 1A.1 | Create `crates/pardosa-genome/Cargo.toml` — features: `default=["std","derive"]`, `std`, `alloc`, `derive`, `zstd` | Cargo manifest |
| 1A.2 | Create `crates/pardosa-derive/Cargo.toml` — proc-macro crate for `#[derive(GenomeSafe)]` | Cargo manifest |
| 1A.3 | Add both crates to workspace `members` in root `Cargo.toml` | Workspace registration |
| 1A.4 | Stub `src/lib.rs` for both crates | `cargo check` passes |
| 1A.5 | Verify CI (`cargo build --workspace`, `cargo test --workspace`) | Green pipeline |

#### 1B — Constants, Config, Errors

| # | Task | File | Notes |
|---|------|------|-------|
| 1B.1 | Magic bytes (`PGNO`), format version (`1`), header/footer/index struct sizes | `format.rs` | 32B header, 32B footer, 24B index entry (mandatory xxHash64) |
| 1B.2 | Flag bit layout: compression_algo (0-2), reserved (3-15) | `format.rs` | No encryption flags in v1. Compression state derived from `compression_algo ≠ 000` |
| 1B.3 | `EncodeOptions` with compression field | `config.rs` | Default: no compression |
| 1B.4 | `DecodeOptions` with all fields | `config.rs` | `max_depth=128`, `max_total_elements=PageClass::Page0` (256), `max_uncompressed_size=256MiB`, `max_message_size=256MiB`, `max_zstd_window_log=22`, `reject_trailing_bytes=true` |
| 1B.5 | `PageClass` enum (Page0=256, Page1=4096, Page2=65536, Page3=1048576) | `config.rs` | `page_class` byte in file header (offset 20) |
| 1B.6 | `Compression` enum (`None`, `Zstd{level}`) | `config.rs` | Feature-gated `Zstd` variant |
| 1B.7 | `SerError`, `DeError`, `FileError`, `RoundtripError` | `error.rs` | `DeError` includes `SchemaMismatch`, `NonZeroPadding`, `BackwardOffset`, `TrailingBytes`, `PostDecompressionTrailingBytes`, `VersionMismatch`, `MessageTooLarge`. `#[cfg]`-gated: `String` with alloc, `&'static str` core-only |
| 1B.8 | Unit tests for config defaults, error display, page class boundaries | `tests/` | |

#### 1C — GenomeSafe

| # | Task | File | Notes |
|---|------|------|-------|
| 1C.1 | Define `pub trait GenomeSafe { const SCHEMA_HASH: u64; const SCHEMA_SOURCE: &'static str; }` | `genome_safe.rs` | Marker trait with compile-time schema fingerprint and human-readable source text |
| 1C.2 | Blanket impls: primitives, `String`, `&str`, `&[u8]`, `Vec<T>`, `[T;N]`, tuples (1-16), `Option<T>`, `Box<T>`, `Arc<T>`, `Cow<'a,T>`, `BTreeMap<K,V>`, `BTreeSet<T>`, `PhantomData<T>`, `()` | `genome_safe.rs` | Recursive bounds: `T: GenomeSafe` |
| 1C.3 | `#[derive(GenomeSafe)]` proc-macro: syntactic rejection of `HashMap`, `HashSet`, `#[serde(untagged)]`, `#[serde(tag)]`, `#[serde(tag, content)]`, `#[serde(flatten)]`, `#[serde(skip_serializing_if)]`. Generates `SCHEMA_HASH` via xxHash64 and `SCHEMA_SOURCE` from cleaned type definition | `pardosa-derive/` | Trait-bound enforcement catches aliases/generics |
| 1C.4 | Compile-fail tests via `trybuild` | `tests/compile_fail/` | HashMap, HashSet, untagged, tag, tag+content, flatten, skip_serializing_if rejection |

**Phase 1 exit criteria:** All types compile under `--no-default-features`, `--features alloc`,
`--features std`. `HashMap<String,u32>` field in `#[derive(GenomeSafe)]` struct fails to compile.

### Phase 2 — Core Serialization + Schema

**Goal:** `encode` / `decode` / `decode_with_options` work for the full serde data model.
Two-pass direct serialization (no intermediate Value tree). Schema hash in message headers.
Every deserialization verifies structural integrity inline.

#### 2A — Serialize Path (Two-Pass)

| # | Task | File | Notes |
|---|------|------|-------|
| 2A.1 | `SizingSerializer` — first pass computing total buffer size, no allocation | `sizing_ser.rs` | `#[cfg(feature = "alloc")]`. Walks value computing inline + heap sizes. Tracks heap offset per nesting level |
| 2A.2 | `WritingSerializer` — second pass writing into pre-allocated buffer | `writing_ser.rs` | `#[cfg(feature = "alloc")]`. Writes inline data + 4-byte heap stubs. Writes heap items with `[len:u32]`/`[count:u32]` prefix. Hard runtime check: `if actual_size != expected_size { return Err(SerError::InternalSizingMismatch) }` |
| 2A.3 | Implement `SerializeStruct`, `SerializeSeq`, `SerializeTuple`, `SerializeMap`, `SerializeTupleStruct`, `SerializeTupleVariant`, `SerializeStructVariant` | `writing_ser.rs` | Runtime detection of `#[serde(flatten)]`, `#[serde(tag)]`, `#[serde(tag+content)]` as defense-in-depth → `SerError::UnsupportedAttribute` |
| 2A.4 | `encode<T: Serialize + GenomeSafe>()` public API | `lib.rs` | Bare header: `[format_version:u16][schema_hash:u64][algo:u8][msg_data_size:u32][data]` (inline data starts at byte 15) |

#### 2B — Schema Hash

| # | Task | File | Notes |
|---|------|------|-------|
| 2B.1 | Schema hash computation from serde type structure | `schema.rs` | 8-byte xxHash64 of canonical type representation (includes root type name). Deterministic across compilations. Trait-based composition: each type's `SCHEMA_HASH` calls inner types' hashes via trait bounds |
| 2B.2 | Schema hash embedded in bare message header (bytes 2-9) and file header (bytes 8-15) | `format.rs` | Verified on decode (bare) and file open (file format) |
| 2B.3 | Research exportable schema definition format (RON, Smithy) | `schema.rs` | Enable code generation from known schema. Prototype in this phase, finalize later |

#### 2C — Deserialize Path (with Integrated Verification)

| # | Task | File | Notes |
|---|------|------|-------|
| 2C.1 | `MessageDeserializer<'de>` implementing `serde::Deserializer<'de>` with always-verify | `de.rs` | Zero-copy `&'de str` / `&'de [u8]`. Never re-slice `buf`. `from_le_bytes` only |
| 2C.2 | Verification inline: padding zeros (hard error), bool `0x00`/`0x01`, backward offsets (hygiene), schema hash, char validation | `de.rs` | `NonZeroPadding` → `DeError`, not warning |
| 2C.3 | Heap reads: `[len:u32][data]` for String/bytes, `[count:u32][elements]` for Vec/Map | `de.rs` | 4-byte inline stub (offset only). Empty containers always allocate `[len:u32 = 0]` heap entry |
| 2C.4 | `SeqAccess`, `MapAccess`, `EnumAccess`, `VariantAccess` | `de.rs` | Sub-deserializers share same `buf` with different cursor |
| 2C.5 | `DecodeOptions` limit enforcement: depth (including newtypes), element count (page class) | `de.rs` | Pre-allocation check before `Vec::with_capacity`. All offset arithmetic widened to u64 |
| 2C.6 | Trailing bytes rejection (default on) | `de.rs` | |
| 2C.7 | `decode` + `decode_with_options` public API | `lib.rs` | Always-verify, no separate verification pass |
| 2C.8 | `cargo-fuzz` target for `decode` on arbitrary `&[u8]` | `fuzz/` | Invariant: no panic on any input |

#### 2D — Roundtrip + Options API

| # | Task | File | Notes |
|---|------|------|-------|
| 2D.1 | `verify_roundtrip<T>()` — serialize/deserialize/compare for CI | `lib.rs` | Catches `#[serde(untagged)]` misuse |
| 2D.2 | `encode_with_options` public API | `lib.rs` | `EncodeOptions` struct (compression field used in Phase 3) |

#### 2E — Quality Tooling

| # | Task | File | Notes |
|---|------|------|-------|
| 2E.1 | Golden test vectors — 16 vectors in `tests/golden/`, one per serde data model type + wire formats | `tests/golden/` | Byte-level format conformance. See §Test Plan: Golden Test Vectors |
| 2E.2 | 32-bit CI cross-compilation — `cargo test --target i686-unknown-linux-gnu` | CI | Validates u64 widening for offset arithmetic (genome-sec.md finding #18) |
| 2E.3 | `cargo-llvm-cov` coverage measurement — 90% line coverage floor | CI | Identifies dead error paths in deserializer |
| 2E.4 | Kani proof harnesses — offset arithmetic overflow proofs | `src/kani_proofs.rs` | 3–5 harnesses behind `#[cfg(kani)]`. See §Quality Tooling: Kani |

#### Phase 2 Tests

| Category | Coverage |
|----------|----------|
| Round-trip | Every serde data model type (bool, ints, floats, char, string, bytes, option, unit, newtype, tuple, seq, map, struct, enum, recursive) |
| Zero-copy | Pointer-range proof for `&str` and `&[u8]` |
| Alignment | Mixed-alignment struct field offset verification |
| Adversarial | Truncated buffers, all-zeros, all-0xFF, backward offsets, cyclic offsets |
| Limits | Depth (including newtype chains), element count (page classes), trailing bytes, `max_message_size` |
| Verification | Padding zeros (hard error), bool `0x00`/`0x01`, backward offsets, schema hash, char validation — all inline |
| Serde attrs | `rename` ✓, `skip` ✓, `default` (silent no-op, documented), `with` ✓ (breaking change documented), `flatten` ✗ (compile-time), `tag` ✗ (compile-time), `untagged` ✗ (compile-time), `skip_serializing_if` ✗ (compile-time) |
| Zero-size | `Option<()>`, `Vec<()>`, empty struct |
| Deeply nested | `Vec<Vec<String>>`, `Option<Vec<Option<String>>>` |
| None sentinel | `0xFFFFFFFF` for None, valid offset for Some |
| Schema hash | Deterministic computation, mismatch detection, field change detection |
| HashMap | Compile-fail test via `trybuild` (not `GenomeSafe`) |
| Property-based | `proptest` round-trip on arbitrary structs |
| Feature gates | `core`-only, `alloc`, `std` compilation matrix |
| Sizing consistency | `InternalSizingMismatch` via mock side-effectful `Serialize` impl |
| Golden vectors | 16 byte-level conformance vectors (§Test Plan: Golden Test Vectors) |
| Kani proofs | Offset arithmetic overflow absence (3–5 harnesses) |
| Coverage | ≥90% line coverage via `cargo-llvm-cov` |

**Phase 2 exit criteria:** `encode` → `decode` round-trips all types. ≈1× memory overhead
(no Value tree). Schema hash catches type mismatches. Structural verification catches all
padding/bool/offset violations inline. `decode` never panics on arbitrary input. Zero `unsafe`.
Golden test vectors confirm wire format matches spec byte-for-byte. Kani proves offset
arithmetic cannot overflow on any input. Line coverage ≥ 90%. CI passes on `i686` (32-bit).

### Phase 3 — Zstd Compression + File Container

**Goal:** Zstd compression for bare messages. Multi-message file format with mandatory
per-message xxHash64 in 24-byte index entries. Page classes via file header byte.

#### 3A — Zstd Compression

| # | Task | File | Notes |
|---|------|------|-------|
| 3A.1 | Compression dispatch | `compress.rs` | `Compression` enum → zstd module |
| 3A.2 | Zstd compress/decompress | `compress/zstd.rs` | Level 1-22, content size flag always set (`ZSTD_c_contentSizeFlag = 1`), window log cap, `#[cfg(feature = "zstd")]` |
| 3A.3 | Compressed bare header: `[version:u16][schema_hash:u64][algo:u8=0x01][compressed_size:u32][msg_data_size:u32][data]` | `compress.rs` | 19-byte header. Pre-allocation: `msg_data_size` vs `max_uncompressed_size` |
| 3A.4 | Post-decompression verification: trailing bytes check, decompressed size matches `msg_data_size` | `compress.rs` | `PostDecompressionTrailingBytes` error |
| 3A.5 | `encode_with_options` + `decode` handle compression transparently | `lib.rs` | `decode` reads algo byte from header — caller doesn't specify algorithm |
| 3A.6 | Zstd `Frame_Content_Size` + `max_zstd_window_log` validation before allocation | `compress/zstd.rs` | Set `ZSTD_d_maxWindowLog` to cap decompressor memory |

#### 3B — File Container

| # | Task | File | Notes |
|---|------|------|-------|
| 3B.1 | `Writer<T: GenomeSafe>` — accumulate messages, 24-byte index entries (mandatory xxHash64) | `writer.rs` | `#[cfg(feature = "alloc")]`. Generic `T` enforces single-schema-per-file at compile time |
| 3B.2 | File header (32B) with `schema_hash` from `T::SCHEMA_HASH`, `page_class` byte at offset 20 | `writer.rs` | format_version, flags, `dict_id = 0` in v1 |
| 3B.3 | File footer (32B) with xxHash64 | `writer.rs` | Index offset, message count, footer magic |
| 3B.4 | `Writer::with_compression(Zstd { level })` | `writer.rs` | Sets header flags `compression_algo` bits |
| 3B.5 | `Reader` — parse header, footer, index. Strict validation | `reader.rs` | Reject unknown flags, non-zero reserved bytes (header 25-31, footer 16-19). Validate: offset ≥ 32, offset + size ≤ index_offset, monotonically non-decreasing, non-overlapping. `message_count × 24` overflow check |
| 3B.6 | `Reader` — per-message xxHash64 verification before decompression | `reader.rs` | Checksum mismatch short-circuits decompression |
| 3B.7 | `Reader` — transparent zstd decompression from header flags | `reader.rs` | Auto-detect, no caller specification. `compressed_size` bounds-checked |
| 3B.8 | `Reader` — `read_message`, `messages` iterator, `message_count` | `reader.rs` | |
| 3B.9 | Page class: `page_class` byte in file header (offset 20) + file extension convention | `reader.rs` | Header byte is optimization hint, not security boundary; reader enforces limits. `dict_id == 0` enforced in v1 |
| 3B.10 | Edge case: 0-message file (64 bytes total) | `reader.rs` / `writer.rs` | |
| 3B.11 | `genome-dump` CLI binary — annotated hex dump of files and bare messages | `bin/genome-dump.rs` | `#[cfg(feature = "std")]`. See §Crate Structure: genome-dump CLI |

#### Phase 3 Tests

| Category | Coverage |
|----------|----------|
| Zstd round-trip | All data model types, compression ratio verification |
| Zstd security | Window size rejection, `Frame_Content_Size` validation, decompression bomb (`msg_data_size = u32::MAX`) |
| Post-decompression | Trailing bytes check after decompression, decompressed size vs `msg_data_size` |
| Compressed bare | Algorithm byte in header, transparent decode, `compressed_size` bounds check |
| File round-trip | 0, 1, 1000 messages, random access, iteration |
| File integrity | xxHash64 per-message, footer xxHash64, corrupted data detection, `message_count × 24` overflow |
| File edges | Invalid magic, truncated file, corrupted index, missing footer, non-zero reserved bytes |
| Page classes | Element limits per class, boundary tests |
| Feature gates | `zstd` feature on/off, `CompressionNotAvailable` error |
| Fuzz targets | `decode` on arbitrary bytes, `Reader::new` on arbitrary bytes |

**Phase 3 exit criteria:** Zstd round-trips all types. Crafted decompression bombs rejected
before allocation. File container read/write works with 0–1000 messages. Per-message xxHash64
catches corruption. Page classes enforce element limits.

### Phase 4 — Pardosa Integration

**Goal:** `pardosa` crate uses `pardosa-genome` as primary serialization. Tracked in
`pardosa-next.md`.

| # | Task | Notes |
|---|------|-------|
| 4.1 | `pardosa` adds `genome` feature flag → `dep:pardosa-genome` | Passthrough `zstd` feature |
| 4.2 | `Event<T>` — private fields, `event_id: u64`, `precursor: Index` (not `Option`), serde derives, `#[non_exhaustive]` | Per pardosa-next.md §P1 |
| 4.3 | `Index::NONE` sentinel (`u64::MAX`), `checked_next()` | Per pardosa-next.md §P2 |
| 4.4 | `Fiber` — serde derives, fallible constructor, bounds-checked `advance` | Per pardosa-next.md §P3 |
| 4.5 | `LockedRescuePolicy` enum replacing `bool` | Per pardosa-next.md §P4 |
| 4.6 | `PersistenceAdapter<T>` trait | `persistence.rs` |
| 4.7 | `GenomePersistence<T>` — genome file persistence | `persistence/genome.rs` |
| 4.8 | NATS integration via separate `pardosa-genome-nats` crate | See [Schema Export](#schema-export) |
| 4.9 | Migration lifecycle: new-file model, index remap, `event_id` preservation | `migration.rs` |

**Phase 4 exit criteria:** `pardosa` reads/writes events via genome files. Migration creates
new file. NATS transport available via separate crate.

### Continuous Testing

Tests run alongside each phase, not deferred to a separate phase.

| Activity | When | Tool |
|----------|------|------|
| Compile-fail tests | Phase 1C onward | `trybuild` |
| Unit tests | Every task | `cargo test` |
| Property-based tests | Phase 2 onward | `proptest` |
| Golden test vectors | Phase 2D onward | `tests/golden/` (format conformance) |
| Fuzz targets | Phase 2C.8 onward (bare), Phase 3B onward (file) | `cargo-fuzz` |
| Code coverage | Phase 2A onward | `cargo-llvm-cov` (floor: 90% line coverage) |
| 32-bit CI target | Phase 2C onward | `cargo test --target i686-unknown-linux-gnu` |
| Kani proof harnesses | Phase 2C onward | `cargo kani` (offset arithmetic, 3–5 harnesses) |
| CI matrix | All phases | `--no-default-features`, `--features alloc`, `--features std`, `--features zstd` |
| `verify_roundtrip` in CI | Phase 2D onward | Part of test suite |

### Risk Register

| Risk | Impact | Mitigation |
|------|--------|------------|
| Two-pass serialization correctness | Data corruption | Extensive round-trip tests, proptest, fuzzing, `InternalSizingMismatch` hard check |
| `#[serde(untagged)]` silent corruption | Data loss | `GenomeSafe` compile-time gate + `verify_roundtrip` CI |
| Decompression bombs | DoS | Pre-allocation size check + `Frame_Content_Size` validation + window log cap |
| Schema hash collisions | Silent type confusion | 8-byte xxHash64 — birthday bound ~4 billion types. Root type name in hash input. One-schema-per-file via pardosa model |
| No schema evolution | Breaking change on field change | `verify_roundtrip` CI detection. New-file migration model |
| Side-effectful `Serialize` impls | Sizing/writing divergence | `InternalSizingMismatch` hard check in all builds. `GenomeSafe` restricted type subset |

### Estimated Effort

| Phase | Complexity | Rough LOC |
|-------|-----------|-----------|
| 1 — Scaffold + Constants + GenomeSafe | Medium | ~1,200 |
| 2 — Core Serialization + Schema | **High** | ~1,800 |
| 3 — Zstd + File Container | Medium | ~1,200 |
| 4 — Pardosa Integration | Medium | ~1,000 |
| Tests (continuous) | High | ~2,000 |
| **Total** | | **~7,200** |

Phase 2 is the critical path. All subsequent phases layer on top of it.

---

## Test Plan

### Round-Trip Tests

Serialize → deserialize → assert equality for every serde data model type:

- `bool`: true, false
- Integers: `i8`, `i16`, `i32`, `i64`, `i128`, `u8`, `u16`, `u32`, `u64`, `u128`
  (boundary values: 0, 1, -1, MIN, MAX)
- Floats: `f32`, `f64` (0.0, -0.0, 1.0, MIN, MAX, INFINITY, NEG_INFINITY, NAN)
- `char`: 'a', '\0', '🦀', '\u{10FFFF}'
- `String` / `&str`: "", "hello", "🕷️ pardosa", long string (>4096 bytes)
- Bytes: empty, small, large
- `Option<T>`: None, Some(scalar), Some(String), Some(None), Some(Some(42))
- `()`, unit struct
- Newtype struct: `Meters(42.0)`
- Tuple: `(u32, String, bool)`, empty tuple `()`
- Tuple struct: `Point(f64, f64)`
- Seq / `Vec<T>`: empty, one element, many elements, `Vec<Vec<String>>`
- Map: empty, `BTreeMap<String, u32>`, `BTreeMap<u32, String>` (non-string keys),
  `BTreeMap<(u32, u32), String>` (compound keys)
- Struct: simple, nested, with all field types
- Enum: unit variant, newtype variant, tuple variant, struct variant
- Recursive: `struct Node { value: u32, children: Vec<Node> }`

### Zero-Copy Proof Tests

```rust
let buf = pardosa_genome::encode(&entry)?;
let entry: Entry<'_> = pardosa_genome::decode(&buf)?;
let buf_range = buf.as_ptr_range();
assert!(buf_range.contains(&(entry.name.as_ptr())));
assert!(buf_range.contains(&(entry.data.as_ptr())));
```

### Verification Tests

- Valid buffers pass `decode`.
- Truncated buffer → `BufferTooSmall`.
- Offset past end → `OffsetOutOfBounds`.
- Offset + len overflow → `OffsetOverflow`.
- Invalid UTF-8 in string → `InvalidUtf8`.
- Invalid char value (0xD800, 0x110000) → `InvalidChar`.
- Invalid bool (0x02) → `InvalidBool`.
- Non-zero padding → `NonZeroPadding` (hard error).
- Backward offset → `BackwardOffset`.
- Trailing bytes → `TrailingBytes` (default config).
- Trailing bytes accepted when `reject_trailing_bytes = false`.
- Schema hash mismatch → `SchemaMismatch`.

### Adversarial Input Tests

- Cyclic offsets (A → B → A): no infinite loop, error returned.
- Offset pointing into header/msg_data_size field.
- Overlapping heap regions.
- Maximum-size message (offset near u32::MAX).
- All-zeros buffer.
- All-0xFF buffer.

### Multi-Message File Tests

- Write 0 messages, read back.
- Write 1 message, read back.
- Write 1000 messages, random-access read, iterate.
- Invalid header magic → `FileError::InvalidMagic`.
- Corrupted footer checksum → `FileError::InvalidChecksum`.
- Truncated file (missing footer, partial index).
- `read_message` with out-of-bounds index → error (not panic).
- Page class from file extension: `.PAGE0` → 256, `.PAGE1` → 4096, `.PAGE2` → 65536,
  `.PAGE3` → 1048576 element limits. Unknown extension → default page class.
- Non-zero reserved header flags → `FileError::UnsupportedVersion` or rejection.

### Feature-Gated Tests

- `--no-default-features`: `decode` compiles and passes with borrow-only types.
- `--no-default-features`: `encode` does NOT compile.
- `--features alloc`: `encode` + `decode` all work.
- `--features std`: full API including file I/O.
- `--features zstd`: zstd compression/decompression APIs available.
- `--features zstd`: `Compression` enum has `Zstd` variant.

### Property-Based Tests (proptest)

```rust
#[derive(Debug, Serialize, Deserialize, Arbitrary)]
struct Fuzzable {
    a: u32,
    b: String,
    c: Option<i64>,
    d: Vec<u8>,
    e: bool,
}

proptest! {
    fn round_trip(v: Fuzzable) {
        let bytes = pardosa_genome::encode(&v).unwrap();
        let decoded: Fuzzable = pardosa_genome::decode(&bytes).unwrap();
        assert_eq!(v, decoded);
    }
}
}
```

### Char Boundary Tests

| Input u32 | Valid? | Reason |
|-----------|--------|--------|
| `0x0000` | ✓ | NUL character |
| `0x0041` | ✓ | 'A' |
| `0xD7FF` | ✓ | Last before surrogates |
| `0xD800` | ✗ | Surrogate start |
| `0xDBFF` | ✗ | High surrogate end |
| `0xDC00` | ✗ | Low surrogate start |
| `0xDFFF` | ✗ | Surrogate end |
| `0xE000` | ✓ | First after surrogates |
| `0x10FFFF` | ✓ | Maximum Unicode |
| `0x110000` | ✗ | Beyond Unicode |
| `0xFFFFFFFF` | ✗ | Maximum u32 |

### Serde Attribute Tests

- `#[serde(rename = "...")]`: works (field name is not stored in binary).
- `#[serde(skip)]`: works (field excluded from layout).
- `#[serde(default)]`: **silent no-op** — fields are always present at their fixed inline
  positions and are never skipped during serialization. Unlike self-describing formats,
  missing fields cannot be defaulted because the format has no concept of field presence.
  This attribute is accepted but functionally inert. Consider a `GenomeSafe` compile-time
  warning when present.
- `#[serde(flatten)]`: **compile-fail test** — `GenomeSafe` derive rejects at compile time.
  Runtime detection in serializer produces `SerError::UnsupportedAttribute` as defense-in-depth.
- `#[serde(tag = "type")]`: **compile-fail test** — `GenomeSafe` derive rejects at compile time.
- `#[serde(tag = "t", content = "c")]`: **compile-fail test** — `GenomeSafe` derive rejects at compile time.
- `#[serde(untagged)]`: **compile-fail test** — `GenomeSafe` derive rejects at compile time.
- `#[serde(skip_serializing_if)]`: **compile-fail test** — `GenomeSafe` derive rejects at compile time.
- `#[serde(with = "module")]`: works — the custom serialization module controls the serde
  data model type emitted. Changing the module is a **breaking change** to the binary layout.
  Test: round-trip a struct using `#[serde(with)]` for a field, verify stability.

### Alignment Tests

For structs with mixed-alignment fields, verify that each field's offset in the serialized
buffer is a multiple of its alignment:

```rust
struct Mixed { a: u8, b: u64, c: u16, d: u32 }
// Expected layout: a@0(1B), pad(7B), b@8(8B), c@16(2B), pad(2B), d@20(4B)
// Total inline: 24 bytes
```

### HashMap Compile-Fail Test

`HashMap` does not implement `GenomeSafe` because its non-deterministic iteration order
makes serialization output non-reproducible. This is enforced at compile time:

```rust
// tests/compile_fail/hashmap_not_genome_safe.rs (trybuild)
fn main() {
    let map: std::collections::HashMap<String, u32> = Default::default();
    pardosa_genome::encode(&map).unwrap(); // should not compile: HashMap !GenomeSafe
}
```

The rationale for rejecting `HashMap` (rather than silently producing non-deterministic
output) is documented in [Determinism](#determinism).

### Zero-Size Element Tests

- `Option<()>`: `Some(())` — offset points to valid position, 0 bytes read. Round-trips.
- `Option<()>`: `None` — offset = `0xFFFFFFFF`. Round-trips.
- `Vec<()>`: `vec![(), (), ()]` — count = 3, offset points to valid position, 0 bytes per
  element. Round-trips with correct length.
- `Vec<()>`: empty — count = 0, offset points to `[count:u32 = 0]` heap entry. Round-trips.
- Struct with no fields: `struct Empty {}` — 0 bytes inline. Round-trips.

### Deeply Nested Heap Tests

Validates the two-pass serialization algorithm with multi-level heap nesting:

- `Vec<Vec<String>>`: three levels of heap data (outer vec → inner vecs → string data).
- `Vec<Vec<Vec<u32>>>`: three levels of offset resolution.
- `Option<Vec<Option<String>>>`: mixed indirection types at multiple depths.
- `Vec<Option<Vec<u8>>>`: alternating vec/option nesting.

### Zero-Length String Tests

- Empty string `""` at various heap positions (first heap item, last, between others).
- Verify empty strings always allocate a `[len:u32 = 0]` heap entry — no offset-0
  optimization. The offset points into the heap region and the length prefix is `0x00000000`.
- Verify `Option<()>` with `Some(())` where offset points to exactly `buf.len()` (valid
  for 0-byte reads).

### File Format Fuzz Target

In addition to the bare-message fuzz target (`decode`), add a file-format
fuzz target:

```rust
// fuzz/fuzz_targets/file_reader.rs
fuzz_target!(|data: &[u8]| {
    let _ = pardosa_genome::Reader::new(data);
    // If Reader::new succeeds, try reading each message
    if let Ok(reader) = pardosa_genome::Reader::new(data) {
        for i in 0..reader.message_count().min(100) {
            let _ = reader.read_message::<FuzzStruct>(i);
        }
    }
});
```

### None Sentinel Tests

- `Option::<u32>::None` serializes offset as `0xFFFFFFFF`, not `0x00000000`.
- `Option::<u32>::Some(0)` serializes with a valid heap offset (not `0xFFFFFFFF`).
- Roundtrip `Option::None` and `Option::Some` for all scalar types.
- Verify that offset `0xFFFFFFFF` in a string/vec/map context (not Option) produces
  `DeError::OffsetOutOfBounds` (it's only valid as a None sentinel).

### Per-Message Checksum Tests (file format)

- File writer computes xxHash64 for each message and stores it in 24-byte index entries.
- File reader verifies per-message xxHash64 before deserialization.
- Corrupted message data → `DeError::ChecksumMismatch` on `read_message`.
- Checksum mismatch short-circuits before decompression when compression is enabled.
- Index entry with correct xxHash64: verification passes.
- Index entry with mismatching xxHash64: verification fails with `DeError::ChecksumMismatch`.

### Compression Tests (`zstd` feature)

- `encode_with_options(Zstd)` → `decode` round-trip for all data model types.
- Compressed output is smaller than uncompressed for strings and padded structs.
- `Writer::with_compression(Zstd { level: 3 })` sets header flags correctly
  (`compression_algo = 001`).
- `Reader` auto-detects compression from header flags and decompresses transparently.
- Level 1–22 all produce valid zstd compressed output.
- Empty message compresses and decompresses correctly (`msg_data_size=0` →
  decompressed empty buffer → `DeError::BufferTooSmall`).
- Compressed file with per-message xxHash64 — checksum is verified before decompression.
- File with `compression_algo ≠ 000` but `zstd` feature not enabled → `FileError::CompressionNotAvailable`.
- Crafted `msg_data_size = u32::MAX` with small compressed payload →
  `DeError::UncompressedSizeTooLarge` (default 256 MiB limit).
- Compressed bare message includes 1-byte algorithm identifier — verify it reads back
  correctly without caller specifying algorithm.
- Post-decompression trailing bytes → `DeError::PostDecompressionTrailingBytes`.

### Zstd-Specific Tests

- Zstd frames always include `Frame_Content_Size` (verify with `ZSTD_getFrameContentSize`
  or by parsing the frame header).
- Crafted zstd frame with `Window_Size > 2^22` → rejected by `max_zstd_window_log` →
  `DeError::DecompressionFailed`.
- Crafted zstd frame with `Frame_Content_Size > max_uncompressed_size` → rejected before
  allocation.
- `DecodeOptions::max_zstd_window_log = 17` (128 KiB) → frames with larger windows
  are rejected.
- Zstd round-trip preserves exact byte output for deterministic inputs (`BTreeMap` keys).
- Verify zstd decompressor memory stays within expected bounds (≈4 MiB default window).

### Deserialization Limit Tests

- `Vec<u32>` with `count = 100_000_000` hits `ElementLimitExceeded` with default PAGE0 config (256 elements).
- Verify `ElementLimitExceeded` fires **before** `Vec::with_capacity` is called
  (instrument with a custom allocator or verify no OOM on low-memory systems).
- Deeply nested `Option<Option<Option<...>>>` (200 levels) hits `DepthLimitExceeded`.
- Deeply nested newtype chain `struct A(B); struct B(C); ...` (200+ levels) hits
  `DepthLimitExceeded`. Newtypes are transparent in layout but increment depth.
- Enum unit variant with offset=0: passes verification (offset treated as padding, not
  subject to backward offset check). Non-zero offset on unit variant → `NonZeroPadding`.
- Custom config with `max_depth: 8` rejects nesting at depth 9.
- Custom config with `max_total_elements: 100` rejects a `Vec` with 101 elements.
- Custom config with `max_message_size: 1024` rejects bare messages with `msg_data_size > 1024`.
- Custom config with `max_uncompressed_size: 1024` rejects compressed messages > 1 KiB.
- Custom config with `max_zstd_window_log: 17` rejects zstd frames with larger windows.
- `usize::MAX` for all limits effectively disables them.
- Limits are enforced post-decompression (on uncompressed message structure), not on
  compressed wire format.
- Page class limits: PAGE0(256), PAGE1(4096), PAGE2(65536), PAGE3(1048576) — test each
  boundary with element counts at and above the limit.

### Sizing Consistency Tests

- Verify `encode` returns `SerError::InternalSizingMismatch` when `SizingSerializer` and
  `WritingSerializer` disagree (requires a mock type with side-effectful `Serialize` impl).

### File Format Integrity Tests

- `message_count × 24` overflow: crafted footer with `message_count = u64::MAX / 23` →
  `FileError::InvalidIndex`.
- Index offset validation: offset < 32 → rejected, offset + size > index_offset → rejected,
  non-monotonic offsets → rejected, overlapping ranges → rejected.

### Roundtrip Verification Tests

- `verify_roundtrip` passes for well-behaved types.
- `verify_roundtrip` returns `ValueMismatch` for `#[serde(untagged)]` enums
  (demonstrates the silent corruption detection).

### Schema Hash Tests

- Schema hash is computed deterministically from the serde type structure.
- Same struct type always produces the same 8-byte schema hash.
- Adding, removing, or reordering fields changes the schema hash.
- Changing a field type changes the schema hash.
- Schema hash is embedded in message header at bytes 2–9.
- `decode` with mismatched schema hash → `DeError::SchemaMismatch`.

### Golden Test Vectors (Format Conformance)

Round-trip tests prove `decode(encode(x)) == x` but do NOT prove the wire format matches
the spec. A serializer and deserializer can drift from the spec in lockstep while
round-trips pass — both produce wrong bytes, but they agree on the wrong bytes.

Golden test vectors are hand-constructed byte sequences matching the §Binary Format tables,
committed to `tests/golden/`. Each vector asserts two properties:

1. `encode(value) == expected_bytes` — the serializer produces spec-conformant output.
2. `decode(expected_bytes) == value` — the deserializer reads spec-conformant input.

This is the only mechanism that pins implementation to spec. Without golden vectors,
format drift is undetectable until a second implementation exists.

#### Vector Format

Each vector is a `.rs` file containing the Rust value, the expected byte sequence as a
hex-encoded constant, and byte-range annotations mapping to spec fields:

```rust
// tests/golden/bare_struct_u32_string.rs

/// Bare uncompressed message: struct { count: u32, name: String }
/// Value: TestStruct { count: 42, name: "hi" }
///
/// Wire layout (31 bytes total = 15-byte header + 16-byte data):
///   00..02  01 00                 format_version  1
///   02..0A  xx xx xx xx xx xx xx  schema_hash     (type-dependent)
///           xx
///   0A      00                    algo            uncompressed
///   0B..0F  10 00 00 00           msg_data_size   16 (inline 8 + heap 6 + pad 2)
///   ---- inline region (8 bytes) ----
///   0F..13  2A 00 00 00           count           42 (u32 LE)
///   13..17  08 00 00 00           name.offset     8 (heap starts at inline_size)
///   ---- heap region (6 data + 2 pad = 8 bytes) ----
///   17..1B  02 00 00 00           name.len        2
///   1B..1D  68 69                 name.data       "hi"
///   1D..1F  00 00                 (padding to 8-byte boundary)
const EXPECTED_BYTES: &[u8] = &[
    0x01, 0x00,                                     // format_version
    // schema_hash: 8 bytes, computed at test time via T::SCHEMA_HASH
    // algo: 0x00
    // msg_data_size: 0x10, 0x00, 0x00, 0x00       (16 bytes)
    // inline: 2A 00 00 00  08 00 00 00             (count=42, name.offset=8)
    // heap:   02 00 00 00  68 69  00 00            (name.len=2, "hi", pad)
    // ... (full byte sequence in actual test)
];

#[test]
fn bare_struct_u32_string_conformance() {
    let value = TestStruct { count: 42, name: "hi".into() };
    let encoded = pardosa_genome::encode(&value).unwrap();
    // Skip schema_hash bytes (type-dependent), verify everything else
    assert_eq!(encoded[0..2], EXPECTED_BYTES[0..2]);   // format_version
    assert_eq!(encoded[10], 0x00);                      // algo
    // ... remaining byte-level assertions
    let decoded: TestStruct = pardosa_genome::decode(&encoded).unwrap();
    assert_eq!(decoded, value);
}
```

#### Required Vectors

One vector per serde data model type, plus format-level vectors:

| Vector | Type | Key Property Verified |
|--------|------|-----------------------|
| `bool_true_false` | `bool` | 1-byte inline, values `0x00`/`0x01` |
| `integers_boundary` | `u8, i64, u128` | LE encoding, natural alignment, boundary values |
| `float_special` | `f64` | NaN bit preservation, -0.0, infinity |
| `char_unicode` | `char` | 4-byte LE u32, emoji encoding |
| `string_empty_nonempty` | `String` | 4B offset stub, heap `[len:u32][data]`, empty allocates `[0u32]` |
| `bytes_slice` | `Vec<u8>` | Same layout as string, no UTF-8 validation |
| `option_none_some` | `Option<u32>` | None sentinel `0xFFFFFFFF`, Some offset |
| `vec_elements` | `Vec<u32>` | Heap `[count:u32][elements]`, alignment padding |
| `btreemap_entries` | `BTreeMap<String, u32>` | Heap `[count:u32][key val pairs]`, sorted order |
| `struct_mixed_align` | `struct { a: u8, b: u64, c: u16 }` | Alignment padding bytes are `0x00` |
| `enum_all_variants` | Unit, newtype, tuple, struct variants | Discriminant + offset layout, unit offset=0 |
| `newtype_transparent` | `Meters(f64)` | Inner type layout, no wrapper overhead |
| `nested_heap` | `Vec<Vec<String>>` | Multi-level offset resolution |
| `bare_header` | Full bare message | 15-byte header: version, schema_hash, algo, msg_data_size |
| `bare_compressed` | Compressed bare message | 19-byte header: version, schema_hash, algo=0x01, compressed_size, msg_data_size |
| `file_header_footer` | File with 1 message | 32B header, index entry, 32B footer, xxHash64 |

Phase: 2D (immediately after encode/decode work), `bare_compressed` and `file_header_footer`
in Phase 3. Cost: hours, not days.

---

## Determinism

pardosa-genome enforces canonical encoding: the same logical value always produces the
same bytes. This is critical for content-addressable storage, deduplication, and
integrity checking (see ADR-032).

### Map key ordering

`BTreeMap` iterates in `Ord` order — guaranteed by the Rust standard library
documentation. Map entries are serialized in iteration order. `HashMap` and `HashSet`
are rejected at compile time (ADR-004) because their iteration order is non-deterministic.

The `GenomeOrd` marker trait (ADR-033) restricts `BTreeMap` keys and `BTreeSet` elements
to types with deterministic, total, platform-independent `Ord` implementations. Only owned
value types implement `GenomeOrd`:

| Implements `GenomeOrd` | Does not implement `GenomeOrd` |
|------------------------|--------------------------------|
| `bool`, `u8`–`u128`, `i8`–`i128`, `char`, `()` | `f32`, `f64` (no `Ord`) |
| `String` | `Box<T>`, `Arc<T>`, `Cow<T>` (use owned type) |
| `Option<T: GenomeOrd>` | `&str`, `&[u8]` (use `String`) |
| `[T: GenomeOrd; N]`, tuples (1–16) | `Vec<T>` (no `Ord`) |

Custom key types must implement `GenomeOrd` manually:

```ignore
#[derive(PartialEq, Eq, PartialOrd, Ord, GenomeSafe)]
struct MyKey { id: u64 }
impl GenomeOrd for MyKey {}
```

The `#[derive(GenomeSafe)]` macro auto-detects generic parameters in `BTreeMap` key
position and adds `GenomeOrd` bounds (see ADR-033 for details and limitations).

### Other determinism guarantees

- **Fixed field ordering**: struct fields serialize in declaration order (no reordering).
- **Deterministic heap layout**: breadth-first ordering with forward-pointing offsets (ADR-021).
- **Padding canonicalization**: all alignment padding bytes are `0x00` (ADR-018).
- **NaN bit-pattern preservation**: float NaN patterns round-trip exactly (ADR-024).
- **Runtime verification**: `verify_roundtrip` catches any canonical encoding violation
  that escapes compile-time checks.

---

## Safety Contract

### Always safe on arbitrary input

`decode` never panics and never produces undefined behavior on any input, including:
- Truncated buffers
- Random bytes
- Maliciously crafted offsets
- Invalid UTF-8

Every offset is bounds-checked with overflow-safe arithmetic. Every string is UTF-8
validated. Every char is validated as a Unicode scalar value. No `unsafe` code.

### DoS protection

Default deserialization limits prevent resource exhaustion on crafted inputs:
- **Depth limit** (128): prevents stack overflow from deeply nested types.
- **Element limit** (page class): prevents OOM from `Vec`/`Map` with huge `count` fields.
  Default PAGE0 allows 256 elements per message; higher page classes (PAGE1=4096,
  PAGE2=65536, PAGE3=1048576) are selected via file extension.

These limits are enforced before allocation occurs — a `Vec<T>` with `count` exceeding the
page class limit is rejected at the count check, before any memory is allocated for elements.

**Worst-case work bound.** The effective worst-case CPU cost per deserialization is
`O(max_total_elements × max_depth)`. With PAGE0 defaults: `256 × 128 = 32,768` operations.
Element counting includes all nested containers: each `SeqAccess::next_element` call
counts as 1, each `MapAccess::next_entry` call counts as 1 (counting the key–value pair
as a single element). Map keys and values that are themselves containers consume additional
budget from the same global counter. The page class value is the total element budget for
the entire message — not per-container.

### No pointer casts

All scalar reads use `from_le_bytes` on byte slices copied from the buffer. No pointer
casting, no alignment requirements on the input buffer. Safe to use with unaligned `mmap`,
network buffers, or any `&[u8]`.

**mmap caveat**: mmap-based reading is only safe if the underlying file is not concurrently
modified. Use `MAP_PRIVATE` or read the file into a private buffer before deserializing.
Concurrent modification through a shared mapping invalidates all structural guarantees
(TOCTOU: CRC passes, then offsets change before deserialization reads them).

### Verification as defense-in-depth

`decode` (and `decode_with_options`) performs structural verification inline during
deserialization — no separate verification pass is needed:
- Padding bytes are zero (hard error: `NonZeroPadding`)
- No backward offsets (hygiene check; depth + element limits are the real cycle defense)
- Bool values are exactly 0 or 1
- Schema hash verified against expected type
- Per-message xxHash64 (in file format)

These checks run on every deserialization. The always-verify design eliminates the risk of
calling the wrong function and processing unverified data.

### Integrity boundaries

**xxHash64 is corruption-detection, not tamper-detection.** xxHash64 is a non-cryptographic hash.
An attacker can compute xxHash64 values for arbitrary modifications. Per-message
and footer xxHash64 detect accidental corruption (disk errors, truncation) but provide zero
protection against intentional tampering. For tamper detection, use an application-level
HMAC or the v2 AEAD feature.

**Bare messages have no integrity mechanism.** The only validation on bare messages is
structural (bounds checks, UTF-8, schema hash). A single bit flip in a scalar field
produces silently wrong data as long as it doesn't violate structural invariants. Callers
must rely on transport-level integrity (TLS, QUIC) for bare messages over untrusted channels.

### Memory usage during serialization

Serialization allocates ≈1× the final message size via two-pass direct serialization
(`SizingSerializer` computes size with no allocation, `WritingSerializer` writes into a
single pre-allocated buffer). See [Serialization Algorithm](#serialization-algorithm).

---

## Operational Guidance

### Crash-safe file writes

`Writer::finish()` returns a `Vec<u8>`. The caller is responsible for writing this
atomically to disk. **If the process crashes mid-write, the file is unrecoverable** —
there is no valid footer.

**Recommended pattern (POSIX):**

```rust
let data = writer.finish()?;
let tmp = format!("{}.tmp", path);
let file = std::fs::File::create(&tmp)?;
file.write_all(&data)?;
file.sync_all()?;  // data + metadata to disk before rename
drop(file);
std::fs::rename(&tmp, &path)?;  // atomic on same filesystem
```

**Note:** `fsync` (`sync_all()`) before `rename` is required for crash safety on Linux/ext4.
Without it, a power failure after `rename` can leave the file with correct metadata but
zero-filled data blocks. The `tempfile` crate's `NamedTempFile::persist()` does NOT call
`fsync` by default — call `as_file().sync_all()` before `persist()`.

**Recommended pattern (cross-platform):**

Use the `tempfile` crate with `NamedTempFile`. Call `as_file().sync_all()` before
`persist()`.

### Serialization memory budget

Peak memory during `encode` is ≈1× the final message size (two-pass direct serialization
eliminates the intermediate `Value` tree). The `SizingSerializer` first pass uses only
stack space; the `WritingSerializer` second pass writes into a single pre-allocated `Vec<u8>`.

| Final message size | Peak memory (approx) | Notes |
|-------------------|---------------------|-------|
| < 100 MiB | ≈ message size | No concern |
| 100 MiB – 1 GiB | ≈ message size | Monitor process RSS |
| > 1 GiB | ≈ message size | Split across multi-message file if RSS is a concern |

### Content-addressed storage

Use `BTreeMap` (not `HashMap`) for all map types when the serialized bytes will be
hashed for caching, deduplication, or content addressing. `HashMap` iteration order is
non-deterministic — identical logical values produce different byte sequences.

### Schema drift detection

Schema identity is enforced at two levels:
1. **Schema hash** (8-byte xxHash64 in message header): catches type mismatches at decode time
   with `DeError::SchemaMismatch`.
2. **Pardosa migration model**: single schema per file, new file on schema change.

Use `verify_roundtrip` in CI to catch accidental field changes:

```rust
#[test]
fn model_types_roundtrip() {
    pardosa_genome::verify_roundtrip(&MyModel::default()).unwrap();
}
```

### `#[serde(untagged)]` and `#[serde(tag)]` detection

`#[derive(GenomeSafe)]` rejects `#[serde(untagged)]`, `#[serde(tag)]`, and
`#[serde(tag, content)]` at compile time. Only externally tagged enums (serde's default,
no attribute) are supported.

For types that bypass `GenomeSafe` (manual `Serialize` impls, transitive dependencies),
`verify_roundtrip` catches silent corruption in CI:

```rust
#[test]
fn model_types_roundtrip() {
    pardosa_genome::verify_roundtrip(&MyEnum::Variant1(42)).unwrap();
    pardosa_genome::verify_roundtrip(&MyEnum::Variant2("x".into())).unwrap();
}
```

Run `verify_roundtrip` for every enum type in your data model as part of CI. This is the
defense-in-depth for types that bypass `GenomeSafe`.

### Silent type confusion on non-self-describing formats

pardosa-genome is not self-describing — no type tags or field names are stored in the
binary. An 8-byte xxHash64 schema hash in the message header catches type mismatches at
decode time (`DeError::SchemaMismatch`). The 8-byte width makes accidental collisions
practically impossible (~4 billion birthday bound), but the hash is not cryptographic —
an adversary could craft a colliding type. Schema identity is further enforced by the
pardosa migration model (one schema per stream/file, new stream on schema change).
Including the root type name in the hash input distinguishes newtypes with different
semantics (e.g., `Meters(f64)` vs `Seconds(f64)`).

**Cross-subject risk**: if a publisher accidentally sends type `A` messages on a subject
where consumers expect type `B`, the schema hash will catch it in most cases. For types
with identical serde structure (same field types in same order), the hash will match and
deserialization will produce semantically wrong data. **Mitigation**: use separate streams
per type, enforce via the pardosa generation/migration model, and run `verify_roundtrip`
in CI for all message types.

### `#[serde(skip)]` and `#[cfg(...)]` cross-compilation

`#[serde(skip)]` and `#[cfg(debug_assertions)]`-gated fields change the binary layout.
If the serializer and deserializer are compiled with different feature flags or profiles
(e.g., debug vs release), deserialization produces garbage or errors.

**CI gate**: run `verify_roundtrip` for all message types in both debug and release
profiles. If a `#[cfg]`-gated field changes the layout, the round-trip will fail:

```rust
#[test]
fn layout_stable_across_profiles() {
    // Run this test in both `cargo test` and `cargo test --release`
    pardosa_genome::verify_roundtrip(&MyStruct::default()).unwrap();
}
```

### Compression selection

v1 supports **zstd only**. Brotli is deferred to a future version.

For real-time pipelines, zstd at level 3 provides an excellent speed/ratio tradeoff:
- Zstd decompression: ≈1500 MB/s
- Zstd compression at level 3: ≈300 MB/s
- Configurable levels 1–22 for latency vs ratio tuning

For cold storage files where write speed is not a concern, consider higher zstd levels
(e.g., 19–22) for improved compression ratio.

### JetStream storage compression

JetStream's `Compression: s2` setting compresses data at the NATS storage layer
(transparent to clients). Genome's compression operates at the application layer (visible
in message bytes). Both can be active simultaneously. They serve different purposes: genome
compression reduces network bandwidth; JetStream s2 reduces disk usage. Neither is a
substitute for the other.

When genome compression is enabled, JetStream s2 on already-compressed genome data
achieves near-zero additional compression while consuming CPU on every store/retrieve.
In practice, choose one layer: genome compression for bandwidth-sensitive pipelines,
JetStream s2 for uncompressed genome messages where disk savings matter.

### Concurrent serialization memory

Peak memory during `encode` is ≈1× the final message size per concurrent serialization
(two-pass direct, no intermediate `Value` tree). With N concurrent serializations, total
serialization buffer memory is ≈N × message_size. Use application-level semaphores or task
concurrency limits to bound memory when serializing many large messages simultaneously.

### Monitoring and observability

For production deployments, instrument the following metrics:

| Metric | Type | Purpose |
|--------|------|---------|
| `pardosa_genome_serialize_duration_seconds` | Histogram | Serialization latency |
| `pardosa_genome_deserialize_duration_seconds` | Histogram | Deserialization latency |
| `pardosa_genome_message_bytes` | Histogram | Serialized message size |
| `pardosa_genome_compressed_ratio` | Histogram | Compression ratio (compressed/uncompressed) |
| `pardosa_genome_checksum_mismatch_total` | Counter | xxHash64 integrity failures |
| `pardosa_genome_verification_failed_total` | Counter | Structural verification failures (padding, bools, offsets) |
| `pardosa_genome_decompression_rejected_total` | Counter | Decompression bomb rejections |

These are not part of the crate API — instrument at the application level using the
error types returned by the public API.

---

## Limitations & Non-Goals

### No schema evolution

Changing a struct's fields (adding, removing, reordering, or changing types) is a **breaking
change** that produces deserialization errors or silent corruption. This is the explicit
tradeoff for maximum read performance. Schema identity is enforced by the pardosa migration
model: one schema per file/stream, new file/stream on schema change. Use `verify_roundtrip`
in CI to catch layout regressions before deployment.

### No streaming

The entire message buffer must be available before deserialization. No progressive/streaming
reads. This matches FlatBuffers' design.

### No self-describing format

The binary contains no type tags, field names, or structural metadata. An 8-byte xxHash64
schema hash provides type mismatch detection but not introspection. The reader must know the
exact Rust type to deserialize. This is the explicit tradeoff for compact size and read
speed. For debugging, serialize to JSON or RON using the same serde derives.

### Platform

Little-endian only. Big-endian targets must byte-swap every scalar read (handled
automatically by `from_le_bytes`, but with a per-read cost).

### 4 GiB per-message limit

u32 offsets limit individual messages to 4 GiB. Files can be arbitrarily large (u64 message
offsets in the index).

### Not a database

pardosa-genome is a serialization format, not a storage engine. It does not support:
- Transactions or ACID guarantees
- Concurrent writers
- Crash-safe append (appending requires rewriting the footer/index;
  see [Operational Guidance](#operational-guidance) for atomic write patterns)
- Partial updates to existing messages

### Unsupported serde attributes

All of the following are **rejected at compile time** by `#[derive(GenomeSafe)]` via
syntactic analysis of the derive input. Runtime detection in the serializer remains as
defense-in-depth for types that bypass `GenomeSafe` (manual `Serialize` impls).

- `#[serde(flatten)]` — incompatible with fixed-layout struct serialization. Serde emits
  `serialize_map` instead of `serialize_struct`.
- `#[serde(tag = "...")]` — internally tagged enums use map-based encoding incompatible
  with pardosa-genome's fixed discriminant-based enum layout. Only externally tagged enums
  (serde's default, no attribute) are supported.
- `#[serde(tag = "...", content = "...")]` — adjacently tagged enums, same issue.
- `#[serde(untagged)]` — serde silently bypasses variant serialization methods, causing
  the inner type to be serialized directly without a discriminant. This produces **silent
  data corruption** on round-trip if not caught. `GenomeSafe` catches it at compile time.
  For types that bypass `GenomeSafe`, use `verify_roundtrip` in CI.
- `#[serde(skip_serializing_if)]` — conditionally omits fields at runtime based on a
  predicate. In a fixed-layout format, this causes data-dependent layout breakage where
  different messages of the same type have different layouts.

### Platform-dependent types

`usize` and `isize` are **rejected at compile time** by `#[derive(GenomeSafe)]`. Their
size varies by target platform (32-bit on 32-bit targets, 64-bit on 64-bit targets),
breaking cross-platform schema compatibility. Use `u32`/`u64`/`i32`/`i64` explicitly.
The rejection is recursive — `Vec<usize>`, `Option<isize>`, and other containers wrapping
these types are also rejected.

### Strict type-identity hashing

`String`, `str`, `&str`, and `Cow<'_, str>` produce **different schema hashes** despite
identical serde wire representation. `&str` and `Cow<'_, str>` share `str`'s hash
(transparent delegation), but `String` differs. Changing a field between these types
is a schema-breaking change. This is intentional: the schema hash tracks Rust type
identity, not serde data model equivalence.

`Box<T>` and `Arc<T>` are exceptions — both delegate to `T`'s hash (transparent wrappers).
Wrapping or unwrapping `Box`/`Arc` is schema-compatible.

`PhantomData<T>` always hashes as `"PhantomData"` regardless of `T`. Changing the phantom
type parameter is NOT a schema-breaking change. This is correct — `PhantomData` occupies
zero bytes on the wire.

### Excluded wrapper types

`Rc<T>` has no `GenomeSafe` implementation. `Rc` is `!Send`, making it incompatible with
async runtimes (Tokio, Axum). Use `Arc<T>` for shared ownership in serializable types.

### Supported serde attributes with caveats

- `#[serde(with = "module")]` — **supported**, but changing the module is a **breaking
  change** to the binary layout. The custom serialization module controls which serde data
  model type is emitted; changing it alters field sizes and alignment. Treat module changes
  the same as field type changes.

### `#[serde(skip)]` and conditional compilation

`#[serde(skip)]` excludes a field from serialization. If the serializer and deserializer
disagree on which fields are skipped (e.g., different `#[cfg(...)]` flags at compile time),
the binary layout silently misaligns and deserialization produces garbage or errors. This is
the same class of problem as schema evolution — treat `skip` changes as breaking changes.
Use `verify_roundtrip` in CI across all build profiles to detect layout drift.

### Tuple struct / tuple wire equivalence

Tuple structs (`struct Point(f64, f64)`) and plain tuples (`(f64, f64)`) produce identical
wire representations. Deserializing one as the other succeeds silently. This is intentional
— the format does not store type names.

---

## Future Scope (v2)

The following features were evaluated during the design phase and explicitly deferred to
a future format version. They are documented here to establish design intent, prevent
scope creep, and ensure the v1 wire format reserves the necessary extensibility points.

### Encryption

Full AEAD encryption with:

- **Ciphers**: XChaCha20-Poly1305 and AES-256-GCM-SIV (no default — caller must choose).
- **Key derivation**: HKDF-SHA256 with a per-file CSPRNG salt for file-mode key derivation.
- **Nonces**: deterministic counter nonces for file mode, random nonces for bare messages.
  Counter nonces require that the same key is never reused across files — counter reuse
  under the same key is catastrophic for XChaCha20-Poly1305 (full plaintext XOR leakage
  + forgery). AES-256-GCM-SIV degrades more gracefully under nonce misuse (retains
  authenticity, leaks only plaintext equality, not full XOR), but confidentiality is
  still compromised. File-mode key derivation via HKDF with per-file salt enforces this
  constraint.
- **AAD construction**: mode-specific (bare, file, JetStream) with exact byte-level
  definitions. Bare AAD: `b"PGNO-BARE-V1" || mode_tag || compression_tag`. File AAD:
  message index bound to file header and encryption header. JetStream AAD: canonical
  `Pardosa-*` metadata.
- **Ordering**: compress-then-encrypt (with documented length leakage — pad when length
  confidentiality matters).
- **`_with_rng` API pattern**: std convenience wrappers use OS entropy; `no_std` / custom
  environments must supply a `CryptoRng + RngCore` explicitly.

The file header `flags` field (bits 3–15 reserved) provides space for encryption flag
bits. The v1 requirement to reject non-zero reserved bits ensures forward compatibility —
v1 readers will cleanly reject v2 encrypted files rather than silently misparsing them.

### Brotli Compression

Brotli as an alternative to zstd, with symmetric safety checks (`max_brotli_window_bits`
in `DecodeOptions`, pre-decompression window bit parsing). Deferred because zstd covers
the primary use case (real-time network/MLOps pipelines) with ≈4× faster decompression
and ≈25× faster compression than brotli at comparable ratios.

The file header `compression_algo` field (3 bits, values `010`–`111` reserved) provides
space for brotli and future compression algorithms.

### Bare Message Checksum Trailer

Optional xxHash64 trailer appended to bare messages for corruption detection on transports
without application-level integrity (raw TCP, UDP, shared memory IPC). Layout:

```
[bare_message][checksum:u64]
```

xxHash64 (seed 0) covers all preceding bare message bytes. Accepted only by checksum-aware APIs
(`encode_checksummed` / `decode_checksummed`); standard `decode` rejects the trailer
as trailing bytes. Feature-gated behind a `checksum` feature flag.

### Zstd Dictionary Compression

The file header reserves `dict_id` (u32 at offset 16) for zstd dictionary identification.
v1 requires `dict_id = 0` and rejects non-zero values. Dictionary distribution is
out-of-band (file path, NATS Object Store, etc.).

Dictionary compression improves ratios 2–5× on small messages (<4 KiB) with repeated
schema structures. Implementation is deferred until real-world payload size data is
available from initial deployments.

### `ruzstd` for `no_std` Decompression

A pure-Rust zstd decompressor (`ruzstd`) behind a `zstd-nostd` feature flag for read-only
consumers in embedded or `no_std` contexts. The current `zstd` crate wraps C libzstd via
`zstd-sys` and requires `std::io`. A `no_std` decompression path would enable embedded
devices to read genome files without linking against C libzstd. Deferred because this is a
niche use case — evaluate after v1 deployment identifies embedded consumers.

### Exportable Schema Definition Format

A structured schema descriptor emitted by the `GenomeSafe` derive macro for cross-language
code generation tools. Drawing on RON's algebraic type model and Smithy's IDL patterns.
Prototyped during Phase 2B.3 (schema hash research), finalized as a separate deliverable.
External tools would compute the schema hash from the descriptor, verifying compatibility
without access to the Rust source.

### ValueSerializer Intermediate Representation

A 13-variant `Value` enum for intermediate representation during serialization. Evaluated
and rejected for v1 because the two-pass direct approach (SizingSerializer + WritingSerializer)
achieves ≈1× peak memory vs the 3–6× memory amplification of a Value tree. May be
reconsidered if use cases emerge that require inspecting or transforming serialized data
before writing (e.g., schema migration transforms).

---

## Quality Tooling

Tools beyond standard `cargo test` that make pardosa-genome a production-quality file
format. Organized by adoption priority — Tier 1 tools are adopted from Phase 2 day one;
Tier 2 at specific milestones; Tier 3 deferred.

### Tier 1 — Phase 2 Day One

#### Golden Test Vectors (Format Conformance)

Round-trip tests prove encode/decode consistency but not spec conformance. Golden test
vectors are hand-constructed byte sequences matching §Binary Format tables, committed
to `tests/golden/`. They verify `encode(value) == expected_bytes` AND
`decode(expected_bytes) == value`. This is the **only mechanism that pins implementation
to spec** — without golden vectors, the serializer and deserializer can drift from the
spec in lockstep while round-trips pass.

16 vectors covering every serde data model type and both wire formats (bare message,
file container). See §Test Plan: Golden Test Vectors for the full vector list and
format specification.

Phase: 2D. Cost: hours.

#### 32-bit CI Cross-Compilation

`genome-sec.md` finding #18 specifies u64 widening for all offset arithmetic to prevent
overflow on 32-bit platforms. The CI matrix tests feature flags but not target
architectures. Add `cargo test --target i686-unknown-linux-gnu` to CI.

This validates that `(offset as u64) + (len as u64) + 4 <= (buf_len as u64)` is
applied consistently and that no code path relies on `usize == u64`.

```yaml
# .github/workflows/ci.yml (addition)
- name: Test on 32-bit
  run: |
    rustup target add i686-unknown-linux-gnu
    cargo test --target i686-unknown-linux-gnu -p pardosa-genome
```

Phase: 2C. Cost: one CI job.

#### Code Coverage (cargo-llvm-cov)

Coverage measurement from Phase 2A onward. Target: ≥90% line coverage. Focus areas:

- Error paths in `MessageDeserializer` (rare conditions like `NonZeroPadding`,
  `BackwardOffset`, `InvalidChar` at surrogates)
- All verification check branches
- Feature-gated code paths (`core`-only, `alloc`, `std`, `zstd`)
- Empty-container edge cases (empty string, empty vec, 0-message file)

```sh
cargo llvm-cov --workspace --html -p pardosa-genome
# Open target/llvm-cov/html/index.html
```

Phase: 2A+. Cost: one CI job.

### Tier 2 — Milestone-Gated

#### Kani Bounded Model Checking (Offset Arithmetic Proofs)

Kani (Amazon's bounded model checker for Rust) proves properties exhaustively over the
input space — not sampling like proptest, but mathematical proof within bounds. The crate
has zero `unsafe` by design, so Kani's primary value is proving absence of arithmetic
overflow in offset computations.

3–5 proof harnesses behind `#[cfg(kani)]`:

```rust
// src/kani_proofs.rs

/// Prove: the u64-widened bounds check correctly rejects all out-of-bounds
/// reads AND accepts all in-bounds reads. Models the actual deserializer
/// check from genome-sec.md finding #18.
#[cfg(kani)]
#[kani::proof]
fn offset_bounds_check_soundness() {
    let offset: u32 = kani::any();
    let len: u32 = kani::any();
    let buf_len: u32 = kani::any();  // message data region size

    // The widened bounds check used by the deserializer:
    let end = (offset as u64) + (len as u64);
    let in_bounds = end <= buf_len as u64;

    if in_bounds {
        // Prove: if the widened check passes, the read [offset..offset+len]
        // is genuinely within [0..buf_len] — no false accepts
        assert!(offset as u64 + len as u64 <= buf_len as u64);
        // Prove: the unwrapped u32 arithmetic would also be safe (no wrap)
        assert!(offset.checked_add(len).is_some());
        assert!(offset + len <= buf_len);
    } else {
        // Prove: if the widened check rejects, at least one of these is true:
        //   (a) the read extends past buf_len, OR
        //   (b) offset + len would overflow u32
        let would_overflow = offset.checked_add(len).is_none();
        let extends_past = (offset as u64) + (len as u64) > buf_len as u64;
        assert!(would_overflow || extends_past);
    }
}

/// Prove: alignment padding computation is correct and cannot overflow.
#[cfg(kani)]
#[kani::proof]
fn alignment_padding_correct() {
    let cursor: u32 = kani::any();
    let align: u32 = kani::any();
    kani::assume(align > 0 && align <= 16 && align.is_power_of_two());
    // Realistic message size — prevents u32 wrap in cursor + padding
    kani::assume(cursor <= u32::MAX - 16);

    let padding = (align - (cursor % align)) % align;
    // Prove: cursor + padding does not overflow u32
    assert!(cursor.checked_add(padding).is_some());
    let aligned = cursor + padding;
    // Prove: result is aligned
    assert!(aligned % align == 0);
    // Prove: padding is minimal (< alignment)
    assert!(padding < align);
    // Prove: aligned >= cursor (we moved forward, not backward)
    assert!(aligned >= cursor);
}

/// Prove: heap offset validation guarantees readable bytes at the target.
/// Models the deserializer's heap read: read `read_len` bytes starting at
/// `offset` within a buffer of `buf_len` bytes, where offsets must be in
/// the heap region (>= inline_size) and forward-only.
#[cfg(kani)]
#[kani::proof]
fn heap_read_within_bounds() {
    let inline_size: u32 = kani::any();
    let offset: u32 = kani::any();
    let read_len: u32 = kani::any();
    let buf_len: u32 = kani::any();
    kani::assume(inline_size <= buf_len);
    kani::assume(offset != 0xFFFF_FFFF);  // None sentinel

    // The deserializer's validation sequence:
    let forward = offset >= inline_size;                      // not backward
    let end_ok = (offset as u64) + (read_len as u64) <= buf_len as u64;  // within buffer

    if forward && end_ok {
        // Prove: the slice [offset..offset+read_len] is valid
        assert!(offset as u64 + read_len as u64 <= buf_len as u64);
        assert!(offset.checked_add(read_len).is_some());
        // Prove: the entire read is in the heap region
        assert!(offset >= inline_size);
        let end = offset + read_len;
        assert!(end <= buf_len);
    } else {
        // Prove: rejection is justified — at least one invariant is violated
        let backward = offset < inline_size;
        let extends_past = (offset as u64) + (read_len as u64) > buf_len as u64;
        assert!(backward || extends_past);
    }
}
```

Run on-demand or nightly (Kani is slow — minutes per harness):

```sh
cargo kani --harness offset_bounds_check_soundness
cargo kani --harness alignment_padding_correct
cargo kani --harness heap_read_within_bounds
```

Phase: after 2C. Cost: 1–2 days for 5 harnesses.

#### genome-dump CLI (Format Debugging)

A built-in diagnostic binary for inspecting genome files and bare messages. Always in
sync with the implementation — no external format description to maintain. See
§Crate Structure for details and output format.

Phase: 3B (when file format exists). Cost: ~200 LOC.

#### Deterministic Benchmarks (iai-callgrind)

Criterion measures wall-clock time, which varies across CI runs. `iai-callgrind` measures
instruction counts — deterministic across runs, making it reliable for CI regression gates.

Adopt only if criterion benchmarks show >5% variance on the CI runner. For
pardosa-genome, the serialization hot path is deterministic (no I/O, no post-pass-1
allocation), so criterion variance should be low.

```toml
# Cargo.toml (conditional addition)
[dev-dependencies]
iai-callgrind = "0.14"

[[bench]]
name = "genome_iai"
harness = false
```

Benchmark targets: `encode` throughput (MB/s), `decode` throughput (MB/s),
compressed vs uncompressed round-trip, file `Writer::push` + `Reader::read_message`.

Phase: 3+. Cost: one crate addition.

### Tier 3 — Deferred

#### cargo-semver-checks (API Stability)

Detects accidental breaking changes to the public API. Not applicable pre-1.0 with no
downstream crates. Add to CI at 0.9 or when the first external crate depends on
pardosa-genome.

```sh
cargo semver-checks check-release
```

#### Kaitai Struct (Cross-Language Format Description)

A `.ksy` file (Kaitai Struct YAML) describing the genome binary format. Generates
parsers in 11+ languages (Python, Java, JavaScript, C++, etc.). Enables non-Rust tools
to read genome files and validates the spec independently from the Rust implementation.

**Deferred because:** maintaining a `.ksy` file in sync with the Rust implementation
during pre-1.0 format evolution is a liability. Every spec change requires coordinated
`.ksy` updates with no automated consistency check. The `genome-dump` CLI (Tier 2)
provides immediate debugging value without the maintenance cost.

**Adopt when:** the format stabilizes post-1.0, OR a concrete non-Rust consumer exists.

#### cargo-mutants (Mutation Testing)

Systematically mutates source code and checks whether tests catch each mutation. For
pardosa-genome, the round-trip oracle (proptest) and fuzz coverage (cargo-fuzz) already
exercise the mutation-equivalent question ("does changing this code break a test?").
Mutation testing adds marginal signal given these strong oracles.

**Deferred indefinitely.** Revisit only if `cargo-llvm-cov` reveals dead code paths
that survive proptest + cargo-fuzz.

### TLA+ for JetStream KV Cutover

The JetStream integration (`pardosa-genome-nats`, `pardosa-next.md` §Phase 5) describes
a distributed coordination protocol:

1. Writer creates new JetStream stream
2. Writer publishes first-message metadata to new stream
3. Writer atomically updates KV registry pointer
4. Old stream retained during grace period
5. Consumers re-read first-message metadata after KV generation change

This involves concurrent writers and consumers, ordering assumptions, and partial failure
modes — the exact domain where TLA+ model-checking catches bugs that unit tests miss.

File: `spec/tla/JetStreamCutover.tla`

Variables: writer state, N consumer states, KV registry value, two stream generations,
first-message metadata presence.

Safety invariants:
- No consumer processes messages from stale stream after observing new KV generation
- No consumer uses stale metadata after generation change
- Partial writer failure (crash between stream creation and KV update) does not leave
  consumers in a broken state
- KV pointer, `Pardosa-Generation`, and `NatsConsumer::generation()` converge

**Adopt when:** JetStream integration implementation planning begins (pre-Phase 4.8).

### Smithy for JetStream Header Protocol

The `Pardosa-*` JetStream header protocol (`pardosa-next.md` §Phase 5) is a
service contract with defined header names, value constraints, canonicalization rules,
and error conditions. A Smithy model would formalize this contract for publisher/consumer
interoperability.

**Scope:** narrow — models the NATS metadata layer, not the binary format. Separate from
`pardosa-web.smithy` (HTTP operations).

File: `spec/smithy/pardosa-genome-nats.smithy`

**Adopt when:** JetStream integration implementation planning begins (pre-Phase 4.8).

### Tool Adoption Summary

| Priority | Tool | Phase | Value |
|----------|------|-------|-------|
| 1 | Golden test vectors | 2D | Pins spec to bytes — only cross-implementation conformance mechanism |
| 2 | 32-bit CI target | 2C | Validates offset arithmetic u64 widening on actual 32-bit target |
| 3 | cargo-llvm-cov | 2A+ | Coverage visibility, dead error path detection |
| 4 | Kani (scoped) | 2C+ | Mathematical proof of offset arithmetic correctness |
| 5 | genome-dump CLI | 3B | Developer debugging, format education |
| 6 | iai-callgrind | 3+ | CI-stable performance regression gate (conditional) |
| 7 | TLA+ (JetStream cutover) | Pre-4.8 | Distributed protocol safety verification |
| 8 | Smithy (JetStream headers) | Pre-4.8 | Metadata protocol contract formalization |
| 9 | cargo-semver-checks | 0.9+ | API stability for downstream consumers |
| 10 | Kaitai Struct | Post-1.0 | Cross-language format description and parsing |
| 11 | cargo-mutants | Deferred | Marginal signal given strong round-trip + fuzz oracles |
