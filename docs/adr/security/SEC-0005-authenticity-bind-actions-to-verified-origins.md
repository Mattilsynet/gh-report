# SEC-0005. Authenticity — Bind Actions to Verified Origins

Date: 2026-04-28
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: SEC-0001, GND-0005

## Context

Authenticity ensures information and behavior originate from their
purported source. The primary CISQ threat is spoofing —
impersonation of identities or forged artifacts. In an event-sourced
system, every event must be traceable to the command that produced
it and the identity that issued the command. Correlation and
causation chains provide this binding.

## Decision

Bind every action to a verified origin through correlation and
causation identifiers in event envelopes.

R1 [5]: Every event envelope carries a correlation_id linking it
  to the originating request or command
R2 [5]: Every event envelope carries a causation_id linking it
  to the immediate cause (preceding event or command)
R3 [5]: Identity of the command issuer is captured at the
  infrastructure boundary before entering domain logic
R4 [7]: Broken correlation chains (missing IDs) are detectable
  by infrastructure and produce warnings

## Consequences

Every event is traceable to its origin through the correlation and
causation chain. This enables audit trails, debugging, and
non-repudiation without additional infrastructure. The cost is
metadata per event, which is small relative to payload size.
Systems processing events can verify provenance without trusting
event content alone.
