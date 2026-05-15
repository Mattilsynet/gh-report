# ADR-003: Compile-time xxHash64 schema hashing

**Status:** Accepted (amended: hash stability contract documented April 2025)

### Context

Fixed-layout formats need a mechanism to detect type mismatches before deserialization
attempts produce corrupt data. The hash must be computed at compile time (no runtime
cost), deterministic across compilations, and have a low collision probability for
accidental mismatches in multi-service deployments.

### Decision

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

### Consequences

- **Positive:** Type confusion detected at deserialization time with zero runtime cost
  for the hash itself (computed at compile time as a `const`).
- **Positive:** Root type name in hash distinguishes newtypes (`Meters(f64)` vs
  `Seconds(f64)`) despite identical inner layout.
- **Positive:** Frozen inputs documented and tested — pinned hash value tests catch
  accidental algorithm changes.
- **Negative:** Hash stability contract is load-bearing. Any change to the algorithm,
  seed, or input canonicalization is a breaking change affecting all persisted data.
- **Negative:** `String` ≠ `&str` may surprise users expecting serde's transparent
  string serialization to imply hash equivalence. Documented in genome.md §String
  Type Identity.

### References

- genome.md §Schema Export, §Hash Stability Contract, §String Type Identity
- `genome_safe.rs`: `schema_hash_bytes`, `schema_hash_combine` doc comments
- Tests: `string_str_cow_hash_identity`, `primitive_hashes_are_distinct`
