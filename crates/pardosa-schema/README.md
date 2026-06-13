# pardosa-schema

Typed-payload vocabulary for pardosa events: `GenomeSafe` marker, bounded
string / byte / vec wrappers, total-order float wrappers, and `CharScalar`.

Part of the [pardosa](https://github.com/acje/solon) workspace.

## Overview

`pardosa-schema` sits one ring above `pardosa-wire`: it builds payload-typed
vocabulary on top of the substrate's payload-opaque codec traits. Every
type here is `Send + Sync` (asserted by `tests/auto_trait_policy.rs`), every
codec round-trip is bounded, and every public type is sealed under the
PGN-0003 "Canonical Encoding, Schema Hash, and EventSafe Bounds" closure
table so the schema-hash gate stays
sound.

`EventString<MAX>`, `NonEmptyEventString<MAX>`, `EventBytes<MAX>`, and
`EventVec<T, MAX>` are bounded wrappers — the `MAX` const is part of the
schema identity (changing `MAX` is a schema migration; PGN-0003 "Canonical
Encoding, Schema Hash, and EventSafe Bounds" + PGN-0009 "Migration Policy
and Clean-Break Posture").
`EventF32` / `EventF64` carry a payable-NaN policy at construction;
`OrderedF32` / `OrderedF64` carry a total order suitable for use in
`GenomeOrd` keys; `RealF32` / `RealF64` reject NaN at construction.

`CharScalar` is the canonical bounded-Unicode-scalar wrapper — a single
USV with `Encode` / `Decode` impls that round-trip through `u32` without
reading past the scalar value.

The `derive` feature re-exports `pardosa_derive::GenomeSafe`, the proc macro
that walks struct / enum definitions and emits the canonical encoder, the
canonical decoder, and the `SCHEMA_HASH: u128` constant from the type's
structural shape.

## Quick start

```rust,no_run
use pardosa_schema::{NonEmptyEventString, GenomeSafe};

let org: NonEmptyEventString<128> = NonEmptyEventString::try_new("acme")
    .expect("nonempty and within bound");

// Use it inside any GenomeSafe-derived event payload — the MAX is part of
// the schema identity, so changing it is a schema migration.
#[derive(Debug, GenomeSafe)]
#[repr(u8)]
enum DomainEvent {
    SweepStarted { org: NonEmptyEventString<128>, repo_count: u64 } = 0,
}
```

## Documentation

API docs: <https://docs.rs/pardosa-schema>

## Architecture decisions

- PGN-0003 "Canonical Encoding, Schema Hash, and EventSafe Bounds" —
  canonical, deterministic byte
  representation for every type that crosses a persistence or wire boundary;
  `EventSafe`, `GenomeSafe`, `GenomeOrd`
  are sealed; `Encode`, `Decode`, `Validate` are open extension points for
  adopter applications; `EventSafe` is a
  pure participation marker with codec capability (`Encode` + `Decode`)
  carried separately so the trait graph stays orthogonal.
- PGN-0009 "Migration Policy and Clean-Break Posture" — bounded
  wrappers' `MAX` is part of the schema identity.

The full ADR set lives under [`docs/adr/`](../../docs/adr/).

## License

Licensed under either of

- Apache License, Version 2.0
- MIT License

at your option.
