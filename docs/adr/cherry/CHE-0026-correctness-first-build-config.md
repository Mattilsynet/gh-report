# CHE-0026. Correctness-First Build Configuration

Date: 2026-04-25
Last-reviewed: 2026-06-19
Tier: D
Status: Accepted

## Related

References: CHE-0007, RST-0003

## Context

Rust's release profile controls optimizations and runtime checks. `overflow-checks = false` (release default) wraps integer overflow silently — the framework uses `u64` for aggregate IDs and sequence numbers where silent overflow could cause data corruption. Clippy pedantic catches subtle correctness issues. LTO and `codegen-units = 1` optimize binary size and performance without affecting correctness.

## Decision

The workspace `Cargo.toml` sets:

```toml
[profile.release]
lto = true
strip = true
codegen-units = 1
overflow-checks = true
```

Key points:

R1 [9]: Set overflow-checks = true in the release profile so integer
  overflow panics in production
R2 [9]: Use lto = true and codegen-units = 1 for whole-program
  optimization in release builds

- **`overflow-checks = true`** — integer overflow panics in release
  builds, not just debug builds. Consistent with design priority P1
  (correctness > speed).
- **`lto = true` + `codegen-units = 1`** — enables whole-program
  optimization. Longer compile times for release builds; smaller,
  faster binaries.
- **`strip = true`** — removes debug symbols from release binaries.
  Reduces binary size.

## Consequences

Overflow is caught in debug and release; `checked_add` remains defense-in-depth. Release builds are slower due to LTO and one codegen unit.
