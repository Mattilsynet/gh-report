# GEN-0012. Little-Endian Wire Encoding — No Pointer Casts

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Superseded by GEN-0035

## Retirement

Superseded-by: GEN-0035
Moved-to-stale: 2026-05-15
Reason: GEN-0035 carries forward LE encoding and the no-pointer-casts
posture (forbid-unsafe in the read path) as part of the in-house
canonical encoding spec. This ADR's framing referenced the
offset-based layout that GEN-0035 retires; the LE rules now live in
GEN-0035:R1.
