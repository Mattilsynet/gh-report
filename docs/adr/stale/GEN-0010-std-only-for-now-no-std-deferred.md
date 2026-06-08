# GEN-0010. std-Only for Now — no_std Deferred

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: GEN-0008

## Context

The original design specified `core` → `alloc` → `std` tiered feature flags for
`no_std` support. However, the implementation used `std::error::Error`,
`std::collections`, and `std::sync` unconditionally. No `#![no_std]` attribute existed.
The `alloc` feature flag was defined in `Cargo.toml` but had no effect — enabling it
did not enable any `no_std` functionality.

No concrete `no_std` consumer existed or was planned.

## Decision

Remove the non-functional `alloc` feature from `Cargo.toml`. Document the crate as
`std`-only. Retain the `std` feature flag (always required) for forward compatibility.
Document the deferred `no_std` tiered model in [genome.md](../../plans/genome.md) §Future:
no_std Support for when an actual consumer exists.

**Previous state (removed):**
```toml
alloc = []  # had no effect — removed
```

**Current state:**
```toml
[features]
default = ["std", "derive"]
std = []
derive = ["dep:pardosa-derive"]
zstd = ["std"]  # Phase 3: will add dep:zstd
```

R1 [9]: Remove the non-functional alloc feature from Cargo.toml and
  document the crate as std-only
R2 [9]: Retain the std feature flag for forward compatibility
R3 [9]: Defer no_std support until an actual consumer exists

## Consequences

- No misleading `no_std` claims. Users get accurate capability information.
- Reduces maintenance surface — no untested `no_std` code paths.
- Design for `no_std` is documented and ready when needed.
- Cannot be used in `no_std` environments until implemented.
