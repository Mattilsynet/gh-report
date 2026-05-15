# GEN-0021. Breadth-First Heap Ordering

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: GEN-0032

## Context

pardosa-genome uses a two-region layout: inline (scalars, discriminants) then heap (strings, vecs, maps, option/enum payloads). GEN-0005 mentions breadth-first heap ordering, but the ordering is a frozen wire format property determining binary equality. Depth-first interleaves heap items and produces backward offsets. Breadth-first guarantees forward-pointing offsets by construction. Unspecified ordering breaks binary equality and content-addressable storage.

## Decision

Heap items are written in **breadth-first order**: all items directly referenced
by the inline region are written first, then items referenced by those heap
items, then items referenced by those items, and so on. Each level processes
nodes that are strictly closer to leaves than the previous level.

This ordering is a **frozen wire format property**. Changing it invalidates all
existing genome files and bare messages. Golden test vectors pin the exact byte
output for multi-level heap structures.

**Termination guarantee:** The type tree is finite by construction. Leaf nodes
(scalars, strings, bytes) produce no further heap items. Each breadth-first
level is strictly smaller than the previous, reaching an empty level in bounded
iterations.

R1 [5]: Heap items are written in breadth-first order — all items
  directly referenced by the inline region first, then items referenced
  by those heap items, and so on
R2 [5]: This ordering is a frozen wire format property — changing it
  invalidates all existing genome files and bare messages
R3 [6]: Golden test vectors pin the exact byte output for multi-level
  heap structures

## Consequences

- **Positive:** All offsets forward-pointing by construction; backward offset check (GEN-0011, check #6) trivially satisfied.
- **Positive:** Binary equality — identical values always produce identical bytes, enabling content-addressable storage and deduplication.
- **Positive:** Deterministic output enables `verify_roundtrip` byte comparison.
- **Negative:** `WritingSerializer` must track heap items by level, more complex than depth-first.
- **Negative:** Frozen forever — any second implementation must replicate this ordering exactly.
