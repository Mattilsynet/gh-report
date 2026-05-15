# GEN-0023. i128/u128 Alignment Capped at 8 Bytes

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Superseded by GEN-0035

## Retirement

Superseded-by: GEN-0035
Moved-to-stale: 2026-05-15
Reason: GEN-0035 has no alignment region — primitives are written
sequentially with no padding. The 8-byte cap on i128/u128 alignment
no longer applies; i128/u128 are written as 16 LE bytes back-to-back
with surrounding fields per GEN-0035:R1.
