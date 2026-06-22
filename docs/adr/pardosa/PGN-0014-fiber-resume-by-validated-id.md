# PGN-0014. Fiber Resume by Validated Identity

Date: 2026-06-11
Last-reviewed: 2026-06-11
Tier: B
Status: Accepted
Crates: pardosa

## Related

References: PGN-0011, PGN-0008, PGN-0002, PGN-0007

## Context

PGN-0011 R6 defers any writer primitive keyed on the identity contract to a follow-up ADR naming its partial-failure shape; this is that follow-up. Effects gap (GND-0001): after reopen, the dragline reconstructs each fiber's `Defined`/`Detached` state, but no public verb mints a writable handle from a rehydrated `FiberId` — so a daemon cannot continue a fiber across restart. PGN-0008 R4's payload-only writer verbs cover origination (`begin`/`append`/`detach`/`resume`), not re-attachment to a rehydrated id. A rehydrated id is dragline-minted (PGN-0002 R1), reconstructed from the journal, hence authoritative — not forged. This ADR admits two resume verbs whose only new input is a `FiberId` validated against the rehydrated state.

## Decision

This ADR amends PGN-0008 R4's mechanism clause to admit identity-resume verbs and constrains their structure; implementations may vary. `StoreWriter` gains `resume_defined(FiberId, T)` (source state `Defined`, grow-by-append) and `rescue_detached(FiberId, T)` (source state `Detached`, driving `(Detached,Rescue)→Defined`). Each admits the `FiberId` only after dragline validation against the rehydrated lookup, mirroring the existing `Defined`-filtered read predicate. The anti-forgery property of PGN-0008 R4 is preserved verbatim: a non-matching id cannot enter the substrate. Adopters discover the id via `FiberIndex<K>::lookup` (PGN-0011), not via a substrate-persisted key.

R1 [5]: The identity-resume verbs are `StoreWriter`-only and never nameable
  from `StoreReader`; the capability split stays type-level per PGN-0007 R6
  and PGN-0008 R3, so re-attachment confers no reader-side authority.
R2 [5]: A resume verb admits its `FiberId` only after validating it against
  the rehydrated dragline state; an absent or wrong-state id is rejected
  before any append, so a forged identity cannot enter the substrate.
R3 [5]: `resume_defined` requires source state `Defined`; `rescue_detached`
  requires source state `Detached`; any other source state is rejected, so
  only the state machine's appendable transitions are reachable by id.
R4 [5]: The partial-failure shape is typed and `#[non_exhaustive]` per
  PGN-0006: unknown or wrong-state id surfaces as `FiberNotFound`, a present
  but non-appendable state as an invalid-transition variant; never a silent
  no-op and never a freshly minted fiber.
R5 [5]: A resume verb takes exactly one already-resolved `FiberId`; against a
  `Diverged` `K` (PGN-0011 R4) the adopter selects the fiber by payload-side
  domain logic before calling, and the substrate offers no divergence opinion.
R6 [5]: PGN-0008 R4's payload-only clause is amended to cover origination
  verbs only; admitting a validated `FiberId` on a resume verb is a
  public-surface change under PGN-0012 semver governance, with author
  judgement as gate and the former record-file gate retired by PGN-0018.

## Consequences

+ becomes easier: a daemon continues one fiber per domain key across restart
  by pairing `FiberIndex::lookup` with a resume verb; re-attachment reuses the
  existing commit pipeline and the existing `Defined`-filtered predicate.
− becomes harder: bypassing validation — the verbs refuse any id the dragline
  did not rehydrate; treating a resume as fiber creation (the typed error
  forbids silent-mint fallback).
risks/migration: this widens one PGN-0008 R4 clause and adds public verbs;
  enforcement is a gated reopen-then-resume test asserting `FiberNotFound` on a
  fabricated id and a fresh `LiveFiber` on a rehydrated `Defined` id. No `.pgno`
  change, no `Lsn`/`FiberId` constructor widening, no schema-hash change.
