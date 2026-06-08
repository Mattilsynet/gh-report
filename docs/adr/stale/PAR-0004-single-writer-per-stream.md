# PAR-0004. Single-Writer Per Stream

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: S
Status: Accepted

## Related

Root: PAR-0004

## Context

Pardosa's publish-then-apply model with `RwLock<Dragline<T>>` assumes one
`Server<T>` instance per NATS JetStream stream. Multiple instances writing to
the same stream create divergent in-memory states with no reconciliation
mechanism — each instance applies only its own publishes locally, unaware of
events from other instances.

Distributed event sourcing frameworks that support multi-writer require either
CRDTs, fencing tokens, or leader election — fundamentally different
architectures with significant complexity costs.

## Decision

Single-writer per stream is a hard architectural constraint. Correctness
takes priority over availability — writes are rejected during partitions
rather than accepted with risk of silent divergence.

Enforced by:

1. **Mandatory fencing:** Every publish sets `Nats-Expected-Last-Subject-Sequence`
   to the sequence number returned by the previous successful publish. The
   JetStream server rejects the write if the expected sequence doesn't match,
   surfacing concurrent-writer conflicts immediately.
   - **Bootstrap (empty stream):** The first publish uses
     `Expected-Last-Subject-Sequence: 0`, which NATS interprets as "subject
     must have no prior messages." This eliminates the TOCTOU race between
     reading stream info and publishing.
   - **Sequence mismatch:** Returns `PardosaError::NatsUnavailable`. The
     caller retries or escalates. In-memory state is unchanged.
   - **No opt-out:** Fencing is always active. Testing uses an isolated stream
     per test, not disabled fencing.
2. **Documentation:** Constraint documented in design invariants and `Server<T>`
   doc comments. This is context for human understanding, not a safety mechanism.

Multi-instance deployment requires leader election or partitioning — a
fundamentally different architecture that is explicitly out of scope.

R1 [1]: Single-writer per stream is a hard architectural constraint
  with correctness taking priority over availability
R2 [3]: Every publish sets Nats-Expected-Last-Subject-Sequence to the
  previous successful publish sequence number for mandatory fencing
R3 [3]: The first publish uses Expected-Last-Subject-Sequence 0 to
  eliminate the TOCTOU race on empty streams

## Consequences

Simplifies the entire system — no conflict resolution, merge logic, or vector clocks. Wall-clock `timestamp: i64` suffices for total ordering (single writer eliminates clock skew). Monotonic `event_id: u64` provides globally unique identification without coordination. Mandatory fencing means a second instance's first publish is rejected immediately — fail-fast over silent divergence. Fencing is a safety net, not a lock — process-level coordination (exclusive deployment, leader election) is the primary enforcement mechanism; two processes can start simultaneously and build divergent in-memory state from replay before the loser's first publish is rejected. Trade-offs: single point of failure for writes (reads can use replicated consumers), horizontal write scaling requires stream partitioning, and callers must handle `NatsUnavailable` on stale-sequence rejection. If this constraint is ever lifted, most of the persistence layer and migration model must be redesigned.
