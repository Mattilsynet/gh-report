# CHE-0076. Event-History Causal Replay Capability

Date: 2026-06-13
Last-reviewed: 2026-06-15
Tier: B
Status: Accepted

## Related

References: CHE-0057, CHE-0039, CHE-0024, CHE-0005, CHE-0019, CHE-0025, CHE-0001

## Context

`EventStore::load` already returns the complete ordered stream for one
aggregate (`crates/cherry-pit-core/src/store.rs:113-164`). Envelopes
also expose `correlation_id` and `causation_id`
(`crates/cherry-pit-core/src/event.rs:288-310`). Agents need a typed
read capability that explains a projected answer from stored facts
without changing the write path or envelope schema.

## Decision

Introduce an `EventHistoryEventStore` extension trait for read-only
history and causal replay. P1 correctness wins over ergonomic query
shortcuts: explanations must be reconstructed from the append-only log
and existing causal metadata.

R1 [5]: Surface event-history replay as `EventHistoryEventStore`, an EventStore extension trait parented by CHE-0057

R2 [5]: Keep every method read-only; implementations return stored envelopes and never append, publish, or dispatch commands

R3 [5]: Bind the capability to one aggregate event type through the inherited EventStore associated type

R4 [5]: Use EventStore::load ordering as the source of truth for per-aggregate history and sequence-bounded replay

R5 [6]: Build causal explanations only from existing event_id, correlation_id, and causation_id envelope metadata

R6 [5]: Return an empty history for unknown aggregates, preserving EventStore::load boundary semantics

R7 [5]: Use RPITIT async signatures and forbid trait objects or runtime capability registries

R8 [5]: Keep cross-aggregate enumeration optional by composing with ListableEventStore when a caller needs discovery

R9 [5]: Add no new envelope fields, runtime schema migration, query language, or hosted query service

## Consequences

+ becomes easier: agents can answer why a read-model value exists by
  replaying the relevant stream and following causal pointers already stored
  in each envelope.

− becomes harder: stores that cannot satisfy causal replay omit the extension
  trait entirely, so downstream code must carry explicit capability bounds.

risks/migration: the Accepted capability is additive. Existing stores and
  projections remain valid; stores that do not implement the extension simply
  lack the event-history capability. The shipped `cherry-pit-core` surface is
  read-only over existing envelope metadata, so no persisted-format migration
  is required.
