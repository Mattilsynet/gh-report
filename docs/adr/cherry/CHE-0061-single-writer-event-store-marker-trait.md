# CHE-0061. SingleWriterEventStore Marker Trait

Date: 2026-05-16
Last-reviewed: 2026-06-10
Tier: B
Status: Accepted

## Related

References: CHE-0057, CHE-0006, PGN-0010

## Context

CHE-0006 asserts single-writer-per-aggregate as a cherry-pit-wide
architectural assumption but does not make it observable at the type
system. PGN-0010:R6 supplies the live single-writer stance for pardosa
authoritative-storage instances. Downstream code needs to observe single-
writer at type level for audit-trail and idempotency-derivation
purposes (CHE-0033:R1, CHE-0040 idempotency keys). A zero-method
marker trait makes the property a type bound without imposing any
method surface; substrates that guarantee single-writer implement it.

## Decision

R1 [5]: SingleWriterEventStore extends EventStore as a supertrait bound
  per CHE-0057:R2 and lives in cherry-pit-core; standalone definition
  is forbidden.

R2 [5]: SingleWriterEventStore is a zero-method marker trait; the
  assertion carried is purely the substrate-level guarantee of one
  logical writer per aggregate stream, e.g. PGN-0010:R6.

R3 [5]: PardosaEventStore MUST implement SingleWriterEventStore on
  the strength of PGN-0010:R6's single-writer backend stance; cherry-pit-storage
  MAY implement it where CHE-0006 single-writer is enforced by the
  file-per-stream layout (CHE-0036) and run-lock (CHE-0043).

R4 [5]: Downstream code requiring single-writer guarantees MUST bound
  on SingleWriterEventStore per CHE-0057:R4 rather than asserting it
  out-of-band; idempotency-key derivation per CHE-0040 is the named
  consumer.

R5 [5]: Adding methods to SingleWriterEventStore is forbidden under
  CHE-0057:R5 append-only signatures and would re-classify the trait
  away from marker shape; any method addition requires a superseding
  ADR explicitly redefining the trait's role.

## Consequences

Single-writer becomes a type-level observable property without
amending CHE-0006 (still substrate-agnostic) or CHE-0023 (still
forbids framework lifecycle traits). The SMI audit-trail signal is
preserved at zero amendment cost. The marker has no runtime cost.
Method-addition pressure is foreclosed by R5; if multi-writer
substrate arrives the marker simply remains unimplemented for that
substrate.
