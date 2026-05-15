# ADR-024: NaN bit-pattern preservation — no canonicalization

**Status:** Accepted

### Context

IEEE 754 floating-point defines NaN (Not a Number) as any value with all
exponent bits set and a non-zero significand. For f64 this allows ~2^52
distinct NaN bit patterns; for f32, ~2^23. NaN payloads are used by some
runtimes for NaN-boxing (encoding type tags in the significand bits) and
diagnostic tagging.

Different CPU architectures may produce different NaN payloads for the same
arithmetic operation. For example, ARM64 `fmul` of `NaN * 1.0` may modify
payload bits differently than x86-64. This means identical computations on
different platforms can produce different NaN bit patterns.

ADR-012 establishes little-endian encoding via `from_le_bytes` and states in
one line that "exact NaN bit patterns [are] preserved (no canonicalization)."
This ADR documents the rationale, tradeoffs, and cross-platform implications
as a standalone decision.

Two approaches were evaluated:

- **Canonicalization** (Cap'n Proto approach): Replace all NaN values with a
  single canonical NaN (e.g., `0x7FF8000000000000` for f64). Guarantees
  cross-platform byte equality for NaN-producing arithmetic. Destroys NaN
  payload information.
- **Bit-pattern preservation** (pardosa-genome approach): Serialize exact
  `to_le_bytes()` output. Round-trips `f64::NAN.to_bits()` exactly. Does not
  guarantee cross-platform byte equality for NaN-producing arithmetic.

### Decision

pardosa-genome preserves exact NaN bit patterns. No canonicalization.
`f64::NAN.to_bits()` round-trips exactly through `to_le_bytes()` /
`from_le_bytes()`.

Floats are serialized as raw LE IEEE 754 bytes. The serializer performs no
inspection or modification of the bit pattern. The deserializer reads the
raw bytes and converts via `from_le_bytes` — no NaN detection, no payload
masking, no quiet/signaling conversion.

### Consequences

- **Positive:** Bit-fidelity preserved. Applications using NaN-boxing or NaN
  payload tagging can serialize and deserialize without information loss.
- **Positive:** Simple implementation — no float inspection code. Zero overhead
  on the serialization hot path.
- **Positive:** Value-level determinism guaranteed: the same `to_bits()` value
  always produces the same bytes, and vice versa.
- **Negative:** Cross-platform determinism is NOT guaranteed for
  NaN-producing arithmetic. The same floating-point computation on x86-64 vs
  ARM64 may produce different NaN payloads, which serialize to different bytes.
  This affects content-addressable storage of floating-point computation results
  across heterogeneous clusters.
- **Negative:** `-0.0` and `+0.0` serialize to different bytes (`0x8000...` vs
  `0x0000...`) despite comparing equal via `==`. Applications needing
  `==`-equivalent byte representations must canonicalize at the application
  level.
- **Mitigation:** For cross-platform byte equality, canonicalize NaN values at
  the application level before serialization. pardosa-genome does not impose
  this cost on all users.

### References

- ADR-012 (little-endian wire encoding — states NaN preservation in one line)
- genome.md §NaN handling: "Exact bit patterns are preserved. No
  canonicalization."
- genome.md §Data Model: f32/f64 rows note "exact NaN bits preserved"
- genome.md §Test Plan: `float_special` golden vector covers NaN, -0.0,
  infinity
