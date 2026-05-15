# PAR-0018. Reserve/Commit API Discipline

Date: 2026-04-29
Last-reviewed: 2026-04-29
Tier: A
Status: Accepted

## Related

References: PAR-0008, PAR-0007, PAR-0017, GND-0001

## Context

PAR-0008 mandates publish-then-apply: durable acknowledgement precedes
in-memory mutation. The current `Dragline::create/update/detach` API
mutates state and returns an `AppendResult` in a single call. Once
JetStream is in the path, broker NACK forces a rollback that the API
shape does not support — the in-memory state has already advanced.

Splitting the API into a pure validation/allocation phase (`reserve`)
and a state-mutation phase (`commit`) makes PAR-0008 enforceable at the
type level. A `ReservedEvent<T>` value carries the allocated event_id,
fiber-state delta, and encoded genome bytes between the two phases. Its
`Drop` impl signals a reservation that escaped without commit or
abandon — the most common bug in this pattern.

## Decision

Replace the single-call mutation API with a three-method discipline.
`reserve` validates and allocates without mutating Dragline state.
`commit` accepts a ReservedEvent plus a broker sequence and applies the
delta. `abandon` releases a reservation on broker NACK. ReservedEvent is
non-Clone and its Drop impl logs an assertion failure.

R1 [5]: Expose Dragline::reserve as a non-mutating method that returns
  a ReservedEvent carrying event_id, fiber delta, and encoded genome
  bytes
R2 [5]: Apply the fiber delta to Dragline state only inside
  Dragline::commit after the broker sequence is bound to the
  ReservedEvent
R3 [5]: Require Dragline::abandon to release a ReservedEvent without
  mutating fiber state when the broker rejects the publish
R4 [5]: Define ReservedEvent without a Clone implementation and emit a
  drop-without-commit assertion from its Drop method
R5 [11]: Bound the in-flight reservation table by max_inflight_writes
  and reject reserve calls that would exceed it

## Consequences

PAR-0008 is enforceable at compile time — there is no API path that
mutates state before durable ack. Broker NACK has a clean rollback path.
The reservation table provides a natural backpressure point per PAR-0014.
Trade-offs: callers see two-phase API surface; the bus driver
(PAR-0017) is responsible for keeping reserve and commit close enough
in time that the reservation table stays bounded. Tests must exercise
the abandon path explicitly to catch leaks.
