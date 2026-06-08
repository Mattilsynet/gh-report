# GEN-0024. NaN Bit-Pattern Preservation — No Canonicalization

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: GEN-0035

## Context

IEEE 754 NaN allows ~2^52 distinct bit patterns for f64. Different architectures produce different NaN payloads for the same arithmetic. Two approaches: canonicalization (Cap'n Proto — destroys payloads) or bit-pattern preservation (round-trips exact `to_bits()`, no cross-platform byte equality for NaN-producing arithmetic).

## Decision

pardosa-genome preserves exact NaN bit patterns. No canonicalization.
`f64::NAN.to_bits()` round-trips exactly through `to_le_bytes()` /
`from_le_bytes()`.

Floats are serialized as raw LE IEEE 754 bytes. The serializer performs no
inspection or modification of the bit pattern. The deserializer reads the
raw bytes and converts via `from_le_bytes` — no NaN detection, no payload
masking, no quiet/signaling conversion.

R1 [9]: Preserve exact NaN bit patterns with no canonicalization —
  f64::NAN.to_bits() round-trips exactly
R2 [9]: The serializer performs no inspection or modification of float
  bit patterns
R3 [9]: The deserializer reads raw bytes via from_le_bytes with no NaN
  detection or payload masking

## Consequences

- Bit-fidelity preserved — NaN-boxing and payload tagging round-trip without loss. Zero overhead.
- Value-level determinism: same `to_bits()` always produces same bytes.
- Cross-platform determinism NOT guaranteed for NaN-producing arithmetic.
- `-0.0` and `+0.0` serialize differently despite `==` equality. Canonicalize at application level if needed.
