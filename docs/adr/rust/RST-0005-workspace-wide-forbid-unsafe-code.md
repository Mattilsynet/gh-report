# RST-0005. Workspace-Wide forbid(unsafe_code)

Date: 2026-04-28
Last-reviewed: 2026-05-12
Tier: B
Status: Accepted

## Related

References: RST-0001, SEC-0004, GND-0002

## Context

CHE-0007 forbids unsafe code in cherry-pit-core. GEN-0006
achieves zero-copy deserialization without unsafe. SEC-0004
mandates restricting capabilities by default. These independent
decisions converge on the same principle: unsafe code is a
liability that bypasses Rust's ownership, borrowing, and lifetime
guarantees. In an application workspace (not a low-level systems
library), unsafe is almost never necessary — the standard library
and vetted dependencies provide safe abstractions for the
operations this workspace requires. Elevating the prohibition to
workspace level ensures new crates inherit the constraint
automatically.

## Decision

All workspace crates use `#![forbid(unsafe_code)]` at the crate
root. Unsafe is forbidden by default and any future exception
requires a dedicated ADR justification.

R1 [5]: Every crate in the workspace includes
  `#![forbid(unsafe_code)]` at the crate root, enforced by
  clippy's `disallowed-macros` or CI grep
R2 [5]: If a future crate requires unsafe for FFI or performance,
  it must have a dedicated ADR documenting the justification,
  the scope of unsafety, and the safety invariants maintained
R3 [6]: Dependencies are preferred that do not themselves use
  unsafe, or that minimize unsafe surface area with auditable
  safety comments (cargo-geiger as assessment tool)

## Consequences

Memory safety and undefined behavior are structurally eliminated
across the workspace, not just in individual crates. CHE-0007 and
GEN-0006 become instances of this workspace-level policy. New
crates inherit the constraint automatically. The trade-off is
that some micro-optimizations requiring unsafe are unavailable —
acceptable for an application workspace where correctness outranks
performance (CHE-0001). If an FFI boundary is ever needed, the
dedicated-ADR requirement ensures the decision is deliberate.
