# CHE-0016. Store-Created Envelopes with Correlation and Causation Tracking

Date: 2026-04-24
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: CHE-0004, SEC-0005, SEC-0008

## Context

When persisting domain events, metadata must be attached: event ID, aggregate ID, sequence number, timestamp. Caller-constructed envelopes risk mismatched IDs and wrong sequences. Store-constructed envelopes are correct by construction. Additionally, distributed tracing needs correlation ID (groups all events from one logical operation) and causation ID (the specific event that caused this event). Adding these fields after production data exists requires schema migration of all persisted envelopes.

## Decision

Callers pass `Vec<Event>` to the store. The store creates
`EventEnvelope<E>` by stamping: `event_id` (UUID v7), `aggregate_id`,
`sequence`, `timestamp`, `correlation_id` (Option), and `causation_id`
(Option). Callers never construct envelopes directly.

Correlation and causation IDs are `Option<Uuid>` — `None` for events
from user-initiated commands without tracing context, `Some` when
propagated through policies or sagas.

R1 [5]: Callers pass Vec<Event> to the store; the store creates
  EventEnvelope by stamping all metadata
R2 [5]: Callers never construct EventEnvelope instances directly;
  only the store creates envelopes
R3 [5]: Include correlation_id and causation_id as Option<Uuid>
  fields on every envelope for distributed tracing

## Consequences

- Malformed envelopes are impossible by construction.
- Single timestamp per batch ensures consistency within an atomic
  operation.
- Correlation/causation IDs enable distributed tracing across
  aggregates and bounded contexts without requiring external
  tracing infrastructure.
- The `Option<Uuid>` type means existing event creation (without
  tracing context) requires no changes — fields default to `None`.
- Adding tracing propagation to `CommandBus` and `CommandGateway`
  implementations is a future concern — the envelope schema is ready.
- `EventEnvelope` fields are private (enforced by CHE-0042).
  External construction via struct literal syntax is rejected at
  compile time. The safety guarantee is structural, not
  convention-based.
