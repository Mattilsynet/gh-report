# GEN-0020. Empty Containers Always Allocate Heap Entries

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Superseded by GEN-0035

## Retirement

Superseded-by: GEN-0035
Moved-to-stale: 2026-05-15
Reason: There is no heap region in the GEN-0035 sequential canonical
encoding. Empty containers emit a length prefix of 0 inline; the
"offset 0 / sentinel ambiguity" the ADR resolved no longer arises.
