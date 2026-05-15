# CHE-0026. Correctness-First Build Configuration

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: CHE-0007

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

[workspace.lints.clippy]
pedantic = { level = "warn", priority = -1 }
```

Key points:

R1 [9]: Set overflow-checks = true in the release profile so integer
  overflow panics in production
R2 [9]: Set clippy::pedantic at warn level across the entire workspace
R3 [9]: Use lto = true and codegen-units = 1 for whole-program
  optimization in release builds
R4 [9]: CI verifies release profile invariants lto, strip,
  codegen-units, and overflow-checks before merging changes

- **`overflow-checks = true`** — integer overflow panics in release
  builds, not just debug builds. Consistent with design priority P1
  (correctness > speed).
- **`clippy::pedantic`** at warn level — applied workspace-wide so
  all crates share the same lint standard.
- **`lto = true` + `codegen-units = 1`** — enables whole-program
  optimization. Longer compile times for release builds; smaller,
  faster binaries.
- **`strip = true`** — removes debug symbols from release binaries.
  Reduces binary size.

## Consequences

Overflow is caught in debug and release; `checked_add` remains defense-in-depth. Clippy pedantic is workspace-wide. Release builds are slower due to LTO and one codegen unit. CI guards release-profile drift.
