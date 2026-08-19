# COM-0018. Single-Writer Concurrency as Foundational Invariant

Date: 2026-04-28
Last-reviewed: 2026-08-12 — refined — added R6 COM-0017:R4 enforcement statement naming job id projection-lock-tripwire (RST-0007:R5)
Tier: S
Status: Accepted

## Related

References: COM-0001, PGN-0010, CHE-0006

## Context

Multiple crates independently converged on the same concurrency
pattern: CHE-0006 mandates single-writer per aggregate, PAR-0004
mandates single-writer per stream, SEC-0006 mandates eliminating
race conditions by construction. Each domain discovered that
shared mutable state defended by fine-grained locks produces
correctness bugs, subtle data races, and reasoning difficulty that
exceeds the complexity budget (COM-0001). The pattern recurs
because it reflects a fundamental force: concurrent mutation of
the same logical entity requires either serialization (locks,
queues) or partitioning (single owner). Partitioning eliminates
contention entirely, making the absence of races structurally
guaranteed rather than tested.

## Decision

Single-writer ownership is a workspace-wide foundational
invariant. Every mutable resource has exactly one writer at any
point in time, enforced by partitioning rather than shared-state
synchronization.

R1 [2]: Each mutable resource has exactly one owning writer;
  concurrent write access to the same logical entity is a design
  error, not a synchronization problem
R2 [2]: Prefer ownership partitioning (sharding, actor isolation,
  channel-based hand-off) over shared-state locking as the primary
  concurrency mechanism
R3 [3]: Where shared reads are required, use read-only snapshots
  or immutable projections rather than read-write lock sharing
R4 [3]: Document the single-writer boundary for each stateful
  component, identifying what entity owns the write path
R5 [3]: Ownership transfer for MsgpackFileStore, Dragline, and
  JetStream stream writers uses fencing, leases, epochs, or
  compare-and-swap before the replacement writer mutates state
R6 [3]: CI enforces R4's write-path boundary for gh-report's projection state
  with a build-time tripwire (job id projection-lock-tripwire,
  .github/workflows/ci-reusable.yml): every .projection_state.lock( call site
  MUST reside in crates/gh-report/src/app/state.rs, so the single writer is
  acquired through one auditable chokepoint; a call site elsewhere fails the
  build (COM-0017:R4).

## Consequences

Domain crates can reason about state transitions sequentially. CHE-0006 and PAR-0004 become instances of this foundation rule. Shared-nothing architectures scale horizontally but require explicit fencing for failover and explicit coordination for cross-partition operations.
