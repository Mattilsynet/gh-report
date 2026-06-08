# PAR-0026. pardosa-traits Public Surface Freeze for v0.1

Date: 2026-05-20
Last-reviewed: 2026-05-20
Tier: A
Status: Accepted

## Related

References: PAR-0024, GEN-0039, GEN-0040, GEN-0041, GEN-0043

## Context

`pardosa-traits` carries the substrate-agnostic event-invariant trait
substrate (PAR-0024): the sealing infrastructure, the `EventSafe`
marker, the `Validate` + `ValidationCost` pair, the `Timestamp`
newtype, and the trusted blanket impls for std types and foreign-floor
gates. Downstream crates and external consumers bind to this surface
as the entry point of the pardosa type stack. v0.1 is the
public-API-freeze milestone; the corpus needs an explicit statement of
what the surface is and what the additivity rule is, separate from the
ADRs that introduced each element piecemeal.

## Decision

R1 [5]: The frozen v0.1 public surface of `pardosa-traits` is:
  `sealed::Sealed`, `EventSafe`, `Validate`, `Validate::COST`,
  `Validate::Error`, `ValidationCost` (variants `Free`, `Cheap`,
  `Bounded { ops }`, `Unbounded`), `Timestamp`, `Timestamp::from_nanos`,
  `Timestamp::as_nanos`, and the re-export of `EventError` from
  `pardosa-encoding`. Trusted blanket impls covering std primitives,
  std containers, tuples up to arity 16, and the GEN-0041 / GEN-0043
  foreign-floor types behind their feature gates are part of the
  frozen surface.

R2 [5]: No event-stream type, no hash-chain type, no runtime concept
  may be added to `pardosa-traits`. The crate is the trait substrate
  only; stream and chain primitives live in `pardosa` and other
  substrate crates.

R3 [5]: The crate has zero non-workspace runtime dependencies on
  pardosa-foreign crates beyond `pardosa-encoding` (the `Encode`
  supertrait of `EventSafe`) and the optional foreign-floor crates
  (`uuid`, `bytes`, `arrayvec`, `jiff`) behind feature gates. New
  dependencies require an ADR amending this rule.

R4 [5]: Additivity is permitted under semver-additive rules: new
  sealed trait additions extending the stack, new blanket impls for
  additional std types, new foreign-floor feature gates, and new
  `ValidationCost` variants (the enum is `#[non_exhaustive]`). Removal
  or signature change of any frozen item requires a superseding ADR.

R5 [6]: The crate root docstring and the crate README cite this ADR
  as the canonical surface inventory. STORY § 6.2 cites this ADR
  rather than enumerating the surface inline.

## Consequences

+ becomes easier: downstream consumers reason against a single
  authoritative inventory; future audits compare source against this
  ADR rather than reconstructing the surface from history.

− becomes harder: adding a runtime concept to `pardosa-traits` (R2
  forbids it); the crate must stay a pure trait substrate.

risks/migration: the inventory was reconstructed from
`crates/pardosa-traits/src/lib.rs` at this ADR's date. Subsequent
additions must update both the source and a follow-up ADR amendment.
