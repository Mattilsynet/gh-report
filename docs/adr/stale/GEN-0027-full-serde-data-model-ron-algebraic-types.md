# GEN-0027. Full Serde Data Model — RON Algebraic Types

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: GEN-0001

## Context

Serde's data model defines 29 types that any `Serializer`/`Deserializer` must handle. Binary formats typically support subsets — FlatBuffers and Protobuf lack tuples, non-string map keys, char, and unit structs. bincode and postcard support the full model. Supporting a subset would limit which existing Rust types can use `#[derive(Serialize, Deserialize, GenomeSafe)]`.

## Decision

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

R1 [5]: Support the full serde data model including tuples, non-string
  map keys, char, unit structs, and all four enum variant forms
R2 [5]: GenomeSafe provides blanket implementations for all standard
  library types in the supported set
R3 [6]: Fixed-size arrays use array length in the schema hash

## Consequences

- **Positive:** Maximum compatibility with existing Rust serde types — any `#[derive(Serialize, Deserialize)]` struct can add `GenomeSafe` (modulo GEN-0004 rejections). Non-string map keys work naturally.
- **Positive:** RON can serve as a human-readable debug format using the same derives.
- **Negative:** All 29 serde trait methods must be implemented in both `SizingSerializer` and `WritingSerializer` (~1800 LOC).
- **Negative:** Non-string map keys require arbitrary key serialization with alignment, increasing heap layout complexity.
