# GEN-0021. Breadth-First Heap Ordering

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Superseded by GEN-0035

## Retirement

Superseded-by: GEN-0035
Moved-to-stale: 2026-05-20
Reason: There is no heap region in the GEN-0035 sequential canonical
encoding. Length-prefixed values are encoded in declaration order with
no offsets to forward-point at; the breadth-first ordering rule has no
representation in the new format.
