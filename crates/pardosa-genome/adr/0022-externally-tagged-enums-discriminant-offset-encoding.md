# ADR-022: Externally tagged enums with fixed discriminant+offset encoding

**Status:** Accepted

### Context

Enum encoding is one of the most consequential wire format decisions. It
determines inline size (affects struct padding), heap layout, and which serde
enum representations are supported. ADR-004 documents the compile-time
rejection of incompatible serde attributes (`tag`, `untagged`, `content`).
ADR-007 lists the enum layout as `[discriminant:u32][offset:u32]`. Neither
documents the rationale for choosing this specific encoding over alternatives.

Four encoding strategies were evaluated:

1. **Varint discriminant + variable payload** (protobuf-style): Compact for
   small enums but variable inline size breaks the fixed-layout contract.
2. **Tag byte + length-prefixed payload** (CBOR-style): Self-describing but
   adds per-variant overhead and complicates random access.
3. **Union table with vtable** (FlatBuffers-style): Supports schema evolution
   but adds vtable complexity, rejected by ADR-002.
4. **Fixed `[discriminant:u32][offset:u32]`**: Constant 8-byte inline cost.
   Deterministic layout. Simple implementation. Data in heap via offset.

### Decision

All enums use a fixed 8-byte inline encoding:

```
[discriminant:u32 LE][offset:u32 LE]
```

- **Discriminant:** 0-indexed variant position in the Rust enum declaration
  order. Validated on deserialization — unknown discriminants produce
  `DeError::InvalidDiscriminant`.
- **Offset (data variants):** Points to the variant's payload in the heap
  region. Payload layout is the variant's field layout (struct variant: fields
  in declaration order; tuple variant: elements inline; newtype variant:
  inner type).
- **Offset (unit variants):** Must be `0x00000000`. Treated as padding — the
  backward offset check does not apply. Non-zero offset on a unit variant
  produces `DeError::NonZeroPadding`.

Only serde's **externally tagged** representation (the default, no attribute)
is supported. Internally tagged (`#[serde(tag)]`), adjacently tagged
(`#[serde(tag, content)]`), and untagged (`#[serde(untagged)]`) are rejected
at compile time by `#[derive(GenomeSafe)]` (ADR-004).

### Consequences

- **Positive:** Fixed 8-byte inline size per enum. Struct layout is fully
  determined at compile time. No branching on variant during inline offset
  computation.
- **Positive:** u32 discriminant supports ~4 billion variants — effectively
  unlimited.
- **Positive:** Simple read path: read discriminant, check bounds, read
  offset, jump to heap. No vtable lookup.
- **Negative:** Unit variants waste 4 bytes (offset field is padding). For
  enums with many unit variants (e.g., 100-variant error codes), this adds
  400 bytes per message vs. a 1-byte discriminant approach.
- **Negative:** Only externally tagged enums supported. Users of `#[serde(tag)]`
  must restructure their types.
- **Negative:** Adding enum variants at positions other than the end changes
  discriminant values, breaking existing data. Append-only variant addition
  preserves compatibility (new discriminant > existing max).

### References

- ADR-004 (compile-time rejection of tag, untagged, content)
- ADR-007 (offset-based binary layout — lists enum encoding)
- ADR-018 (unit variant offset = padding, non-zero is hard error)
- genome.md §Data Model (enum rows), §Serde Integration (serialize_*_variant)
- `error.rs` — `DeError::InvalidDiscriminant`
