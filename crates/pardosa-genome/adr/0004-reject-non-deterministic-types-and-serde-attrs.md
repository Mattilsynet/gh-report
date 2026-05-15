# ADR-004: Compile-time rejection of non-deterministic types and serde attributes

**Status:** Accepted (amended: structured parsing + extended type recursion, April 2025; `serde(with)` escape hatch documented, April 2026)

### Context

A fixed-layout binary format requires deterministic serialization: the same logical
value must always produce the same bytes. `HashMap`/`HashSet` have non-deterministic
iteration order. Platform-sized types (`usize`, `isize`) have non-deterministic width.
Several serde attributes (`flatten`, `tag`, `untagged`, `skip_serializing_if`, `content`)
alter serde's serialization path in ways incompatible with fixed-layout encoding.

### Decision

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

**Implementation details (amended April 2025):**
- Serde attribute rejection uses `syn::parse_nested_meta` for structured parsing.
  Previously used `tokens_str.contains("flatten")` string matching, which false-positived
  on `rename = "flatten_count"`. Fixed.
- Type rejection recurses into all `syn::Type` variants: `Path`, `Reference`, `Slice`,
  `Array`, `Tuple`, `Paren`. Previously only recursed into `Path`, missing `&HashMap`,
  `[usize; 4]`, `(usize, u32)` etc. Fixed.
- Variant-level attribute validation runs in `derive_genome_safe_impl` (main validation),
  not inside `build_schema_source` (a side effect of source generation). Fixed.

### Consequences

- **Positive:** Compile-time errors with clear messages citing the specific field and
  rejected type/attribute. No runtime surprises.
- **Positive:** Structured attribute parsing eliminates false positives on field names
  or rename values containing rejected keywords.
- **Positive:** Extended type recursion catches `&HashMap<K,V>`, `[usize; N]`,
  `(usize, u32)` — previously only caught as worse trait-bound errors.
- **Negative:** Users must use `BTreeMap`/`BTreeSet` instead of `HashMap`/`HashSet`.
- **Negative:** Users must use fixed-width integers (`u32`, `u64`) instead of `usize`.
- **Residual risk:** Manual `Serialize` impls can bypass all compile-time checks.
  Runtime `UnsupportedAttribute` detection in the serializer and `verify_roundtrip`
  in CI are defense-in-depth.

**`#[serde(with = "...")]` escape hatch (added April 2026):**

`#[serde(with = "module")]` replaces a field's serialize/deserialize implementation
with a custom module. This bypasses the genome serializer for that field — the custom
module may produce non-deterministic output, skip fields, or alter the wire layout.

`serde(with)` is **not rejected** by the derive macro because:
1. It cannot be reliably distinguished from benign uses (e.g., custom date formatting
   that is still deterministic) without analyzing the referenced module's source.
2. Rejecting it would force users to implement entire `Serialize`/`Deserialize` traits
   manually, which is worse (less visible, harder to review).
3. `verify_roundtrip` catches any canonical encoding violation at runtime.

`serde(with)` is distinct from the other rejected attributes: `flatten`, `tag`,
`untagged`, and `skip_serializing_if` are structurally incompatible with fixed-layout
encoding regardless of implementation. `serde(with)` is implementation-dependent —
it _may_ be compatible if the custom module respects the genome wire format.

**Recommendation:** Treat `#[serde(with)]` as an auditable escape hatch. Code review
should verify that the referenced module produces deterministic, layout-compatible
output. `verify_roundtrip` in CI provides automated verification.

### References

- genome.md §Serialization Algorithm (compile-time detection table)
- `pardosa-derive/src/lib.rs`: `reject_serde_attrs`, `reject_hashmap_type`,
  `reject_platform_sized_type`
- Tests: `compile_fail/serde_*.rs`, `compile_fail/ref_hashmap.rs`,
  `compile_fail/array_usize.rs`, `compile_fail/tuple_usize.rs`,
  `compile_pass/field_named_flatten.rs`, `compile_pass/field_renamed_to_tag.rs`
