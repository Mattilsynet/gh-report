# RST-0002. Intentional Toolchain and Dependency Evolution

Date: 2026-04-27
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: RST-0001, COM-0013

## Context

RST-0001 pins the Rust toolchain to a specific stable version.
COM-0013 establishes that evolution should be guided and
intentional. Toolchain updates introduce change in three
dimensions: new lints surface warnings across the workspace, newly
stabilized APIs become available, and dependency compatibility may
require MSRV bumps. Dependency updates carry parallel risks:
behavioral changes in patches, harder regression bisection when
mixed with feature work, and accumulating known vulnerabilities.
Both update types need dedicated attention, not ambient drift.

## Decision

Toolchain and dependency updates are deliberate, atomic, reviewed
changes — never ambient drift or side effects of feature work.

R1 [5]: Toolchain bumps are dedicated PRs: update
  `rust-toolchain.toml`, `rust-version`, fix all new lint
  warnings, and verify CI — no feature work in the same PR
R2 [5]: Dependency updates are dedicated PRs: run `cargo update`,
  review `Cargo.lock` diff, run full test suite, submit standalone
R3 [5]: Recently stabilized Rust APIs are not adopted in the same
  PR that bumps the toolchain; allow one release cycle buffer
R4 [5]: Edition migrations are dedicated PRs: run
  `cargo fix --edition`, review for semantic correctness
R5 [6]: Toolchain and dependency updates are performed on a
  regular cadence rather than reactively

## Consequences

Blame history remains clean: lint fixes, dependency updates, and
feature work are in separate commits. Contributors never encounter
overnight CI breakage from unintentional drift. The one-release
buffer means the project trails the cutting edge by approximately
six weeks — an intentional trade-off of stability over novelty.
Risk of staleness if cadence slips; mitigation is treating updates
as maintenance commitments, not best-effort aspirations.
