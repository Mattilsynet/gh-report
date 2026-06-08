# PAR-0011. 64-bit Target Requirement

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: PAR-0001

## Context

Pardosa uses `Index(u64)` for line positions. Converting to `Vec` index requires `u64 → usize`. On 32-bit targets, `usize` is 4 bytes — valid `Index` values above `u32::MAX` would silently truncate, corrupting lookups. A panic on a valid index is a correctness bug, not an acceptable fallback.

## Decision

Reject 32-bit targets at compile time:

```rust
#[cfg(not(target_pointer_width = "64"))]
compile_error!("pardosa requires a 64-bit target (usize must be at least 8 bytes)");
```

This is placed at the crate root (`src/lib.rs`) so it fires
immediately on any attempt to compile pardosa for a 32-bit target.

R1 [9]: Place a compile_error macro at the pardosa crate root
  rejecting targets where target_pointer_width is not 64
R2 [9]: Use Index::value() as usize without checked conversion
  relying on the 64-bit compile gate for lossless cast

## Consequences

- Eliminates an entire class of truncation bugs. `Index::value() as usize` is always lossless.
- Explicit — the error message explains why and what to do.
- Cannot be used on 32-bit or WASM32 targets. Acceptable for server deployments.
- If 32-bit support is ever needed, `Index` would need redesign.
