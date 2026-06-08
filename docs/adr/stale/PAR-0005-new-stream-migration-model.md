# PAR-0005. New-Stream Migration Model

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: PAR-0004

## Context

The original design (pardosa-design.md Phase 4) specified in-place line rebuild
with index remapping under a write-lock. This approach holds the write-lock for
O(n) over all events, has no crash-recovery semantics (a crash mid-rebuild
corrupts the line), and requires complex index remap tables.

## Decision

Migrations never mutate existing streams or files. Each migration:

1. Reads the active stream/file (immutable source of truth).
2. Creates a new JetStream stream and/or genome file.
3. Writes surviving events (with optional schema upcast) to the new
   stream/file. Indices are contiguously remapped starting from 0.
4. Atomically updates the NATS KV registry pointer (see PAR-0013).
5. Sets `max_age` on the old stream for a configurable grace period
   (default: 7 days).
6. Old genome files are retained until the operator deletes them.

The write-lock is held only during step 1 (set `migrating = true`, instantaneous)
and step 4 (build new in-memory `Dragline`, update pointer). Steps 2–3 release
the lock — reads proceed against the stale but self-consistent `Dragline`.

Crash recovery: discard incomplete new stream/file, retry from intact old stream.

`event_id` values are preserved across migrations (not reset). The new stream's
first post-migration `event_id` continues from the old stream's last
`event_id + 1`.

R1 [5]: Migrations create a new JetStream stream and genome file
  rather than mutating existing streams or files
R2 [5]: Atomically update the NATS KV registry pointer in a single
  CAS operation during migration cutover
R3 [6]: Preserve event_id values across migrations so the new stream
  continues from the old stream's last event_id plus one

## Consequences

Old stream is immutable — crash-safe with idempotent retry. Write-lock duration reduced from O(n) to O(1) for lock-held phases. Rollback within the grace period is trivial — re-point the KV registry to the old stream name. Reads continue during migration against a self-consistent snapshot. Trade-offs: doubles storage during the grace period, consumers must handle stream cutover (see PAR-0013), and `Index` values are generation-scoped — code caching indices must invalidate on generation change. `event_id` is the cross-generation stable identifier.
