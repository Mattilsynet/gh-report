# PGN-0013. Payload-Type Vocabulary

Date: 2026-06-08
Last-reviewed: 2026-06-15 — refined — amended R1/R8 for closed bounded-field-type algebra and Box/Arc removal (mission:adr-fmt-50naw)
Tier: B
Status: Accepted
Crates: pardosa-wire, pardosa-schema

## Related

References: PGN-0003, PGN-0006, PGN-0001, AFM-0029

## Context

This ADR is the PGN payload-type vocabulary — the bounded-wrapper alphabet adopters compose to express per-field invariants on top of PGN-0003's canonical wire format. Vocabulary, impl set, and durability-cap policy are separate concerns: the closed in-tree foreign-type impl inventory and codec fuzz matrix live with PGN-0003 R5; the per-message decompressed cap and decompression-bomb ordering live with PGN-0004 R2. This ADR therefore lists vocabulary types and invariants only; it does not enumerate foreign impls, cap defaults, or fuzz targets, and admits no new failure-surface variant.

The 2026-06-15 amendment follows AFM-0029's amend-in-place path: R8's prior allowance for `Box<T>` was narrower than the now-explicit closure rule, while the rest of this ADR remains in force. The payload vocabulary is a closed algebra under bounded field types, not under field count, variant count, or Rust layout pressure. A named struct or enum is field-eligible when each of its fields is field-eligible recursively; a large enum of bounded-field variants is the idiomatic illegal-states-unrepresentable shape. A genuine multi-large-variant layout problem is answered by a second dragline with its own `ENVELOPE_HASH`, not by hiding heap indirection inside one event schema.

## Decision

The PGN bounded-wrapper alphabet is `EventString<MAX>`, `EventBytes<MAX>`, `EventVec<T, MAX>`, and `NonEmptyEventString<MAX>`, shipped from `pardosa-schema::bounded`. Each wrapper is a `Vec`- or `String`-backed newtype carrying a per-field `MAX` invariant enforced at construction, on every decode, and through `Validate::validate`. Wire shape is identical to the unwrapped inner — the wrapper is invariant-bearing, not wire-affecting. Every wrapper ships the full sealed-trait stack (`Sealed`, `EventSafe`, `Encode`, `Decode`, `Validate`); none exposes a `From<inner>` or `DerefMut`-shaped escape hatch. The wrapper alphabet is the sole vocabulary primitive PGN adopters reach for to express bounded fields — opaque-byte payloads with cheap clone, inline-storage capacity, or other non-invariant ergonomics belong to the codec-trait impl surface (PGN-0003 R5), not to this vocabulary. Generic ownership and indirection wrappers such as `Box<T>`, `Arc<T>`, `Rc<T>`, and `Cow<'_, T>` are outside the alphabet because they do not add a bounded field invariant.

R1 [5]: The PGN bounded-wrapper alphabet is exactly
  `EventString<MAX>`, `EventBytes<MAX>`, `EventVec<T, MAX>`, and
  `NonEmptyEventString<MAX>`, all shipped from `pardosa-schema::bounded`;
  no other wrapper joins the alphabet without a successor PGN ADR.
  Generic ownership or indirection wrappers (`Box<T>`, `Arc<T>`,
  `Rc<T>`, `Cow<'_, T>`) are not alphabet members; layout transparency
  does not grant `Sealed`, `EventSafe`, or `GenomeSafe` eligibility.
R2 [5]: Each wrapper ships the full sealed-trait stack (`Sealed`,
  `EventSafe`, `Encode`, `Decode`, `Validate`); a wrapper missing any
  of the five is malformed vocabulary and rejected at review.
R3 [5]: Each wrapper enforces its `MAX` invariant at construction
  (`try_new` / `TryFrom`), on every decode (`Decode::decode` rejects
  `len > MAX` before per-element decode), and through
  `Validate::validate`; the three enforcement sites are non-optional.
R4 [5]: The wrapper wire shape is identical to its unwrapped inner —
  `EventString<MAX>` matches `String`, `EventBytes<MAX>` matches
  `Vec<u8>`, `EventVec<T, MAX>` matches `Vec<T>` — so the wrapper is
  invariant-bearing only and does not perturb `SCHEMA_HASH`
  contributions beyond the inner's shape.
R5 [5]: No wrapper exposes a `From<inner>`, `DerefMut`, or other
  invariant-bypassing surface; the construction path is the sole
  authority for `MAX` enforcement.
R6 [5]: `MAX` is a `usize` const-generic and is part of the type;
  two wrappers with different `MAX` values are different types and
  do not coerce. Schema-hash visibility is provided by the inner's
  contribution per PGN-0003 R4.
R7 [5]: No bounded wrapper introduces a new `EventError` /
  `PardosaError` variant; capacity violations surface through
  `EventError::InvalidInput` (the taxonomy frozen by PGN-0006 is the
  sole failure surface).
R8 [5]: Event fields must be bounded field types: primitive booleans
  and fixed-width integers, `char`, `Timestamp`, the PGN-0003 foreign
  type inventory, `Option<T>`, `[T; N]`, tuples up to arity 16,
  bounded wrappers from R1, or a `GenomeSafe` struct or enum whose
  fields each satisfy this rule recursively. Structs and enums are
  transparent bounded combinators: a struct is bounded by the sum of
  its fields' bounds, and an enum is bounded by the maximum of its
  variants' bounds. Field count and variant count are not eligibility
  criteria. Zero-width types (`()`, `PhantomData<T>`) are not
  `GenomeSafe` and are not field-eligible because `()` carries no data
  and `PhantomData<T>` erases `T` from `SCHEMA_HASH`. Heap and shared
  ownership wrappers (`Box<T>`, `Arc<T>`, `Rc<T>`, `Cow<'_, T>`) are not
  field-eligible; if layout pressure comes from multiple large bounded
  variants, split the model into a second dragline instead of adding an
  indirection wrapper inside one schema.

## Consequences

+ becomes easier: locating the PGN payload-type vocabulary in one
  ADR; reading the wrapper alphabet without chasing impl-inventory
  or cap-policy concerns; adding a per-field bound to a payload by
  picking a wrapper rather than inventing one; recognising a large
  bounded enum as the blessed event idiom rather than a count problem.
− becomes harder: shipping a fifth bounded wrapper (requires a
  successor PGN ADR); adding a non-invariant-bearing variant (the
  alphabet exists to carry `MAX`, not ergonomic ownership flavours);
  using heap indirection to mask layout pressure inside one event
  schema.
risks/migration: opaque-byte payloads with cheap-clone ergonomics
  and inline-storage bounded sequences are codec-impl decisions
  under PGN-0003 R5 and do not enter this vocabulary even when they
  overlap with `EventBytes`/`EventVec` at wire shape. Default
  decoded-payload cap and decompression-bomb ordering are owned by
  PGN-0004 R2 and are not restated here. Removing `Box<T>` and
  `Arc<T>` eligibility narrows the public type vocabulary under
  PGN-0012 semver governance; PGN-0018 retires the former record-file
  gate. The change is wire- and schema-hash-neutral for existing
  unboxed shapes.
