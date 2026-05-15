# PAR-0007. Monotonic event_id for Idempotent Publish

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: PAR-0004

## Context

JetStream provides at-least-once delivery, not exactly-once. If a publish
succeeds but the ACK is lost (network timeout, partition), the caller sees a
failure while the event exists in JetStream. On startup replay, this phantom
event appears — divergence between observed failure and durable state.

Without an idempotency key, deduplication during replay is impossible. Adding
this field after initial deployment is a breaking serialization change.

The original design did not specify an event ID. A distributed systems review (§H2) identified this as
high-risk.

## Decision

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

R1 [5]: Assign event_id as a monotonic u64 at append time in the
  Dragline, never caller-supplied
R2 [5]: Use event_id as Nats-Msg-Id for JetStream publish-side
  deduplication in the format pardosa-{stream_name}-{event_id}
R3 [6]: Continue event_id from the old stream's last value plus one
  across stream generations without resets

## Consequences

- **Positive:** Idempotent publish and replay — the core requirement for at-least-once delivery.
- **Positive:** Total ordering of events across generations by `event_id`.
- **Positive:** 8 bytes per event (u64). UUID would add 16 bytes for no benefit under single-writer (PAR-0004).
- **Negative:** Counter overflow at `u64::MAX` produces `EventIdOverflow`. At 1B events/sec, overflow after ~584 years.
- **Negative:** `event_id` as first field in `Event<T>` — changing position breaks the genome schema hash.
- **Dependency:** JetStream's dedup window is finite (default 2 min). The `publish_timeout` (PAR-0008) must be within the dedup window for idempotency to hold.
