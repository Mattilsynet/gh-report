# pardosa

EDA storage layer implementing fiber semantics. `pardosa` is the runtime ring
of the workspace — it composes the substrate crates (`pardosa-wire`,
`pardosa-schema`, `pardosa-file`) into a single adopter-facing appliance,
[`pardosa::store::EventStore`], that opens a typed `.pgno`-backed journal,
appends typed events, walks per-fiber history, tails the global event line
with consumer ACK/resume, and follows same-fiber causal precursors.

Part of the [pardosa](https://github.com/acje/solon) workspace.

## Overview

A `Fiber` is the unit of identity in pardosa: a strand of related events
through which causality flows (PGN-0002 "Fibers, Events, Timestamps, and
Inspectable State"). Each `Event<T>`
carries both a `FiberId` (which fiber it touched) and an `EventId` (its
position in the global event line). Both are monotonic within their own
scope, and `Dragline<T>` owns the dragline-local `next_fiber_id` allocator
at commit time.

The public surface is a *sole-interface seal* (PGN-0008 "EventStore Facade
and Operation-Specific Bounds"): adopters reach the journal exclusively
through `pardosa::store::EventStore` and the items re-exported from
`pardosa::prelude`. Internal substrate types — the in-tree `inner::EventStore<T, W>`,
the `Syncable` trait, the `DraglineView<'_, T>`, the publisher
surface, the durability sidecar lifecycle — are never named in adopter
code. Drift is pinned by `tests/ui_pass/prelude_usable.rs`.

`StoreReader` is `!Send` and `StoreWriter` is `Send` (see PGN-0007
"Cursor, Publish, Ack, and Capability Boundaries"); the
`!Send` reader can hold per-thread sidecar state without paying the cost
of cross-thread synchronisation, while the writer can be moved into a
publishing task.

## Quick start

```rust,no_run
use pardosa::store::{EventStore, GenomeSafe, HasEventSchemaSource, Validate};
use pardosa_schema::Timestamp;

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Tick { when: Timestamp, seq: u64 }

impl HasEventSchemaSource for Tick {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = Some("examples/tick");
}
impl Validate for Tick {
    type Error = core::convert::Infallible;
    fn validate(&self) -> Result<(), Self::Error> { Ok(()) }
}

let path = std::env::temp_dir().join("ticks.pgno");
let mut store: EventStore<Tick> = EventStore::create(&path)?;
let fiber = store.writer().begin(Tick {
    when: Timestamp::from_nanos(1).unwrap(),
    seq: 100,
})?.fiber();
let _lsn = store.writer().sync()?;
# Ok::<_, Box<dyn std::error::Error>>(())
```

See `crates/pardosa/examples/basic_lifecycle.rs` for the full
create → begin → sync → reopen → read cycle.

## Documentation

API docs: <https://docs.rs/pardosa>

## Architecture decisions

- PGN-0002 "Fibers, Events, Timestamps, and Inspectable State" —
  dragline-local `FiberId`, `FiberState` as
  `#[non_exhaustive]`, detached / precursor invariants.
- PGN-0008 "EventStore Facade and Operation-Specific Bounds" — the
  sole-interface seal this crate enforces.
- PGN-0004 "pgno File Format and Durability Substrate" — `Lsn` /
  `AckPosition` semantics; PGN-0007 covers the durable-publish recovery
  story.
- PGN-0003 "Canonical Encoding, Schema Hash, and EventSafe Bounds" —
  which traits adopters may implement
  (`Validate`, `Encode`, `Decode`) and which are closed
  (`EventSafe`, `GenomeSafe`, `GenomeOrd`).

The full ADR set lives under [`docs/adr/`](../../docs/adr/).

## License

Licensed under either of

- Apache License, Version 2.0
- MIT License

at your option.
