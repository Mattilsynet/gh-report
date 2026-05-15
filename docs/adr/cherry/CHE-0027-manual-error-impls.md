# CHE-0027. Manual Error Trait Implementations in cherry-pit-core

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: CHE-0015

## Context

`cherry-pit-core` defines three error types requiring `Display` and `Error` impls. `thiserror` reduces boilerplate but adds a proc-macro dependency. `cherry-pit-core` is the foundation crate — its dependency tree is a multiplier for the entire ecosystem. Current dependencies are `serde`, `uuid`, `jiff` — all load-bearing. `thiserror` would be a fourth dependency added purely for convenience.

## Decision

`cherry-pit-core` uses manual `Display` and `Error` implementations. No
`thiserror` dependency.

R1 [9]: cherry-pit-core uses manual Display and Error implementations
  with no thiserror dependency
R2 [9]: Infrastructure crates may use thiserror where dependency
  count is less critical

Manual `Display::fmt` matches on all `DispatchError` variants with
structured formatting (aggregate_id, expected/actual sequence in
conflict messages). Manual `Error::source` chains to inner errors for
`Rejected` and `Infrastructure` variants; returns `None` for
`AggregateNotFound` and `ConcurrencyConflict`. Same pattern applies to
`StoreError` and `BusError`. See `cherry-pit-core/src/error.rs` for full
implementations.

`thiserror` **is** available in the workspace for infrastructure
crates (`cherry-pit-gateway`, `pardosa`, etc.) where dependency count is
less critical:

```toml
# workspace Cargo.toml
thiserror = "2"
```

## Consequences

- **Minimal dependency tree** — `cherry-pit-core` depends on three external crates, all load-bearing.
- **Full control over error formatting** — `source()` returns are explicit; `ConcurrencyConflict` has structured data.
- **More boilerplate** — ~60 lines of manual impls vs ~20 with `thiserror`. Maintenance burden grows with new variants.
- Infrastructure crates may use `thiserror` — this decision applies only to `cherry-pit-core`.
