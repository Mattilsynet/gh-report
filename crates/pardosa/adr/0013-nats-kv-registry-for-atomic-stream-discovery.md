# ADR-013: NATS KV registry for atomic stream discovery

**Status:** Accepted (amended April 2026 — CAS guard added)

### Context

The new-stream migration model (ADR-005) creates a new JetStream stream per
migration. Consumers and the server itself need to discover which stream is
currently active. This requires an atomic pointer — updating the stream name
and generation must be a single operation to prevent split-state reads.

### Decision

Use a NATS KV bucket (`PARDOSA_REGISTRY`) as the stream discovery mechanism:

```
Bucket: PARDOSA_REGISTRY
  History: 10       # Last 10 pointer values for debugging
  TTL: 0            # No expiration on registry entries

Keys:
  {name}.active → "{generation}:{stream_name}"
                   e.g., "2:PARDOSA_orders_g2"
```

Single-key design ensures atomicity — no split state between generation and
stream name. Parsed by splitting on the first `:`.

**Startup:** Server reads `{name}.active` from KV and stores the returned
revision number in memory. If the key doesn't exist, this is a fresh
deployment — creates generation 1 and writes `1:PARDOSA_{name}_g1`.

**Migration cutover (ADR-005 step 4):** CAS update via `kv.update(key,
new_value, expected_revision)`. The expected revision is the one stored
at startup or after the last successful update. On success, the server
stores the new revision. On CAS failure (another process updated the key),
returns `PardosaError::RegistryConflict` — the losing process must abort
its migration and clean up the orphan stream it created.

**Consumer cutover:** Consumers watch the KV key. On change, they reconnect
to the new stream, re-read `Pardosa-*` headers, and resume processing by
skipping events with `event_id <= last_processed_event_id` (idempotent
catch-up via ADR-007).

### Consequences

- **Positive:** Atomic pointer update — consumers see a consistent
  `generation:stream_name` pair.
- **Positive:** KV history provides an operational audit trail of
  stream transitions.
- **Positive:** Watch-based notification eliminates polling. Consumers
  detect cutover in real-time.
- **Positive:** Rollback is trivial — re-point the KV key to the old
  stream name within the grace period.
- **Positive:** CAS guard prevents unauthorized or stale overwrites.
  Only the holder of the current revision can update the pointer.
- **Negative:** Adds a NATS KV dependency. If the KV bucket is
  unavailable, startup and migration fail (`RegistryUnavailable`).
- **Negative:** Single-key format (`generation:stream_name`) requires
  parsing. Minor complexity.
- **Negative:** CAS failure during migration requires cleanup of the
  orphan stream the losing process created. Cleanup is the migration
  caller's responsibility.
- **Negative:** Stale revision after another process updates the key
  requires a re-read from KV before retry.

### References

- pardosa-next.md §Architecture Model, §Phase 5, §Phase 6
- pardosa-next.md Design Invariant 5
