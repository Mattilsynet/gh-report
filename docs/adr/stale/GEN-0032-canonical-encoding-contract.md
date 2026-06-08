# GEN-0032. Canonical Encoding Contract

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: S
Status: Accepted

## Related

References: GEN-0001, GND-0005

## Context

pardosa-genome requires **canonical encoding**: the same logical value must always
produce the same bytes. This property is critical for content-addressable storage,
deduplication, and schema-hash-based integrity checking.

The invariants enforcing canonical encoding are currently scattered across multiple
ADRs and spec sections:
- GEN-0004: type and attribute rejection
- GEN-0018: non-zero padding as hard error
- GEN-0021: breadth-first heap ordering
- GEN-0024: NaN bit-pattern preservation
- GEN-0033: GenomeOrd marker trait for map keys

This ADR consolidates the canonical encoding contract into a single reference.

## Decision

The following invariants collectively guarantee canonical encoding:

1. **Deterministic map key ordering.** `BTreeMap` iterates in `Ord` order
   (guaranteed by `std::collections::BTreeMap` documentation). Map entries are
   serialized in iteration order. The `GenomeOrd` marker trait (GEN-0033) restricts
   map keys to types with deterministic, total, platform-independent `Ord`
   implementations — enforced at compile time.

2. **Fixed field ordering.** Struct fields serialize in declaration order. Enum
   variants use fixed discriminant indices. No field reordering, no optional fields,
   no default values (GEN-0029).

3. **Deterministic heap layout.** Variable-length data is written to the heap in
   breadth-first order (GEN-0021). All offsets are forward-pointing. Empty containers
   allocate a heap entry with count=0 (GEN-0020).

4. **Non-deterministic types rejected.** `HashMap`, `HashSet`, `usize`, `isize` are
   rejected at compile time (GEN-0004). Serde attributes that alter serialization
   paths (`flatten`, `tag`, `untagged`, `skip_serializing_if`) are also rejected.

5. **Padding canonicalization.** All alignment padding bytes are `0x00`. Non-zero
   padding is a hard deserialization error (GEN-0018).

6. **Float bit-pattern preservation.** NaN bit patterns are preserved exactly as
   written (GEN-0024). No NaN canonicalization — the same NaN bit pattern in
   produces the same NaN bit pattern out.

7. **Runtime verification.** `verify_roundtrip` provides defense-in-depth: serialize,
   deserialize, re-serialize, and compare bytes. This catches any violation of the
   canonical encoding contract that escapes compile-time checks (e.g., manual
   `Serialize` impls, `#[serde(with)]` modules, incorrect `GenomeOrd` impls).

R1 [2]: The same logical value must always produce the same bytes —
  canonical encoding is required for content-addressable storage
R2 [2]: Seven invariants collectively guarantee canonical encoding
  including deterministic map ordering, fixed field ordering,
  breadth-first heap layout, and padding canonicalization
R3 [2]: verify_roundtrip provides defense-in-depth by serialize,
  deserialize, re-serialize, and byte comparison

## Consequences

- **Positive:** Single reference document for auditors and contributors to verify
  canonical encoding properties.
- **Positive:** Makes the trust boundary explicit: compile-time checks enforce most
  invariants; `verify_roundtrip` catches the rest.
- **Negative:** The contract depends on Rust's `BTreeMap` ordering guarantee, which
  is documented but not formally specified. If `BTreeMap` iteration order changed in
  a future Rust edition, canonical encoding would break.
- **Residual risk:** Types implementing `GenomeOrd` with non-deterministic `Ord`
  (e.g., ordering that depends on thread-local state) break canonicality silently.
  `GenomeOrd` is a safe trait — the compiler cannot prevent this. `verify_roundtrip`
  is the mitigation.
