# PAR-0008. Publish-then-Apply with Durable-First Semantics

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: S
Status: Accepted

## Related

References: PAR-0004, GND-0001

## Context

Event sourcing systems must choose between apply-then-publish (risk: in-memory
state diverges from durable store on publish failure) and publish-then-apply
(risk: stale in-memory state on ACK loss, but durable store is always
authoritative).

Pardosa's single-writer model (PAR-0004) and idempotent replay via `event_id`
(PAR-0007) make publish-then-apply safe: on any failure, the in-memory state
can be reconstructed from the durable store.

The original design established durable-first as a core invariant. An
`RwLock` contention analysis (§M1) identified write-lock duration as the
primary throughput bottleneck.

## Decision

Each mutation = one NATS publish (or one genome file append). In-memory
`Dragline` state is updated only after receiving the durable ACK.

On failure:
- In-memory state remains unchanged (as if the operation never happened).
- The caller receives `PardosaError::NatsUnavailable`.
- If the event was actually persisted (ACK-lost scenario), startup replay
  deduplicates by `event_id` — the phantom event is applied idempotently.

No compound operations at the library layer. Each CRUD operation (create,
update, detach, rescue) is a single atomic publish-then-apply cycle.

**Write-lock timeout:** The NATS publish call inside the write lock is
wrapped in `tokio::time::timeout`. Default: 5 seconds, configurable via
`ServerConfig::publish_timeout`.

- *Timeout boundary:* Covers only the NATS publish call. The in-memory apply
  (push to `Vec`, update `HashMap`) is infallible and O(1) — it executes
  after the timeout window closes successfully.
- *On timeout:* Return `PardosaError::NatsUnavailable`. In-memory state
  unchanged — identical to the existing ACK-loss behavior.
- *Phantom-event scenario:* If NATS persisted the event but the ACK was lost
  (or arrived after timeout), startup replay deduplicates by `event_id`
  (PAR-0007). The timeout adds no new failure modes — it bounds an existing one.
- *Interaction with fencing (PAR-0004):* Timeout and fencing rejection both
  map to `NatsUnavailable`. The caller's retry path is identical.

R1 [1]: Update in-memory Dragline state only after receiving the
  durable ACK from NATS or genome file append
R2 [3]: Each CRUD operation is a single atomic publish-then-apply
  cycle with no compound operations at the library layer
R3 [3]: Wrap the NATS publish call inside the write lock with
  tokio::time::timeout bounded by ServerConfig::publish_timeout
R4 [3]: Use tokio::sync::RwLock for Dragline state to permit holding
  the write lock across async NATS publish await points without
  blocking the executor thread

## Consequences

No split-brain — the durable store is always authoritative; in-memory state is a rebuildable cache. Failure mode is clean: caller sees `NatsUnavailable` with no partial state. No WAL, two-phase commit, or compensation logic needed. Trade-offs: write latency includes the NATS round-trip, and lock contention serializes mutations behind network latency. `RwLock` held across async publish is bounded by `publish_timeout` (default 5s); a NATS partition triggers write rejection within that window. See PAR-0014 for circuit breaker behavior. No batching — each mutation is a separate publish; a `transact()` pattern is a future enhancement.
