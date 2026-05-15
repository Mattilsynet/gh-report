# ADR-005: New-stream migration model

**Status:** Accepted (Phase 4 implementation pending)

### Context

The original design (pardosa.md Phase 4) specified in-place line rebuild with
index remapping under a write-lock. This approach holds the write-lock for O(n)
over all events, has no crash-recovery semantics (a crash mid-rebuild corrupts
the line), and requires complex index remap tables.

### Decision

Migrations never mutate existing streams or files. Each migration:

1. Reads the active stream/file (immutable source of truth).
2. Creates a new JetStream stream and/or genome file.
3. Writes surviving events (with optional schema upcast) to the new
   stream/file. Indices are contiguously remapped starting from 0.
4. Atomically updates the NATS KV registry pointer (see ADR-013).
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

### Consequences

- **Positive:** Old stream is immutable — crash-safe, idempotent retry.
- **Positive:** Write-lock duration reduced from O(n) to O(1) for the
  lock-held phases (steps 1 and 4).
- **Positive:** Rollback within the grace period is trivial — re-point the
  KV registry to the old stream name.
- **Positive:** Reads continue during migration against a self-consistent
  snapshot.
- **Negative:** Doubles storage during the grace period (old + new stream).
- **Negative:** Consumers must handle stream cutover (see ADR-013 and
  pardosa-next.md §Phase 6).
- **Negative:** `Index` values are generation-scoped — code that caches
  indices must invalidate on generation change. `event_id` is the
  cross-generation stable identifier.

### References

- pardosa-next.md §Phase 4 — Migration (New-Stream Model)
- pardosa-next.md Design Invariants 4, 5, 6, 10
