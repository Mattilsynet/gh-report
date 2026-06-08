# PAR-0013. NATS KV Registry for Atomic Stream Discovery

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: PAR-0004

## Context

The new-stream migration model (PAR-0005) creates a new JetStream stream per
migration. Consumers and the server itself need to discover which stream is
currently active. This requires an atomic pointer — updating the stream name
and generation must be a single operation to prevent split-state reads.

## Decision

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

**Migration cutover (PAR-0005 step 4):** CAS update via `kv.update(key,
new_value, expected_revision)`. The expected revision is the one stored
at startup or after the last successful update. On success, the server
stores the new revision. On CAS failure (another process updated the key),
returns `PardosaError::RegistryConflict` — the losing process must abort
its migration and clean up the orphan stream it created.

**Consumer cutover:** Consumers watch the KV key. On change, they reconnect
to the new stream, re-read `Pardosa-*` headers, and resume processing by
skipping events with `event_id <= last_processed_event_id` (idempotent
catch-up via PAR-0007).

R1 [5]: Store the active stream pointer as a single KV key in format
  generation:stream_name for atomic discovery
R2 [5]: Use CAS update via kv.update with expected_revision for
  migration cutover to prevent stale overwrites
R3 [6]: Consumers watch the KV key and reconnect to the new stream
  on change using event_id for idempotent catch-up

## Consequences

Atomic pointer update — consumers see a consistent `generation:stream_name` pair. KV history provides an operational audit trail. Watch-based notification eliminates polling for real-time cutover detection. Rollback is trivial — re-point the KV key within the grace period. CAS guard prevents stale overwrites. Trade-offs: NATS KV dependency means startup and migration fail if the bucket is unavailable (`RegistryUnavailable`). CAS failure during migration requires cleanup of the orphan stream (caller's responsibility). Stale revision after another process updates the key requires a re-read before retry.
