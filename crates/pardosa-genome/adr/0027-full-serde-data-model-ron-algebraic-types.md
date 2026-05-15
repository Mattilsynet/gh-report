# ADR-027: Full serde data model support — RON algebraic types

**Status:** Accepted

### Context

Serde's data model defines 29 types that any `Serializer`/`Deserializer` must
handle. Binary serialization formats typically support a subset:

| Format | Tuples | Non-string map keys | Char | Unit structs | All enum forms |
|--------|:------:|:-------------------:|:----:|:------------:|:--------------:|
| FlatBuffers | ✗ | ✗ | ✗ | ✗ | Partial (unions) |
| Protobuf | ✗ | ✗ (string/int only) | ✗ | ✗ | ✗ (oneof) |
| bincode | ✓ | ✓ | ✓ | ✓ | ✓ |
| postcard | ✓ | ✓ | ✓ | ✓ | ✓ |
| **pardosa-genome** | **✓** | **✓** | **✓** | **✓** | **✓** |

Supporting a subset would reduce implementation complexity but limit which
existing Rust types can be serialized with `#[derive(Serialize, Deserialize,
GenomeSafe)]`.

### Decision

pardosa-genome supports the **full serde data model**, matching RON's algebraic
type system:

- **Tuples** (1–16 elements, matching serde's limit): inline with alignment.
- **Non-string map keys**: any serializable type can be a map key. Keys
  serialized in container iteration order (BTreeMap = sorted).
- **Char**: 4 bytes LE u32, validated as Unicode scalar on deserialization.
- **Unit structs**: 0 bytes inline.
- **All four enum variant forms**: unit, newtype, tuple, struct.
- **Newtype structs**: transparent (inner type's layout).
- **Nested containers**: `Vec<Vec<String>>`, `Option<Vec<Option<u32>>>`, etc.
- **Fixed-size arrays**: `[T; N]` with array length in schema hash.

The `GenomeSafe` trait provides blanket implementations for all standard
library types in this set: primitives, `String`, `str`, `&str`, `&[u8]`,
`Vec<T>`, `Option<T>`, `Box<T>`, `Arc<T>`, `Cow<T>`, `BTreeMap<K,V>`,
`BTreeSet<T>`, `PhantomData<T>`, `[T; N]`, tuples (1–16), and `()`.

### Consequences

- **Positive:** Maximum compatibility with existing Rust serde types. Any
  `#[derive(Serialize, Deserialize)]` struct can add `GenomeSafe` (modulo
  the rejected types from ADR-004).
- **Positive:** No artificial limitations that force users to restructure
  their data models. Non-string map keys (e.g., `BTreeMap<(u32, u32), Vec<u8>>`)
  work naturally.
- **Positive:** RON can serve as a human-readable debug format for the same
  types — same serde derives, different serializer.
- **Negative:** All 29 serde `Serializer`/`Deserializer` trait methods must
  be implemented in both `SizingSerializer` and `WritingSerializer`. Significant
  implementation surface (~1800 LOC estimated for Phase 2).
- **Negative:** Non-string map keys require arbitrary key serialization with
  alignment, increasing heap layout complexity.
- **Negative:** Char validation (Unicode scalar check) adds a per-char
  branch on deserialization. Negligible cost but nonzero.

### References

- genome.md §Design Principles, item 4: "All of Rust's algebraic types"
- genome.md §Data Model (full type table)
- `genome_safe.rs` — blanket impls for all supported types
- `error.rs` — `DeError::InvalidChar` for char validation
