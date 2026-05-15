# PAR-0012. Precursor Chain Verification on Startup

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: PAR-0008, GND-0005

## Context

Each event's `precursor` field (when not `Index::NONE`) forms a singly-linked
chain through the line, connecting events in the same fiber. If a precursor
points to a forward position, a different fiber's event, or itself, the fiber
history is corrupted — `history()` would return wrong events or loop
indefinitely.

Corruption sources: bugs in migration reindexing, manual event injection,
deserialization of tampered data, or replay of a corrupted stream.

## Decision

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

R1 [5]: Run Dragline::verify_precursor_chains as an O(n) single pass
  after replay completes and before the server accepts writes
R2 [5]: Reject any event whose precursor index is greater than or
  equal to the event's own position in the line
R3 [6]: Reject any event whose precursor references an event with a
  different domain_id than the event itself

## Consequences

Detects corruption before the server serves reads — no silent data integrity loss. O(n) is acceptable for a one-time startup check (1M events completes in milliseconds). The check is structural, validating graph topology rather than application-level correctness. Trade-offs: does not detect within-fiber reordering where a precursor points to a valid earlier same-fiber event but the wrong one (that would require O(n × fibers) chain walks). Startup time scales linearly with line size; for very large lines (>10M events), consider making the check opt-in or sampling-based.
