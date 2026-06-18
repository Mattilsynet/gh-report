# CHE-0033. UUID v7 for Event Identity

Date: 2026-04-25
Last-reviewed: 2026-06-19
Tier: D
Status: Accepted

## Related

References: CHE-0016

## Context

Every `EventEnvelope` carries an `event_id`. UUID v4 is random with no ordering. UUID v7 (RFC 9562) embeds a millisecond timestamp with 62 bits of randomness — naturally sortable by creation time. Cherry-pit needs global uniqueness without coordination and chronological ordering for debugging.

## Decision

`event_id` is a UUID v7, generated via `uuid::Uuid::now_v7()`.

R1 [10]: Generate event_id as UUID v7. Default construction uses
  uuid::Uuid::now_v7(); DST-substrate adapters MAY derive deterministic
  v7 ids from substrate-native ordering when replays must produce
  identical envelope ids. R2 and R3 remain binding.
R2 [10]: UUID v7 IDs are globally unique across all aggregate types
  and processes without coordination
R3 [10]: Use EventEnvelope::sequence as the authoritative per-stream
  ordering field; treat event_id ordering as diagnostic metadata

```rust
// EventEnvelope field
pub event_id: uuid::Uuid,

// Generated in MsgpackFileStore::build_envelopes
event_id: uuid::Uuid::now_v7(),
```

## Consequences

UUID v7 gives compact global identity and approximate chronological sort for debugging. It is not causal ordering: clock rollback, restart, and multi-process generation can disagree with stream order. `EventEnvelope::sequence` remains authoritative inside a stream. Changing ID schemes requires migration.

Adapters wrapping deterministic substrates (e.g. pardosa) may construct `event_id` deterministically from substrate-native ordering. `now_v7()` remains the default; deterministic construction is a documented opt-in for replayability. R2 and R3 unchanged.
