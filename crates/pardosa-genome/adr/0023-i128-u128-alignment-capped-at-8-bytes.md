# ADR-023: i128/u128 alignment capped at 8 bytes

**Status:** Accepted

### Context

Every type in pardosa-genome's wire format has a fixed natural alignment that
determines padding insertion before the type's inline data. For most types,
alignment matches the type's byte width: u8 aligns to 1, u32 aligns to 4,
u64 aligns to 8. The question is what alignment to use for 128-bit types
(i128, u128), which occupy 16 bytes.

Two options:

- **16-byte alignment:** Matches the logical width. Maximizes SIMD friendliness
  on platforms with 128-bit SIMD registers. Worst-case padding: 15 bytes per
  field.
- **8-byte alignment:** Reduces worst-case padding to 7 bytes. Matches Rust's
  actual alignment for i128/u128 on most targets (8 bytes on x86-64, 4 bytes
  on 32-bit). SIMD consumers must manually align.

Rust itself aligns i128/u128 to 8 bytes on x86-64 and 4 bytes on 32-bit
targets. A wire format must choose a single cross-platform alignment.

### Decision

i128 and u128 align to **8 bytes** in the wire format, not 16.

This is consistent with the maximum alignment of any other supported type
(u64/i64/f64 all align to 8). It prevents excessive padding on 32-bit targets
where the next-lower-aligned field would leave up to 15 bytes of zeros.

The encoding is 16 bytes LE, written at an 8-byte-aligned cursor position.

### Consequences

- **Positive:** Consistent maximum alignment (8 bytes) across all types.
  Struct padding is predictable without special-casing 128-bit types.
- **Positive:** Reduced waste on 32-bit targets. A struct with `u8` followed
  by `u128` wastes 7 bytes of padding (not 15).
- **Positive:** Cross-platform consistency — the same alignment on all targets.
- **Negative:** SIMD consumers on platforms with 128-bit registers (AVX, NEON)
  must copy i128/u128 values to aligned storage before SIMD operations.
  Marginal cost given that pardosa-genome uses `from_le_bytes` (not pointer
  casts) for all reads.
- **Negative:** Frozen wire format decision. Cannot change to 16-byte alignment
  without breaking all existing data.

### References

- genome.md §Alignment Rules, item 4: "i128/u128 align to 8 (not 16) to
  avoid excessive padding on 32-bit targets"
- genome.md §Type Inline Sizes: i128/u128 listed as 16 bytes, 8-byte aligned
- `genome_safe.rs` — i128, u128 in `impl_genome_safe_primitive!` macro
