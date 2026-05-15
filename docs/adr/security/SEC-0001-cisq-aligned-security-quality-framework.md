# SEC-0001. CISQ-Aligned Security Quality Framework

Date: 2026-04-28
Last-reviewed: 2026-04-28
Tier: S
Status: Accepted

## Related

Root: SEC-0001

## Context

Security decisions scattered across domain-specific ADRs lack a
unifying vocabulary. The CISQ model decomposes security into four
primary qualities (Availability, Integrity, Control, Authenticity)
and their compositions. Adopting this vocabulary as a foundation
domain ensures every crate inherits baseline security rules through
the `--context` resolution chain, preventing gaps and overlaps in
security coverage.

## Decision

Establish a SEC foundation domain using the CISQ four-quality
decomposition as the MECE partition for all security ADRs.

R1 [1]: Every security ADR maps to one or more CISQ primary
  qualities (Availability, Integrity, Control, Authenticity)
R2 [3]: Security rules are foundation-level and apply to all
  crates via `--context` resolution
R3 [3]: Each CISQ primary quality has exactly one SEC ADR;
  composed qualities reference their constituent primaries

## Consequences

Security vocabulary is consistent across all domains. The CISQ
decomposition prevents gaps (collectively exhaustive) and overlaps
(mutually exclusive). Domain-specific security ADRs in CHE, GEN,
and PAR reference SEC primaries for traceability, creating a
two-layer security architecture: universal principles in SEC,
domain-specific implementations in their respective domains.
