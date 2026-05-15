# ADR-014: Backpressure and circuit breaker

**Status:** Accepted

### Context

ADR-008 holds `RwLock` across async NATS publish. Without backpressure,
high write throughput serializes all mutations behind network latency.
Without a circuit breaker, sustained NATS unavailability causes every write
to block until the publish timeout (ADR-008 amendment, default 5s),
consuming tokio tasks and degrading read availability.

### Decision

1. **Write-lock timeout** (5s default, configurable via
   `ServerConfig::publish_timeout`) — per ADR-008 amendment. Bounds the
   maximum time the write lock is held across a single NATS publish.

2. **Circuit breaker** — After 3 consecutive `NatsUnavailable` errors
   (configurable via `ServerConfig::circuit_breaker_threshold`), the
   server enters degraded mode:
   - Reads continue to be served from in-memory state.
   - Writes are rejected immediately with `PardosaError::NatsUnavailable`
     — no NATS publish attempted.
   - Recovery: counter resets on the next successful NATS health check
     (ping) or publish. The NATS client's built-in reconnection handles
     transport recovery; the circuit breaker tracks application-level
     publish success.

3. **Migration bypass** — Circuit breaker is suppressed when
   `Dragline::is_migrating() == true`. Migration has its own timeout and
   retry logic (Phase 4). The circuit breaker must not interfere with
   migration-initiated NATS operations, which may cause transient failures
   during stream creation.

4. **Future: bounded publish channel** — For the `transact()` batching
   pattern (automerge-ideas.md §1), a bounded channel between the
   application write path and the NATS publish path provides backpressure
   at the batch level. Deferred to Phase 5.

### Consequences

- **Positive:** Write latency bounded at `publish_timeout`. A NATS
  partition triggers circuit breaker within `threshold × timeout`
  (default: 15s), after which writes fail immediately (no blocking).
- **Positive:** Degraded mode preserves read availability during NATS
  outages. Consumers querying in-memory state are unaffected.
- **Positive:** Automatic recovery — no operator intervention required
  once NATS reconnects.
- **Negative:** Circuit breaker adds state to `Server<T>`: failure
  counter and degraded flag. Minor complexity.
- **Negative:** Threshold of 3 may trip on transient NATS leader election
  (~1s). Configurable to mitigate. A time-window-based approach (e.g.,
  3 failures within 10s) is a future refinement.
- **Negative:** Degraded mode means writes are silently dropped from the
  caller's perspective — the caller must handle `NatsUnavailable` and
  decide whether to retry or queue.

### References

- pardosa.md §M1 (RwLock contention)
- ADR-008 (Publish-then-apply, write-lock timeout)
- ADR-004 (Single-writer, mandatory fencing)
- automerge-ideas.md §1 (Transaction with Auto-Rollback)
