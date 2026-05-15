# ADR-011: 64-bit target requirement

**Status:** Accepted

### Context

Pardosa uses `Index(u64)` for line positions. Converting `Index` to a `Vec`
index requires `u64 → usize`. On 32-bit targets, `usize` is 4 bytes — valid
`Index` values above `u32::MAX` would silently truncate, corrupting lookups.

The `Index::as_usize()` method documents this: "Panics on 32-bit targets if
the value exceeds `usize::MAX`." But a panic on a valid index is a
correctness bug, not an acceptable fallback.

### Decision

Reject 32-bit targets at compile time:

```rust
#[cfg(not(target_pointer_width = "64"))]
compile_error!("pardosa requires a 64-bit target (usize must be at least 8 bytes)");
```

This is placed at the crate root (`lib.rs`) so it fires immediately on any
attempt to compile pardosa for a 32-bit target.

### Consequences

- **Positive:** Eliminates an entire class of truncation bugs. `Index::value()
  as usize` is always lossless.
- **Positive:** Explicit — the error message explains why and what to do.
- **Negative:** Pardosa cannot be used on 32-bit embedded or WASM32 targets.
  Acceptable for an EDA storage layer that targets server deployments.
- **Negative:** If 32-bit support is ever needed, `Index` would need to be
  redesigned (e.g., `usize`-based or with checked conversion at every use).

### References

- `src/lib.rs`: `compile_error!` guard
- `src/event.rs`: `Index::as_usize()`
