# GEN-0018. Non-Zero Padding Is Hard Error

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: GEN-0011

## Context

Binary formats with alignment padding face a design choice: should non-zero
padding bytes be silently ignored, produce a warning, or cause a hard error?

Silent ignoring creates a covert channel — data can be hidden in padding bytes.
It also masks corruption: a bitflip in a padding byte goes undetected. Warnings
are rarely actionable in automated pipelines. Hard errors are strict but
unambiguous.

## Decision

All padding bytes must be `0x00`. Non-zero padding produces
`DeError::NonZeroPadding { offset }` — a hard, non-recoverable error. There is
no "lenient" mode.

This applies to three categories of zero-requirement bytes:

**1. Alignment padding between struct fields:**

When fields require alignment (e.g., a `u8` followed by a `u64`), the gap bytes
must be `0x00`. Example:

```
struct Mixed { a: u8, b: u64 }
// Layout: a@0(1B), pad@1(7B zeros), b@8(8B)
```

Bytes 1–7 must all be `0x00`.

**2. Reserved bytes in file header and footer:**

File header bytes 25–31 (7 reserved bytes) and file footer bytes 16–19
(4 reserved bytes) must be all zeros. This enforces forward compatibility:
v1 readers reject v2 files that use reserved fields, rather than silently
ignoring unknown data.

**3. Enum unit variant offset field:**

Enum layout is `[discriminant:u32][offset:u32]`. For unit variants, the offset
field is unused — it must be `0x00000000`. Non-zero offset on a unit variant
produces `DeError::NonZeroPadding`. The backward offset check (GEN-0011, check #6)
does not apply to unit variant offsets — they are treated as padding, not as
heap references.

R1 [5]: All padding bytes must be 0x00 — non-zero padding produces
  DeError::NonZeroPadding as a hard non-recoverable error
R2 [5]: There is no lenient mode for padding validation
R3 [6]: Enum unit variant offset fields must be 0x00000000 and are
  treated as padding not as heap references

## Consequences

- **Positive:** Catches corruption in padding regions that would otherwise be
  invisible.
- **Positive:** Eliminates covert channels in padding bytes.
- **Positive:** Enforces forward compatibility — reserved bytes in headers/footers
  cause rejection when non-zero, ensuring v1 readers cleanly reject v2+ features.
- **Positive:** Deterministic output — serializers must zero-fill padding,
  producing identical bytes for identical values.
- **Negative:** Strictness means any single bitflip in padding causes rejection.
  This is intentional: corruption should be detected, not silently tolerated.
- **Negative:** Serializers must explicitly zero-fill padding bytes rather than
  leaving them uninitialized. Minimal performance cost.
