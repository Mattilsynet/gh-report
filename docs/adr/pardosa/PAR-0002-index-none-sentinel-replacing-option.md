# PAR-0002. Index::NONE Sentinel Replacing Option\<Index\>

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: PAR-0001

## Context

The original design used `Option<Index>` for event precursors — `None` for the
first event in a fiber, `Some(Index)` for subsequent events. In genome's
fixed-layout binary format, `Option<T>` requires a 4-byte offset into the heap
region plus the heap entry itself. Every event pays this cost for the precursor
field, even though the overwhelming majority of events have a valid precursor.

## Decision

Replace `Option<Index>` with a sentinel value:

```rust
impl Index {
    pub const NONE: Index = Index(u64::MAX);
}
```

`u64::MAX` is permanently reserved. A line with `u64::MAX` events would require
~147 exabytes of storage, so this value can never be a valid line position.

Guards:
- `Index::new(v)` panics if `v == u64::MAX` — prevents accidental sentinel
  construction from application code.
- `Index::checked_next()` caps at `u64::MAX - 1` — no valid index arithmetic
  can produce the sentinel.
- `is_none()` / `is_some()` methods for ergonomic checking.

R1 [9]: Reserve u64::MAX as Index::NONE sentinel replacing
  Option<Index> for the precursor field
R2 [9]: Index::new(v) panics if v equals u64::MAX to prevent
  accidental sentinel construction from application code
R3 [9]: Index::checked_next() caps at u64::MAX minus 1 so valid
  index arithmetic never produces the sentinel value

## Consequences

Saves 4 bytes inline plus heap indirection per event in genome encoding (~4 MiB for 1M events). `Index` is `Copy` with no heap allocation. Sentinel semantics are explicit via `is_none()` and `NONE` constant. Trade-off: less type-safe than `Option` — callers could forget `is_none()` checks. `u64::MAX` is permanently consumed from the value space (no runtime impact).
