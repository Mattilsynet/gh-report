# GEN-0007. FlatBuffers-Style Offset-Based Binary Layout

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: S
Status: Superseded by GEN-0035

## Retirement

Superseded-by: GEN-0035
Moved-to-stale: 2026-05-15
Reason: GEN-0035 replaces the offset-based inline+heap layout with an
in-house sequential canonical encoding. The two-region layout, u32
offsets, and breadth-first heap ordering have no representation in
the new format and are wholesale retired.
