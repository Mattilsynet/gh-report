# CHE-0030. Flat Public API via Private Modules

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: CHE-0029

## Context

Rust crates expose their public API through module paths. The internal
module structure can be visible (via `pub mod`) or hidden (via private
`mod` + `pub use` re-exports). Two approaches:

1. **Public module tree** — `pub mod aggregate`, `pub mod event`, etc.
   Users write `cherry_pit_core::aggregate::Aggregate`. Internal
   reorganization is a breaking change.
2. **Flat re-exports** — all modules are private `mod`. Public items
   are re-exported via `pub use` in `lib.rs`. Users write
   `cherry_pit_core::Aggregate`. Internal reorganization is invisible to
   users.

## Decision

All cherry-pit crates use private modules with selective `pub use`
re-exports. Users interact with a flat namespace:

R1 [5]: Use private modules with selective pub use re-exports in all
  cherry-pit crates
R2 [5]: Internal module structure is an implementation detail; the
  public API is the set of re-exported items

```rust
use cherry_pit_core::{Aggregate, HandleCommand, DomainEvent, EventEnvelope};
use cherry_pit_gateway::MsgpackFileStore;
```

Internal module structure (`aggregate.rs`, `event.rs`, `store.rs`,
etc.) is an implementation detail.

## Consequences

- Module reorganization is a non-breaking change. The public API is the set of re-exported items.
- Users see a flat, discoverable API — `cherry_pit_core::` autocomplete shows all public types.
- Re-exports must be maintained manually in `lib.rs`. Adding a new public type requires both definition and re-export.
- All workspace crates follow this pattern.
