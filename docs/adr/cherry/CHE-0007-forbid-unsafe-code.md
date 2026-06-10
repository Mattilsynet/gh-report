# CHE-0007. Forbid Unsafe Code

Date: 2026-04-25
Last-reviewed: 2026-05-14
Tier: B
Status: Accepted

## Related

References: CHE-0001, COM-0017, SEC-0004, RST-0005

## Context

Cherry-pit's P1 correctness and P2 security priorities (CHE-0001)
demand structural memory safety. RST-0005 establishes workspace-wide
`#![forbid(unsafe_code)]` as the primary rule. CHE-0007 is retained
as the cherry-domain anchor — surfacing the constraint under
`--tree CHE` for cherry-pit consumers — and instantiates RST-0005
for the cherry-pit crates.

## Decision

Every cherry-pit crate uses `#![forbid(unsafe_code)]` at the crate
root, instantiating RST-0005 R1 for the cherry domain. Active
cherry-pit workspace members: `adr-fmt`, `cherry-pit-app`,
`cherry-pit-core`, `cherry-pit-gateway`, `cherry-pit-projection`,
`cherry-pit-storage`, `cherry-pit-web`, `cherry-pit-wq`,
`gh-report`.

R1 [5]: Every cherry-pit crate uses #![forbid(unsafe_code)] at the
  crate root (per RST-0005 R1)
R2 [5]: No unsafe blocks, unsafe impl, or unsafe fn in any
  cherry-pit crate (per RST-0005 R1)
R3 [5]: Every new crate added to the workspace must include
  #![forbid(unsafe_code)] (per RST-0005 R1, inherited workspace-wide)

## Consequences

- Memory safety in cherry-pit code is structurally guaranteed by
  the compiler. `forbid` (not `warn` or `deny`) cannot be overridden
  by inner `#[allow]`.
- Operations needing `unsafe` (unchecked indexing, `MaybeUninit`)
  are unavailable; the framework relies on safe abstractions.
- Third-party dependencies are out of scope — audit via
  `cargo-geiger` per RST-0005 R3.
