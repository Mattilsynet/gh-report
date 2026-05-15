# ADR-004: Single-writer per stream

**Status:** Accepted (amended April 2026 — fencing mandatory)

### Context

Pardosa's publish-then-apply model with `RwLock<Dragline<T>>` assumes one
`Server<T>` instance per NATS JetStream stream. Multiple instances writing to
the same stream create divergent in-memory states with no reconciliation
mechanism — each instance applies only its own publishes locally, unaware of
events from other instances.

Distributed event sourcing frameworks that support multi-writer require either
CRDTs, fencing tokens, or leader election — fundamentally different
architectures with significant complexity costs.

### Decision

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
     **Open question for Phase 5:** A dedicated `FencingRejected` variant
     would let callers distinguish "NATS down" (retry) from "concurrent
     writer detected" (abort). Decide during implementation.
   - **No opt-out:** Fencing is always active. Testing uses an isolated stream
     per test, not disabled fencing.
2. **Documentation:** Constraint documented in design invariants and `Server<T>`
   doc comments. This is context for human understanding, not a safety mechanism.

Multi-instance deployment requires leader election or partitioning — a
fundamentally different architecture that is explicitly out of scope.

### Consequences

- **Positive:** Simplifies the entire system — no conflict resolution, no
  merge logic, no vector clocks.
- **Positive:** Wall-clock `timestamp: i64` is adequate for total ordering
  (single writer eliminates clock skew).
- **Positive:** Monotonic `event_id: u64` provides globally unique
  identification without coordination.
- **Positive:** Mandatory fencing means a second instance's first publish is
  rejected immediately — fail-fast instead of silent divergence.
- **Negative:** Single point of failure for writes. Reads can be served from
  replicated consumers.
- **Negative:** Horizontal write scaling requires partitioning across
  multiple pardosa instances with separate streams.
- **Negative:** Mandatory fencing means writes are rejected (not queued) when
  the expected sequence is stale. Callers must handle
  `NatsUnavailable` and retry.
- **Risk:** If this constraint is ever lifted, most of the persistence layer
  and migration model must be redesigned. This is accepted as a deliberate
  simplicity trade-off.

### References

- pardosa.md §H4 (Distributed Systems Review)
- pardosa-next.md Design Invariant 3
