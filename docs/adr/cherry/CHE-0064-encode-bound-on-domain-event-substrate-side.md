# CHE-0064. Encode Bound on DomainEvent for Substrate-Side Hash Chaining

Date: 2026-05-18
Last-reviewed: 2026-05-18

Tier: B
Status: Accepted

## Related

References: CHE-0029, CHE-0060, PAR-0021, PAR-0024, CHE-0058

## Context

PAR-0021:R5 requires every persisted event to carry `precursor_hash =
BLAKE3(canonical encode of predecessor)`. The hash is produced
substrate-side inside `pardosa::Dragline::update/detach/rescue`, which
holds `Dragline<EventEnvelope<E>>` in `cherry-pit-pardosa`. Computing
the hash requires `EventEnvelope<E>: pardosa_encoding::Encode`, which in
turn requires `E: Encode` and `AggregateId: Encode`. CHE-0060:R2 placed
encoding locality at the trait-output boundary (frontier hashing in the
adapter); that scope does not address the substrate-internal bound the
writer needs. CHE-0029:R4 today admits only `serde`, `uuid`, `jiff` in
`cherry-pit-core`.

## Decision

R1 [5]: `cherry-pit-core` MAY depend on `pardosa-encoding` in addition
  to CHE-0029:R4's `serde`, `uuid`, `jiff` set; the CHE-0029:R6 closure
  check still excludes `tokio`, `axum`, `async-nats`, `tracing` and
  this rule does not relax it. `pardosa-encoding` is a leaf crate whose
  transitive closure must remain inside that exclusion to preserve the
  CHE-0029 de-scalability invariant.

R2 [5]: `cherry-pit-core::DomainEvent` carries `pardosa_encoding::Encode`
  as a supertrait; every workspace `impl DomainEvent for X` therefore
  requires a matching `impl Encode for X`. Per PAR-0024:R5 every Encode
  impl touched by this mission is hand-rolled — no `#[derive(Encode)]`.

R3 [5]: `cherry-pit-core` hosts `impl Encode for EventEnvelope<E>` and
  `impl Encode for AggregateId` as the type-owner. The impls are
  hand-rolled and match the field-by-field pattern in
  `crates/pardosa/src/event.rs` and `crates/pardosa-encoding/src/lib.rs`.

R4 [5]: `pardosa::Dragline`'s writer-side bound on `T` is tightened to
  `T: pardosa_encoding::Encode` on the methods that produce
  `precursor_hash` (`update`, `detach`, `rescue`), and the three writer
  sites currently emitting `[0u8; 32]` (dragline.rs L428, L483, L585
  at HEAD) compute `precursor_hash_of(&pardosa_encoding::to_vec(
  predecessor))` instead.

R5 [5]: `Dragline::verify_precursor_chains` extends its structural
  check by recomputing BLAKE3 of each predecessor's canonical encoding
  and asserting equality with the stored `precursor_hash`; mismatch
  returns a new `PardosaError::PrecursorHashMismatch { event_id,
  expected, actual }` variant.

## Consequences

+ becomes possible: PAR-0021:R5 substrate-level tamper-evidence
  end-to-end; CHE-0060 frontier-hash impls (separate mission) gain a
  canonical encoder they can reuse.

− becomes harder: every new `impl DomainEvent for X` in the workspace
  must ship a hand-rolled `impl Encode for X` alongside; the compiler
  surfaces omissions, but the cost is borne at every aggregate
  introduction.

risks/migration: writer-side `encode_to_vec` allocates per append.
Reclaim is Phase-3 follow-up; this ADR accepts the regression unless
it exceeds 50% wall-clock on existing proptests. CHE-0029:R6 closure
check is reasserted in this mission's verify pipeline; any transitive
breach by `pardosa-encoding` halts the work before commit.
