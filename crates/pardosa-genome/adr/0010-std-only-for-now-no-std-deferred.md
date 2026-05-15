# ADR-010: std-only for now, no_std deferred

**Status:** Accepted (amended: no_std claims removed, April 2025)

### Context

The original design specified `core` → `alloc` → `std` tiered feature flags for
`no_std` support. However, the Phase 1 implementation used `std::error::Error`,
`std::collections`, and `std::sync` unconditionally. No `#![no_std]` attribute existed.
The `alloc` feature flag was defined in `Cargo.toml` but had no effect — enabling it
did not enable any `no_std` functionality.

No concrete `no_std` consumer existed or was planned.

### Decision

Remove the non-functional `alloc` feature from `Cargo.toml`. Document the crate as
`std`-only. Retain the `std` feature flag (always required) for forward compatibility.
Document the deferred `no_std` tiered model in genome.md §Future: no_std Support for
when an actual consumer exists.

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

### Consequences

- **Positive:** No misleading `no_std` claims. Users and dependents get accurate
  capability information.
- **Positive:** Reduces maintenance surface — no need to maintain untested `no_std`
  code paths.
- **Positive:** Design for `no_std` is documented and ready to implement when needed.
- **Negative:** Cannot be used in `no_std` environments until implemented.
- **Migration path:** When a `no_std` consumer exists: add `#![no_std]` attribute,
  gate `std::error::Error` impls, gate collections behind `alloc`, feature-gate
  `String` vs `&'static str` in error types. The tiered model is specified in
  genome.md §Future: no_std Support.

### References

- genome.md §Feature Flags, §Future: no_std Support
- `Cargo.toml` (current feature definitions)
- `error.rs`, `genome_safe.rs` (std dependencies)
