# COM-0023. Checked Arithmetic — Explicit Overflow Handling

Date: 2026-04-28
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: COM-0020

## Context

Integer overflow is CWE-190, a persistent source of security vulnerabilities (CVE-2002-0639, CVE-2018-14634, CVE-2023-42752). CERT rules INT30-C/INT32-C require validating every integer operation. Language designers disagree: Rust panics in debug but wraps in release (RFC 560), Swift traps unconditionally, Go silently wraps, Ada/SPARK can prove absence via formal verification. The pro-wrapping camp notes memory-safe languages produce wrong values, not memory corruption. Dan Luu measured checked arithmetic overhead at ~3%. Cherry-pit treats overflow as a bug in all profiles: counters use `checked_add()` with explicit error types, and release profiles keep `overflow-checks = true`.

## Decision

All integer arithmetic on counters, sequences, identifiers, and
sizes uses checked operations that surface overflow as a typed,
recoverable error rather than panicking or wrapping silently.

R1 [5]: Counter and sequence increments use checked_add or
  checked_next and return a typed error variant on overflow
R2 [5]: Release build profiles enable overflow-checks so arithmetic
  overflow is detected in production, not only during development
R3 [5]: Wrapping and saturating arithmetic require a documented
  justification in a code comment citing the domain guarantee
  that makes silent wrap or clamp correct
R4 [6]: Size calculations derived from external input use checked
  multiplication and addition before allocation to prevent
  overflow-driven undersized buffers

## Consequences

Overflow becomes a recoverable error rather than silent corruption or an unrecoverable panic. Event logs and identifiers never contain silently-wrapped nonsensical values. The cost is friction: every arithmetic operation on domain integers requires explicit error handling. Hot-path arithmetic on bounded-range values may use unchecked arithmetic with a documented justification. The ~3% performance overhead is acceptable under CHE-0001's priority ordering. This principle does not apply to hash functions or cryptographic operations where wrapping is intended.
