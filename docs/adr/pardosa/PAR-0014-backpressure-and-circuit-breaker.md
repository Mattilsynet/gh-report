# PAR-0014. Backpressure and Circuit Breaker

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: C
Status: Accepted

## Related

References: PAR-0008, GND-0001, GND-0004

## Context

PAR-0008 holds `RwLock` across async NATS publish. Without backpressure,
high write throughput serializes all mutations behind network latency.
Without a circuit breaker, sustained NATS unavailability causes every write
to block until the publish timeout (PAR-0008 amendment, default 5s),
consuming tokio tasks and degrading read availability.

## Decision

1. **Write-lock timeout** (5s default, configurable via
   `ServerConfig::publish_timeout`) — per PAR-0008 amendment. Bounds the
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
   retry logic. The circuit breaker must not interfere with
   migration-initiated NATS operations, which may cause transient failures
   during stream creation.

R1 [7]: After circuit_breaker_threshold consecutive NatsUnavailable
  errors, reject writes immediately without attempting NATS publish
R2 [7]: Continue serving reads from in-memory state during degraded
  mode while writes are rejected
R3 [8]: Reset the circuit breaker failure counter on the next
  successful NATS health check or publish operation

4. **Future: bounded publish channel** — For a `transact()` batching
   pattern, a bounded channel between the application write path and the
   NATS publish path provides backpressure at the batch level. Deferred
   to a future enhancement.

## Consequences

Write latency bounded at `publish_timeout`. Circuit breaker trips within `threshold × timeout` (default 15s), then writes fail immediately. Degraded mode preserves read availability during NATS outages — but reads serve potentially stale state; clients should treat degraded reads as eventually consistent until replay catches up. Automatic recovery once NATS reconnects. Trade-offs: threshold of 3 may trip on transient leader election (~1s); configurable. Callers must handle `NatsUnavailable` during degraded mode.
