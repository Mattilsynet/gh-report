# GEN-0003. Compile-Time xxHash64 Schema Hashing

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: GEN-0001
Superseded-in-part-by: GEN-0035

## Context

> **Partial supersession (2026-05-15):** GEN-0035 replaces the
> algorithm and width — xxh64 → xxh3-128, `SCHEMA_HASH: u64` →
> `SCHEMA_HASH: u128` — for new schemas. The frozen-input rules of
> this ADR (seed, struct/enum/variant prefixes, primitive name
> canonicalisation, PhantomData ignoring T, and the
> str/String/bytes/Vec<u8> equivalence classes) carry forward
> unchanged into GEN-0035's schema-bytes encoding.

Fixed-layout formats need a mechanism to detect type mismatches before deserialization
attempts produce corrupt data. The hash must be computed at compile time (no runtime
cost), deterministic across compilations, and have a low collision probability for
accidental mismatches in multi-service deployments.

## Decision

Compute an 8-byte xxHash64 fingerprint at compile time from the type's serde structure:
root type name, field names, field types, enum variant names and shapes. The hash is
embedded in every serialized message and verified on deserialization. The 8-byte width
pushes the birthday bound to ~4 billion types — practically collision-free for accidental
mismatches.

The hash algorithm inputs are **frozen** — any change invalidates all existing data:

| Input | Frozen value |
|-------|-------------|
| Algorithm | xxHash64 (`xxhash_rust::const_xxh64::xxh64`) |
| Seed | `0` (all calls) |
| Combine method | LE-concatenate two u64 → 16 bytes → xxh64(bytes, 0) |
| Struct prefix | `"struct:Name"` |
| Enum prefix | `"enum:Name"` |
| Variant prefix | `"variant:Name"` |
| Primitive names | `stringify!($ty)` |
| Array length | `N as u64` |
| PhantomData | Always `"PhantomData"`, ignores `T` |

**String and bytes type equivalence classes:**

| Equivalence class | Types | Hash input |
|-------------------|-------|-----------|
| str-identity | `str`, `&str`, `Cow<'_, str>`, `Box<str>`, `Arc<str>` | `"str"` |
| String-identity | `String`, `Box<String>`, `Arc<String>` | `"String"` |
| bytes-identity | `&[u8]` | `"bytes"` |
| Vec\<u8\>-identity | `Vec<u8>` | `combine("Vec", u8::SCHEMA_HASH)` |

Schema-compatible substitutions within a class (e.g., `&str` → `Cow<str>`) preserve
the hash. Cross-class substitutions (`String` → `&str`) break the hash intentionally —
they change zero-copy deserialization semantics. The bytes/Vec\<u8\> split parallels
the str/String split: `&[u8]` supports zero-copy borrowing, `Vec<u8>` does not.

R1 [5]: Compute an 8-byte xxHash64 fingerprint at compile time from the
  type's serde structure including root type name, field names, field
  types, enum variant names, and shapes
R2 [5]: The hash algorithm inputs are frozen — any change invalidates
  all existing data
R3 [5]: String and bytes types use equivalence classes for schema hash
  identity — substitutions within a class preserve the hash, cross-class
  substitutions break it intentionally

## Consequences

- **Positive:** Type confusion detected at deserialization time with zero runtime cost (compile-time `const`).
- **Positive:** Root type name distinguishes newtypes (`Meters(f64)` vs `Seconds(f64)`) despite identical layout.
- **Positive:** Pinned hash value tests catch accidental algorithm changes.
- **Negative:** Hash stability contract is load-bearing — any change to algorithm, seed, or input canonicalization breaks all persisted data.
- **Negative:** `String` ≠ `&str` may surprise users; the distinction is intentional under string-type identity rules.
