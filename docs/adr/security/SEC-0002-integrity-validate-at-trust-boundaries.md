# SEC-0002. Integrity — Validate at Trust Boundaries

Date: 2026-04-28
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: SEC-0001, GND-0005

## Context

Integrity ensures information remains correct and complete. The
primary CISQ threat is tampering — unauthorized or accidental
modification during storage, transmission, or deserialization.
Trust boundaries exist where data crosses crate, process, or
network edges. Validating at these boundaries prevents corrupted
or malicious data from propagating into domain logic where it
would be silently trusted.

## Decision

Validate all data at trust boundaries before use. Never trust
data that crossed a crate, process, or network boundary.

R1 [5]: All deserialization outputs pass structural validation
  before entering domain logic
R2 [5]: Type constructors enforce invariants; no public fields
  that allow bypassing validation
R3 [5]: Parse, don't validate — convert unvalidated types to
  validated domain types at the boundary
R4 [7]: Validation failures produce typed errors with enough
  context to diagnose the source of corruption

## Consequences

Corrupted data is caught at entry points rather than propagating
silently. Type constructors serve as the enforcement mechanism,
making invalid states unrepresentable after the boundary. The cost
is additional validation code at each trust boundary, offset by
eliminating defensive checks deeper in the call stack.
