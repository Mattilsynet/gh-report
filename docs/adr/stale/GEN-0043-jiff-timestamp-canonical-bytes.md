# GEN-0043. `jiff::Timestamp` Canonical Bytes — Microsecond Truncation

Date: 2026-05-17
Last-reviewed: 2026-05-17
Tier: B
Status: Accepted

## Related

References: GEN-0041, GEN-0035, GEN-0039, PAR-0021, SEC-0011, CHE-0034

## Context

PAR-0021's per-fiber hash chain hashes `EventEnvelope<E>` canonical
bytes (PAR-0021:R3), and CHE-0034 mandates `jiff::Timestamp` as the
envelope's wall-clock field. The in-house canonical encoding
(GEN-0035) and the sealed-trait substrate (GEN-0036) together forbid
ad-hoc `Encode` impls outside `pardosa-encoding`; GEN-0041 ratified
the v0 foreign-floor (`uuid`, `bytes`, `arrayvec`) and deferred other
foreign time types via R6. `jiff` is not named in R6 but is excluded
by the same pattern: no sanctioned byte shape, no sealed-stack
participation. This ADR adds `jiff::Timestamp` to the sanctioned
foreign surface so PAR-0021's chain hashes a deterministic envelope.

## Decision

`jiff::Timestamp` encodes as `i64` little-endian of
`Timestamp::as_microsecond()`; decode reads 8 bytes LE and
reconstructs via `Timestamp::from_microsecond(i64)`. Sub-microsecond
precision is truncated at encode; the hash chain (PAR-0021:R5)
hashes canonical bytes, not in-memory `Timestamp` values, so the
truncation is the canonical wall-clock identity of the envelope.

`cherry-pit-pardosa/src/lib.rs:{472,490,497}` already converts
timestamps via `as_microsecond()` for substrate boundaries; this
ADR ratifies that precedent as the sole canonical shape.

Placement follows GEN-0041:R4 — `Encode`/`Decode` impls live in
`pardosa-encoding` behind feature gate `jiff`; matching
`sealed::Sealed` and `EventSafe` impls live in `pardosa-traits`
behind the same flag, with the trait crate's feature pulling
through to the encoding crate. No new `EventError` variants are
added. `from_microsecond` accepts the full `i64` domain, so no
in-range check is performed at decode — round-trip is total.

R1 [5]: `jiff::Timestamp` encodes as 8 bytes LE of
  `as_microsecond()`; decode reads 8 bytes LE and reconstructs via
  `from_microsecond`; the round-trip is total over `i64`
R2 [5]: sub-microsecond precision is truncated at encode and the
  truncated value is the canonical wall-clock identity hashed into
  PAR-0021's per-fiber chain (PAR-0021:R5)
R3 [5]: foreign-floor placement mirrors GEN-0041:R4 — impls behind
  feature gate `jiff` in both `pardosa-encoding` and `pardosa-traits`,
  with the latter pulling through to the former
R4 [5]: no new `EventError` variants and no new workspace crate; the
  decoder cap (GEN-0035:R8) does not apply because the encoding is
  fixed-width 8 bytes (no length prefix)
R5 [5]: GEN-0041:R6 is unchanged — `chrono`, `time`, `rust_decimal`,
  and non-empty collections remain out of v0 scope; this ADR adds
  `jiff` as a parallel sanctioned foreign type, not an amendment

## Consequences

- **Positive:** PAR-0021's chain hash is deterministic over envelope
  bytes without leaking `jiff`'s internal representation choice
  across substrate boundaries.
- **Positive:** Ratifies an existing in-tree precedent
  (`cherry-pit-pardosa` uses `as_microsecond()` for substrate work);
  no migration of live code beyond consolidation.
- **Negative:** Sub-microsecond precision is unrecoverable from the
  canonical bytes. Producers needing nanosecond fidelity must carry
  it in a separate payload field, not in the envelope timestamp.
- **Negative:** A future ADR widening time fidelity (i128 nanoseconds,
  TAI, etc.) must supersede R1 and R2 — wire-incompatible.
