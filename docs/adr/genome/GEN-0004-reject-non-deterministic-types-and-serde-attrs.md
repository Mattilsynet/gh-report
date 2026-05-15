# GEN-0004. Reject Non-Deterministic Types and Serde Attributes

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: GEN-0002

## Context

A fixed-layout binary format requires deterministic serialization: the same logical
value must always produce the same bytes. `HashMap`/`HashSet` have non-deterministic
iteration order. Platform-sized types (`usize`, `isize`) have non-deterministic width.
Several serde attributes (`flatten`, `tag`, `untagged`, `skip_serializing_if`, `content`)
alter serde's serialization path in ways incompatible with fixed-layout encoding.

## Decision

The `#[derive(GenomeSafe)]` proc-macro rejects these at compile time:

**Types rejected (recursive, through all type positions):**
- `HashMap`, `HashSet` — non-deterministic iteration order
- `usize`, `isize` — platform-dependent width

**Serde attributes rejected:**
- `#[serde(flatten)]` — changes `serialize_struct` → `serialize_map`
- `#[serde(tag = "...")]` — internally tagged enums, incompatible with discriminant layout
- `#[serde(tag = "...", content = "...")]` — adjacently tagged enums
- `#[serde(untagged)]` — removes variant discriminant entirely
- `#[serde(skip_serializing_if = "...")]` — data-dependent field omission

**Implementation details (amended 2025-04-01):**
- Serde attribute rejection uses `syn::parse_nested_meta` for structured parsing.
  Previously used `tokens_str.contains("flatten")` string matching, which false-positived
  on `rename = "flatten_count"`. Fixed.
- Type rejection recurses into all `syn::Type` variants: `Path`, `Reference`, `Slice`,
  `Array`, `Tuple`, `Paren`. Previously only recursed into `Path`, missing `&HashMap`,
  `[usize; 4]`, `(usize, u32)` etc. Fixed.
- Variant-level attribute validation runs in `derive_genome_safe_impl` (main validation),
  not inside `build_schema_source` (a side effect of source generation). Fixed.

R1 [5]: Reject HashMap, HashSet, usize, and isize at compile time
  recursively through all type positions
R2 [5]: Reject serde attributes flatten, tag, content, untagged, and
  skip_serializing_if at compile time
R3 [6]: Type rejection recurses into all syn::Type variants including
  Path, Reference, Slice, Array, Tuple, and Paren

## Consequences

Compile-time errors cite the specific field and rejected type/attribute. Structured attribute parsing eliminates false positives on field names containing rejected keywords. Extended type recursion catches `&HashMap<K,V>`, `[usize; N]`, `(usize, u32)`. Users must use `BTreeMap`/`BTreeSet` and fixed-width integers instead. Manual `Serialize` impls can bypass checks; runtime `verify_roundtrip` in CI is defense-in-depth.

`#[serde(with = "...")]` is **not rejected** because it cannot be distinguished from benign uses without analyzing the referenced module. Treat it as an auditable escape hatch verified by `verify_roundtrip`.
