# ADR-008: Publish-then-apply with durable-first semantics

**Status:** Accepted (amended April 2026 — write-lock timeout added)

### Context

Event sourcing systems must choose between apply-then-publish (risk: in-memory
state diverges from durable store on publish failure) and publish-then-apply
(risk: stale in-memory state on ACK loss, but durable store is always
authoritative).

Pardosa's single-writer model (ADR-004) and idempotent replay via `event_id`
(ADR-007) make publish-then-apply safe: on any failure, the in-memory state
can be reconstructed from the durable store.

### Decision

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
  (ADR-007). The timeout adds no new failure modes — it bounds an existing one.
- *Interaction with fencing (ADR-004):* Timeout and fencing rejection both
  map to `NatsUnavailable`. The caller's retry path is identical.

### Consequences

- **Positive:** No split-brain — the durable store is always authoritative.
  In-memory state is a cache that can be rebuilt from replay.
- **Positive:** Failure mode is clean — the caller sees `NatsUnavailable`
  and can retry. No partial state.
- **Positive:** Simplifies reasoning about consistency — no WAL, no
  two-phase commit, no compensation logic.
- **Negative:** Write latency includes network round-trip to NATS.
  Under high write throughput, lock contention serializes all mutations
  behind network latency.
- **Negative:** `RwLock` held across async NATS publish. Bounded by
  `publish_timeout` (default 5s). A NATS partition causes write rejection
  within the timeout, not indefinite blocking. See ADR-014 for circuit
  breaker behavior under sustained failures.
- **Negative:** No batching — each mutation is a separate publish.
  The `transact()` pattern (from automerge-ideas.md) is a future
  enhancement for atomic multi-mutation batches.

### References

- pardosa.md §Design Invariants (item 1), §M1 (RwLock contention)
- pardosa-next.md Design Invariant 1
- automerge-ideas.md §1 (Transaction with Auto-Rollback)
