# GEN-0022. Externally Tagged Enums — Discriminant Offset Encoding

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Superseded by GEN-0035

## Retirement

Superseded-by: GEN-0035
Moved-to-stale: 2026-05-15
Reason: GEN-0035:R4 replaces the 8-byte [discriminant:u32][offset:u32]
inline stub with a single discriminant byte carrying the explicit
repr(u8) value followed by the variant payload. The 0-indexed
position-as-discriminant rule and the unit-variant offset-padding
rule are retired with the offset-based layout.
