# GEN-0035. In-House Canonical Encoding

Date: 2026-05-15
Last-reviewed: 2026-05-15
Tier: A
Status: Accepted

## Related

References: GEN-0001, GEN-0032, GEN-0033, PAR-0024, CHE-0045
Supersedes: GEN-0007, GEN-0012, GEN-0017, GEN-0020, GEN-0022, GEN-0023

## Context

The genome-typing v2 work (decision record
`.ooda/decision-genome-structural-20260514T214716Z.md`, decisions D-α
in-house canonical encoding and D-γ xxh3-128) replaces the legacy
FlatBuffers-style offset+heap binary layout (GEN-0007 family) with an
in-house sequential canonical encoding owned by the workspace. The
encoding is byte-shape identical to the borsh-1.5 canonical form but
implemented in-house so the workspace controls the spec, the trait
sealing, the decoder bound semantics, and the SCHEMA_HASH width
(xxh3-128 instead of xxh64).

The legacy offset-based layout (inline region + heap region addressed
by u32 offsets, breadth-first heap ordering, 4 GiB per-message cap,
0xFFFFFFFF None sentinel, 8-byte enum stub) has no representation in
the new format and is wholesale superseded. Two earlier ADRs are
partially superseded: GEN-0001 (R2 widens `SCHEMA_HASH: u64` →
`SCHEMA_HASH: u128`) and GEN-0003 (algorithm xxh64 → xxh3-128). The
remaining substance of GEN-0001 (marker-trait pattern, no mirror
types, no external schema) and of GEN-0003 (compile-time fingerprint,
frozen inputs, equivalence classes) carries forward.

PAR-0024 ratified `pardosa-derive` as the substrate-agnostic
event-invariant carrier. GEN-0035 is the GEN-domain wire-format ADR
that the carrier crate's traits emit conformant implementations for.

## Decision

The pardosa wire format is an in-house canonical sequential encoding.
Every value has exactly one byte representation; encode/decode is
deterministic and length-prefix driven; there is no offset region and
no heap region.

### Primitive encoding

| Type | Encoding |
|---|---|
| `u8` / `i8` | 1 byte |
| `u16` / `i16` | 2 bytes LE |
| `u32` / `i32` | 4 bytes LE |
| `u64` / `i64` | 8 bytes LE |
| `u128` / `i128` | 16 bytes LE |
| `f32` | 4 bytes LE IEEE 754 |
| `f64` | 8 bytes LE IEEE 754 |
| `bool` | 1 byte: `0u8` = false, `1u8` = true (other values reject) |

### Composite encoding

- **`Option<T>`** — tag byte `0u8` (None) or `1u8` (Some); when Some,
  immediately followed by the encoding of `T`. No other tag value is
  accepted.
- **Length-prefixed sequences** — `Vec<T>`, `String`, `&[u8]`, byte
  sequences, and `BTreeMap<K, V>` are encoded as `[len:u32 LE]`
  followed by the encoded elements in canonical order.
- **`String`** — `[len:u32 LE][utf8 bytes]`. The length is the byte
  length, not the codepoint count.
- **`BTreeMap<K, V>`** — `[count:u32 LE]` followed by entries in
  ascending order of `K`'s canonical encoding (sort the *encoded
  bytes* of each key, not the key value). `HashMap<K, V>` is not part
  of the canonical surface and is rejected by the EventSafe sealing.
- **Tuples and tuple structs** — elements encoded back-to-back in
  declaration order, no length prefix.
- **Unit / unit struct** — zero bytes.
- **Newtype struct** — transparent: encoded as the inner type.
- **Struct with named fields** — fields encoded back-to-back in
  declaration order, no length prefix.
- **Enums** — `[discriminant:u8]` followed by the variant payload.
  The discriminant is the **explicit `repr(u8)` discriminant value**
  declared in the Rust source (e.g. `Internal = 7` emits `0x07`); it
  is *not* the 0-indexed variant position. Variants must be `repr(u8)`
  so the discriminant fits one byte; this caps the canonical surface
  at 256 variants per enum. Internally tagged, adjacently tagged, and
  untagged representations are not supported.

### Trait stack and sealing

- `Encode` and `Decode` are sealed traits. Sealing is enforced via a
  private supertrait pattern: each trait extends a private
  `seal::Sealed` marker that downstream crates cannot implement,
  ensuring all conformant implementations are emitted by the
  workspace's derive macros.
