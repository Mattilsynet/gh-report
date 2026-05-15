# COM-0018. Single-Writer Concurrency as Foundational Invariant

Date: 2026-04-28
Last-reviewed: 2026-04-29
Tier: S
Status: Accepted

## Related

References: COM-0001, PAR-0004, CHE-0006

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

## Consequences

Domain crates can reason about state transitions sequentially. CHE-0006 and PAR-0004 become instances of this foundation rule. Shared-nothing architectures scale horizontally but require explicit fencing for failover and explicit coordination for cross-partition operations.
