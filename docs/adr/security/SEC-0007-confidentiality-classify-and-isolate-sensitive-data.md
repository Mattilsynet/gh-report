# SEC-0007. Confidentiality — Classify and Isolate Sensitive Data

Date: 2026-04-28
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: SEC-0001

## Context

Confidentiality ensures access to information is limited to
authorized entities. The primary CISQ threat is information
disclosure. In event-sourced systems, events are persisted
indefinitely and replayed across projections — any secret in an
event payload becomes permanently exposed to every consumer.
Secrets must be isolated from the event stream.

## Decision

Classify data sensitivity and prevent secrets from entering
persisted event streams.

R1 [5]: Secrets (credentials, tokens, private keys) never appear
  in event payloads, command payloads, or log output
R2 [5]: Secret values use opaque wrapper types that implement
  Debug without revealing content
R3 [5]: Configuration secrets are loaded from environment
  variables or secret stores, never from checked-in files
R4 [7]: Log output is reviewed for accidental secret leakage as
  part of code review

## Consequences

The event store contains no secrets, making replay safe and
reducing the blast radius of storage compromise. Opaque wrapper
types prevent accidental logging through the type system. The
trade-off is that secrets require separate management
infrastructure outside the event stream.
