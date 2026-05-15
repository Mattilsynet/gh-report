# ADR-012: Precursor chain verification on startup

**Status:** Accepted

### Context

Each event's `precursor` field (when not `Index::NONE`) forms a singly-linked
chain through the line, connecting events in the same fiber. If a precursor
points to a forward position, a different fiber's event, or itself, the fiber
history is corrupted — `history()` would return wrong events or loop
indefinitely.

Corruption sources: bugs in migration reindexing, manual event injection,
deserialization of tampered data, or replay of a corrupted stream.

### Decision

`Dragline::verify_precursor_chains()` validates every event in the line after
replay:

1. If `precursor` is `Index::NONE`, skip (first event in fiber).
2. If `precursor.as_usize() >= current_position`, reject — forward reference
   (includes self-reference).
3. If `line[precursor].domain_id() != event.domain_id()`, reject —
   cross-domain precursor.

Time complexity: O(n) single pass over the line. Called once on startup
after replay completes, before the server accepts writes.

On failure, returns `PardosaError::BrokenPrecursorChain { event_id, precursor }`
with enough context to identify the corrupted event.

### Consequences

- **Positive:** Detects corruption before the server serves reads — no
  silent data integrity loss.
- **Positive:** O(n) is acceptable for a one-time startup check. A line
  with 1M events completes in milliseconds.
- **Positive:** The check is structural, not semantic — it validates the
  graph topology, not application-level correctness.
- **Negative:** Does not detect all corruption. A precursor that points to
  a valid earlier event of the same fiber but is the *wrong* event
  (reordered within the fiber) would not be caught. This would require
  walking each fiber's chain end-to-end, which is O(n × fibers).
- **Negative:** Startup time scales linearly with line size. For very large
  lines (>10M events), consider making the check opt-in or sampling-based.

### References

- `src/dragline.rs`: `verify_precursor_chains()`
- `src/error.rs`: `PardosaError::BrokenPrecursorChain`
- pardosa-next.md Design Invariant 8
