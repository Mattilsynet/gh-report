# CHE-0039. Correlation Context Propagation

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: A
Status: Accepted

## Related

References: CHE-0004

## Context

`EventEnvelope` carries `correlation_id` and `causation_id` fields (CHE-0016), but the current API cannot populate them — all port signatures lack a correlation parameter, and `build_envelopes` hardcodes both to `None`. Three propagation styles were considered: explicit parameter (transparent, consistent with CHE-0003), task-local context (ergonomic but invisible, violating "no magic"), and middleware (deferred to `cherry-pit-agent`).

## Decision

Explicit parameter propagation. A `CorrelationContext` struct is
added to `cherry-pit-core` and threaded through all port methods that
produce `EventEnvelope`s.

R1 [4]: Thread CorrelationContext as an explicit parameter through
  all port methods that produce EventEnvelopes
R2 [4]: CorrelationContext does not implement Default; every call site
  must explicitly choose none(), correlated(id), or new(corr, cause)
R3 [4]: Policy implementations construct CorrelationContext for
  downstream commands using the source event's event_id and
  correlation_id

### Type definition

`CorrelationContext` is a struct in `cherry-pit-core` with two fields:
`correlation_id: Option<uuid::Uuid>` (groups related events) and
`causation_id: Option<uuid::Uuid>` (the event_id that caused this
command). Three named constructors: `none()` (user-initiated, no
tracing), `correlated(id)` (first command in a chain), and
`new(corr, cause)` (policy-propagated from a prior event). Does not
implement `Default`. See `cherry-pit-core/src/correlation.rs`.

`CorrelationContext` does not implement `Default`. Every call site
must explicitly choose `none()`, `correlated(id)`, or
`new(corr, cause)`. This forces callers to think about correlation
at every dispatch point — forgetting correlation is a conscious
omission, not an accidental default.

### Port trait signature changes

All port trait methods that produce `EventEnvelope`s gain a
`context: CorrelationContext` parameter: `EventStore::create`,
`EventStore::append`, `CommandBus::create`, `CommandBus::dispatch`,
`CommandGateway::create`, `CommandGateway::send`. See
`cherry-pit-core/src/lib.rs` for full trait definitions.

### Propagation flow

Propagation is linear: adapter calls `CommandGateway::send` with a
`CorrelationContext`, which passes it to `CommandBus::dispatch`, which
passes it to `EventStore::append`. The store stamps `correlation_id`
and `causation_id` from the context onto each `EventEnvelope`. After
publication via `EventBus`, a `Policy::react` handler reads the
envelope's `event_id` and `correlation_id` to construct a new
`CorrelationContext::new(envelope.correlation_id, envelope.event_id)`
for downstream commands, preserving the causal chain.

### Relationship to tracing

`correlation_id` and `tracing::Span` serve different purposes:

| Concern | `correlation_id` | `tracing::Span` |
|---------|------------------|-----------------|
| Scope | Cross-process, persisted in events | Process-local, ephemeral |
| Lifetime | Permanent (stored in event log) | Request duration |
| Purpose | Causal chain reconstruction | Diagnostic logging |

A future integration could set `correlation_id` as a `tracing` span
field for bridging, but this is not required by this ADR.

## Consequences

- **Breaking change** to `EventStore`, `CommandBus`, and `CommandGateway` trait signatures. All implementations and call sites must add `CorrelationContext`. Acceptable pre-1.0.
- Existing tests add `CorrelationContext::none()` at each call site — mechanical migration.
- `CorrelationContext::none()` is visible in code — a reviewer can see correlation was intentionally omitted.
- Policy implementations construct `CorrelationContext` for downstream commands. The framework provides types; the policy provides values.
- `EventBus::publish` does NOT gain a context parameter — it receives already-stamped envelopes.
