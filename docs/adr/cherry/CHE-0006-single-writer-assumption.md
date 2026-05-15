# CHE-0006. Single-Writer Assumption Per Aggregate

Date: 2026-04-24
Last-reviewed: 2026-04-27
Tier: S
Status: Accepted

## Related

References: CHE-0001, CHE-0004, SEC-0006

## Context

Event-sourced systems face a fundamental question: how many processes
can concurrently write to the same aggregate? Options:

1. **Multi-writer with distributed consensus** — Raft, Paxos, or
   similar protocol to coordinate writes. Complex, high latency.
2. **Multi-writer with optimistic concurrency** — all writers attempt,
   conflicts detected and retried. Requires a shared store with
   atomic compare-and-swap.
3. **Single-writer** — each aggregate instance is owned by exactly one
   process. No coordination needed.

Cherry-pit targets agent-first, single-process systems where
distributed coordination is overhead without benefit.

## Decision

Each aggregate instance is owned by exactly one OS process. No
distributed coordination protocol is used. Sequential auto-increment
IDs (NonZeroU64) are safe without distributed ID generation. Optimistic
concurrency (`expected_sequence` on `append`) serves as defense-in-depth
within the single writer, not as a distributed coordination mechanism.

R1 [2]: Each aggregate instance is owned by exactly one OS process
  with no distributed coordination
R2 [2]: Use optimistic concurrency as defense-in-depth within the
  single writer, not as distributed coordination

## Consequences

- Eliminates distributed consensus complexity entirely.
- Sequential `NonZeroU64` IDs are simple, fast, and Copy.
- Horizontal scaling per aggregate is impossible.
- No fencing existed at the storage level. **Mitigated**: CHE-0043 adds advisory file locking (`flock`) that detects a second writer on the same directory.
- Multi-node deployment requires external routing (NATS subject partitioning, process registry) — currently undesigned.
- Single-writer simplifies idempotency (CHE-0041): sequence numbers are monotonic within one writer, so duplicate detection reduces to a simple high-water-mark check rather than requiring distributed deduplication.
- The single-writer assumption is load-bearing. Changing it requires significant rearchitecture.
