# PGN-0003. Canonical Encoding, Schema Hash, and EventSafe Bounds

Date: 2026-06-08
Last-reviewed: 2026-06-08
Tier: A
Status: Accepted
Crates: pardosa-wire, pardosa-derive, pardosa-schema

## Related

References: PGN-0001, PGN-0002

## Context

Sources rescue ADR-0005 (canonical encoding contract), rescue ADR-0020 (`EventSafe` decoupled from codec traits), and rescue ADR-0014 (sealed-trait closure table — the `EventSafe` row). Compatible Solon material: GEN-0035's wire-shape definition (primitive endianness, length-prefix shape, enum discriminant byte, schema hash derivation). Where GEN-0035 R7 seals `Encode`/`Decode` and makes them supertraits of `EventSafe`, **rescue ADR-0014 and ADR-0020 take precedence** — `Encode` and `Decode` are Open codec traits with no sealing edge and are not supertraits of `EventSafe`. Sealing axis and codec axis stay separate; sealing is Pardosa's distinguishing substrate guarantee, codec follows the host ecosystem idiom (serde, bincode, borsh, rkyv).

## Decision

`Encode` and `Decode` produce a canonical, deterministic byte form. `pardosa-derive` is the blessed source of derives for application types. `SCHEMA_HASH` is derived from the type's structural shape and changes whenever wire bytes change. `EventSafe`'s supertrait set is **`sealed::Sealed` only**; `GenomeSafe: EventSafe: Sealed`. Per-impl codec bounds are body-scoped: writer paths use `T: Encode + GenomeSafe`, reader and cursor paths use `T: Decode + GenomeSafe`. Decode-only and encode-only payload types are structurally permitted.

R1 [4]: `Encode` and `Decode` produce a canonical, platform-deterministic
  byte form; non-determinism (`HashMap` iteration, time-dependent bytes,
  implementation-defined gaps) is a bug.
R2 [5]: `EventSafe`'s sole supertrait is `pardosa_wire::sealed::Sealed`;
  `Encode` and `Decode` are not supertraits of `EventSafe` and not
  supertraits of `GenomeSafe`.
R3 [5]: Public façade codec bounds are body-scoped — writer paths require
  `T: Encode + GenomeSafe`, reader and cursor paths require
  `T: Decode + GenomeSafe`; no fused `Encode + Decode` bound at the trait
  edge.
R4 [4]: `SCHEMA_HASH` (u128) is recomputed by any change that affects wire
  bytes — adding, reordering, removing fields, or changing a field type —
  and is the load-bearing gate at `Reader::open` before any payload decode.
R5 [4]: `pardosa-derive` is the only blessed source of `Encode`/`Decode`
  impls for application types; in-tree hand impls are permitted for
  substrate primitives and must round-trip via `proptest`.

## Consequences

+ becomes easier: cursor and reader paths drop the vacuous `Encode` clause;
  adopters write decode-only frozen historical types and encode-only sinks;
  trait-graph reading matches the host ecosystem.
− becomes harder: a manual `impl GenomeSafe for X` must now ship `Encode`
  and/or `Decode` explicitly when `X` is used on the corresponding path —
  no transitive supertrait carries it.
risks/migration: GEN-0035 R7 (`Encode`/`Decode` sealed and extending
  `EventSafe`) is superseded for the PGN domain by rescue ADR-0014 row
  `EventSafe` and rescue ADR-0020 D1; GEN-0035's wire-byte definitions
  (endianness, length prefix shape, discriminant byte, schema-bytes
  encoding) remain compatible inputs to this ADR. GEN-0035 retirement is
  deferred to a follow-up mission.
