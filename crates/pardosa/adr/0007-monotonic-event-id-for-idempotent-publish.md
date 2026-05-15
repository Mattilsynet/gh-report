# ADR-007: Monotonic event_id for idempotent publish

**Status:** Accepted

### Context

JetStream provides at-least-once delivery, not exactly-once. If a publish
succeeds but the ACK is lost (network timeout, partition), the caller sees a
failure while the event exists in JetStream. On startup replay, this phantom
event appears — divergence between observed failure and durable state.

Without an idempotency key, deduplication during replay is impossible. Adding
this field after initial deployment is a breaking serialization change.

The original design (pardosa.md) did not specify an event ID. The distributed
systems review (§H2) identified this as high-risk.

### Decision

Add `event_id: u64` to `Event<T>` as the first field. Properties:

- **Globally monotonic across stream generations.** A new stream's first
  `event_id` continues from the old stream's last `event_id + 1`. No resets.
- **Assigned at append time** by the `Dragline`. Not caller-supplied.
- **Used as `Nats-Msg-Id`** for JetStream publish-side deduplication:
  `pardosa-{stream_name}-{event_id}`.
- **Used during replay** to skip already-applied events (idempotent replay).
- **Cross-generation stable identifier.** Unlike `Index` (which is
  generation-scoped and remapped during migration), `event_id` is the
  permanent identity of an event across the entire lifetime of a pardosa
  instance.

### Consequences

- **Positive:** Idempotent publish and replay — the core requirement for
  at-least-once delivery.
- **Positive:** Total ordering of events across generations. Two events
  can always be ordered by `event_id`.
- **Positive:** 8 bytes per event (u64). UUID would add 16 bytes for no
  benefit under the single-writer constraint (see ADR-004).
- **Negative:** Counter overflow at `u64::MAX` produces `EventIdOverflow`.
  At 1 billion events per second, overflow occurs after ~584 years.
- **Negative:** `event_id` is the first field in `Event<T>` — changing its
  position breaks the genome schema hash.

### References

- `src/event.rs`: `event_id` field, `Event::event_id()` accessor
- `src/dragline.rs`: `next_event_id`, `peek_event_id()`
- pardosa.md §H2 (Distributed Systems Review)
- pardosa-next.md §P1, Design Invariants 2, 6