- The trait stack is `EventSafe ⊂ GenomeSafe ⊂ GenomeOrd` in
  implementor-set terms (each named trait is *more* constrained than
  the next; in Rust syntax `trait GenomeOrd: GenomeSafe` and
  `trait GenomeSafe: EventSafe`). `EventSafe` lives in
  `pardosa-derive` per PAR-0024:R1; `GenomeSafe` and `GenomeOrd`
  remain in `pardosa-genome`.
- Both `Encode` and `Decode` extend `EventSafe` so wire-emission
  conformance and event-invariant conformance are inseparable: the
  type system rejects encoding of a non-`EventSafe` value.

### Strict decode

The decoder reads one value, returns it, and reports any remaining
input bytes as an error. There is no trailing-bytes tolerance, no
buffered re-entry, and no skip-unknown behaviour.

### Decoder cap

The decoder accepts a configurable byte cap; the default is **1 MiB**
(`1 << 20` bytes). Behaviour:

- Any length-prefix header (`u32 LE`) whose value exceeds the
  remaining cap budget causes immediate `DecodeError::CapExceeded`
  before allocation. The decoder does not allocate the requested
  capacity speculatively.
- Buffer growth during decode uses bounded doubling: the working
  buffer doubles until it reaches the cap, after which growth is
  capped to the remainder. This bounds peak allocation to `2 × cap`
  for any single decode call.
- The cap is per-decode-invocation, not per-process.

### Schema hash

`SCHEMA_HASH` is a `u128` produced by xxh3-128 over a deterministic
schema-bytes encoding (root type name, field names, field types,
enum variant names and discriminants, in canonical declaration
order). The schema-bytes encoding is itself frozen — any change
invalidates all persisted data. The frozen-input rules of GEN-0003
(seed, struct/enum/variant prefixes, primitive name canonicalisation,
PhantomData ignoring `T`, str/String/bytes/Vec<u8> equivalence
classes) carry forward unchanged; only the algorithm and width
change.

### Rules

R1 [4]: All multi-byte primitives are little-endian; bool is one byte
  with values 0u8 false and 1u8 true and other values rejected
R2 [4]: Option uses one tag byte 0u8 None or 1u8 Some followed by
  the inner encoding when Some
R3 [4]: Length-prefixed types Vec String byte-slice and BTreeMap use
  a u32 LE length or count prefix followed by elements in canonical
  order
R4 [4]: Enums encode as one discriminant byte with the explicit
  repr(u8) value followed by the variant payload — the discriminant
  is not the 0-indexed variant position
R5 [4]: BTreeMap entries are emitted in ascending order of the
  canonical encoded bytes of K and HashMap is excluded from the
  canonical surface
R6 [4]: Decode is strict — one value is read and any remaining bytes
  produce DecodeError::TrailingBytes
R7 [4]: Encode and Decode are sealed via a private supertrait pattern
  preventing downstream impls outside the workspace derive macros
R8 [4]: The decoder enforces a configurable byte cap default 1 MiB —
  any length header exceeding the remaining budget is rejected
  before allocation and buffer growth is bounded doubling capped at
  the cap
R9 [4]: SCHEMA_HASH is a u128 produced by xxh3-128 over a
  deterministic schema-bytes encoding with the GEN-0003 input rules
  carried forward unchanged

## Consequences

- **Positive:** byte representation is sequential and self-contained
  per value — no offset arithmetic, no heap region, no
  random-position writes. The encoder is single-pass.
- **Positive:** the workspace owns the spec; sealing prevents
  out-of-tree implementations from drifting from the canonical bytes.
- **Positive:** decoder cap + bounded doubling makes
  decompression-bomb mitigation a property of the trait surface, not
  a wrapper concern.
- **Positive:** `repr(u8)` literal discriminants make wire output
  legible to humans inspecting hex dumps and stable under enum
  reorders provided discriminants are explicit.
- **Negative:** removes random-access reads of inner fields — a
  consumer must decode prefix bytes to reach a later field. The hot
  path was already sequential; the loss is theoretical for current
  consumers.
- **Negative:** the canonical surface is capped at 256 enum variants
  per type. Larger enums must be restructured (e.g. nested enums).
- **Negative:** xxh3-128 raises `SCHEMA_HASH` width from 64 to 128
  bits, doubling the in-message hash cost. The collision-floor gain
  (birthday bound `~2^64`) is the explicit tradeoff.
- **Negative:** the wire format is frozen — changing primitive
  encoding, length-prefix width, enum discriminant byte semantics,
  BTreeMap ordering, or schema-bytes canonicalisation invalidates
  all persisted data.
