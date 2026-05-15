# COM-0015. Communicate Architecture Through Structure

Date: 2026-04-26
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: COM-0006

## Context

Read (Communication Patterns, Ch. 1) argues architecture is only as good as its communication. Brown and Clements et al. emphasize "architecture-evident coding style": the source tree should communicate structure without external documentation. Structural channels include directory layout mirroring domain boundaries, naming conventions communicating architectural role, the Cargo.toml dependency graph as an enforceable DAG, and the ADR system with cross-references and linted indexes.

Cherry-pit's `docs/adr/` tree mirrors the four-domain taxonomy. The crate DAG mirrors COM-0012. Flat public APIs (CHE-0030) make the surface browsable from `lib.rs`. Auto-generated indexes via `adr-fmt` keep the system self-documenting.

## Decision

Architecture should be legible from the project's structure
itself. Directory layout, naming, dependency graphs, and the ADR
system are primary communication channels — not supplements to
external documentation.

R1 [5]: The source tree must reflect domain boundaries, layer
  boundaries, and crate boundaries so a developer can sketch
  the architecture from the directory listing
R2 [5]: Crate names, module names, and type names communicate their
  architectural role without requiring lookup
R3 [6]: The Cargo.toml dependency graph is the physical architecture
  diagram; it must be inspectable, minimal, and aligned with
  intended layering
R4 [5]: Generated indexes are kept current automatically; stale
  indexes communicate false architecture and are worse than none
R5 [6]: Prose supplements structure but does not replace it;
  structure communicates what exists, prose communicates why

## Consequences

The `docs/adr/` directory structure is an architectural communication tool — each subdirectory represents a domain. `adr-fmt` auto-generates README indexes, preventing drift. The crate DAG (CHE-0029) is both a compile-time enforcement mechanism (COM-0012) and a communication tool — `Cargo.toml` files reconstruct the layering. Flat public APIs (CHE-0030) communicate the surface directly from `lib.rs`. The ADR system investment (140+ ADRs, indexes, cross-references, linting) is the architecture's communication infrastructure. Maintenance effort is mitigated by automation: `adr-fmt`, Cargo's dependency checker, and `rustfmt`.
