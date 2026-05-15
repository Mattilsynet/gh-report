# SEC-0008. Non-Repudiation via Append-Only Event Log

Date: 2026-04-28
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: SEC-0001, GND-0007

## Context

Non-repudiation ensures actions cannot be denied after the fact.
The CISQ model composes this from Integrity, Control, and
Authenticity. In event-sourced systems, the event log is the
natural audit trail — but only if events are immutable, ordered,
and traceable to their origins. Any mutation or deletion of events
destroys non-repudiation guarantees.

## Decision

Preserve non-repudiation through immutable, ordered, traceable
event logs.

R1 [5]: Events are append-only; no update or delete operations
  exist on persisted events
R2 [5]: Event ordering is guaranteed by monotonic sequence
  numbers assigned at write time
R3 [5]: Each event carries correlation and causation metadata
  binding it to the originating action
R4 [5]: Compensating events are used to logically reverse
  business outcomes without mutating history

## Consequences

The event log serves as a complete audit trail with ordering
guarantees. Regulatory and forensic requirements are met by
design. Business reversals use compensating events rather than
destructive updates. The trade-off is that storage grows
monotonically and sensitive data requires careful handling per
SEC-0007.
