# CHE-0066. EventEnvelope and AggregateId Seal Compliance

Date: 2026-05-20
Last-reviewed: 2026-05-20
Tier: B
Status: Accepted

## Related

References: CHE-0064, GEN-0035, GEN-0037, PAR-0024
Supersedes: CHE-0064:R2, CHE-0064:R3

## Context

GEN-0035:R7 mandates that `Encode` and `Decode` are sealed via a
private supertrait pattern, preventing downstream impls outside the
workspace derive macros. CHE-0064:R2 / R3 carved an exception:
`EventEnvelope<E>` and `AggregateId` were permitted hand-rolled
`Encode` impls because they did not derive `GenomeSafe` at the time
the ADR was written. The exception is incompatible with closing the
GEN-0035:R7 seal without enumerated workspace-internal escapes.

Closing the seal cleanly is preferred over admitting per-crate
exceptions: enumerated exceptions inflate the trait-coherence story,
require ADR amendments for each new structural carrier, and erode
the type-system guarantee that wire emission goes through the
blessed path.

## Decision

R1 [5]: `cherry_pit_core::EventEnvelope<E>` derives `GenomeSafe`
  rather than hand-rolling `Encode`. The derive emits the conformant
  `Encode` and `Decode` impls per GEN-0037:R4.

R2 [5]: `cherry_pit_core::AggregateId` derives `GenomeSafe`. Any
  newtype wrapper structure preserved by the current hand-rolled
  impl must be expressible via the derive; if not, the type is
  restructured.

R3 [5]: The `DomainEvent` payload bound tightens from
  `E: pardosa_encoding::Encode` to `E: pardosa_genome::GenomeSafe`.
  This is a strict tightening — every workspace `impl DomainEvent
  for X` already derives `GenomeSafe`. The tighter bound makes the
  derive composition on `EventEnvelope<E>` mechanical: `E:
  GenomeSafe` ⇒ `EventEnvelope<E>: GenomeSafe` via the derive.

R4 [5]: The hand-rolled `impl Encode for EventEnvelope<E>` and
  `impl Encode for AggregateId` (CHE-0064:R3) are removed in the
  same commit as the derive additions. No transitional period:
  the derives produce byte-identical output, verified by a
  golden-bytes regression test against the pre-amendment hand-rolled
  output.

## Consequences

+ becomes easier: GEN-0035:R7 closes without enumerated escapes;
  introducing future structural carriers no longer requires an ADR
  amendment per type; the trait-coherence story is one sentence.

− becomes harder: any future `EventEnvelope` field whose type does
  not derive `GenomeSafe` (e.g. a foreign type) blocks the
  derivation. Workaround is the existing newtype-and-derive pattern.

risks/migration: byte-shape preservation against the hand-rolled
output is the central risk. The golden-bytes test in
`crates/cherry-pit-core/tests/` is the tripwire. If the derive
produces a different byte shape, the amendment is non-conformant
and the implementation halts before commit.
