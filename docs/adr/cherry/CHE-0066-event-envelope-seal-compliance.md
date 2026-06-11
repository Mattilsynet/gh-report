# CHE-0066. EventEnvelope and AggregateId Seal Compliance

Date: 2026-05-20
Last-reviewed: 2026-05-20
Tier: B
Status: Accepted

## Related

References: CHE-0074, GEN-0035, GEN-0037, PAR-0024

## Context

GEN-0035:R7 mandates that `Encode` and `Decode` are sealed via a
private supertrait pattern, preventing downstream impls outside the
workspace derive macros. CHE-0071 supersedes the native event-store
encoding path; structural carriers still need a seal-compliant derive
story so adapter payloads and future native schemas cannot reopen the
hand-rolled encoding escape.

Closing the seal cleanly is preferred over admitting per-crate
exceptions: enumerated exceptions inflate the trait-coherence story,
require ADR amendments for each new structural carrier, and erode
the type-system guarantee that wire emission goes through the
blessed path. This ADR governs the seal-compliant derive path for
structural carriers while CHE-0071 governs the active adapter.

## Decision

R1 [5]: `cherry_pit_core::EventEnvelope<E>` derives `GenomeSafe`
  rather than hand-rolling `Encode`. The derive emits the conformant
  `Encode` and `Decode` impls per GEN-0037:R4.

R2 [5]: `cherry_pit_core::AggregateId` derives `GenomeSafe`. Any
  newtype wrapper structure preserved by the pre-amendment
  hand-rolled impl must be expressible via the derive; if not, the
  type is restructured.

R3 [5]: The `DomainEvent` payload bound tightens from
  `E: pardosa_encoding::Encode` to `E: pardosa_genome::GenomeSafe`.
  This is a strict tightening — every workspace `impl DomainEvent
  for X` already derives `GenomeSafe`. The tighter bound makes the
  derive composition on `EventEnvelope<E>` mechanical: `E:
  GenomeSafe` ⇒ `EventEnvelope<E>: GenomeSafe` via the derive.

R4 [5]: The pre-amendment hand-rolled `impl Encode for
  EventEnvelope<E>` and `impl Encode for AggregateId` are removed in
  the same commit as the derive additions. No transitional period:
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
