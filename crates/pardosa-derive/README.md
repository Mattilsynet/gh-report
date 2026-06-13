# pardosa-derive

Substrate-agnostic event-invariant carrier: sealed traits and derive macros
for pardosa events. Currently exposes one derive: `#[derive(GenomeSafe)]`.

Part of the [pardosa](https://github.com/acje/solon) workspace.

## Overview

`pardosa-derive` is the proc-macro ring of the pardosa workspace. It walks
the structural shape of a user type (`struct` or `#[repr(u8)] enum`),
rejects any shape that would silently break a downstream invariant, and
emits three coordinated impls in one pass:

- `pardosa_wire::sealed::Sealed` — the sealed-trait gate from PGN-0003
  "Canonical Encoding, Schema Hash, and EventSafe Bounds"; ensures only
  types that travel through this derive
  (or in-crate hand impls) can participate in pardosa events.
- `pardosa_schema::EventSafe` — the payload-typed event-participation
  marker decoupled from codec capability per PGN-0003 "Canonical Encoding,
  Schema Hash, and EventSafe Bounds".
- `pardosa_schema::GenomeSafe` — the schema-hash-bearing trait whose
  `SCHEMA_HASH: u128` constant is derived from the structural shape so
  the `.pgno` schema-hash gate (PGN-0003 "Canonical Encoding, Schema Hash,
  and EventSafe Bounds" + PGN-0004 "pgno File Format and Durability
  Substrate") stays sound.

Plus canonical `Encode` / `Decode` impls so the derived type round-trips
through `pardosa_wire::to_vec` / `from_bytes`.

The macro is deliberately strict at the input stage: serde attributes are
rejected at the type, variant, and field level — once a type derives
`GenomeSafe` its canonical encoding is fixed by structural shape and no
serde rename / skip / flatten attribute may silently alter it. Unions are
rejected (`EVT-001`). Enums must be `#[repr(u8)]` with explicit
discriminants so adding a variant later does not silently renumber.

## Quick start

```rust,no_run
use pardosa_schema::{NonEmptyEventString, GenomeSafe, Timestamp};

#[derive(Debug, GenomeSafe)]
#[repr(u8)]
enum DomainEvent {
    SweepStarted {
        org: NonEmptyEventString<128>,
        repo_count: u64,
        timestamp: Timestamp,
    } = 0,
    SweepCompleted {
        batch_id: NonEmptyEventString<64>,
        timestamp: Timestamp,
    } = 1,
}
// The derive emits sealed + EventSafe + GenomeSafe + Encode + Decode;
// SCHEMA_HASH is a const u128 derived from the structural shape above.
```

## Documentation

API docs: <https://docs.rs/pardosa-derive>

## Architecture decisions

- PGN-0003 "Canonical Encoding, Schema Hash, and EventSafe Bounds" —
  which traits are closed
  (`EventSafe`, `GenomeSafe`, `GenomeOrd`) and which stay open
  (`Encode`, `Decode`, `Validate`); the derive enforces the closure.
  `SCHEMA_HASH: u128` is derived from
  structural shape; the canonical encoder is deterministic across runs.
  The derive emits
  participation and codec impls separately so the trait graph stays
  orthogonal.

The full ADR set lives under [`docs/adr/`](../../docs/adr/).

## License

Licensed under either of

- Apache License, Version 2.0
- MIT License

at your option.
