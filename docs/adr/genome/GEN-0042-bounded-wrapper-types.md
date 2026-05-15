# GEN-0042. Bounded Wrapper Types — EventString, EventBytes, EventVec, NonEmptyEventString

Date: 2026-05-15
Last-reviewed: 2026-05-15
Tier: A
Status: Accepted

## Related

References: GEN-0035, GEN-0036, GEN-0040, GEN-0041

## Context

The in-house canonical encoding (GEN-0035:R8) ships a workspace-wide
decoder cap (default 1 MiB) that rejects length-prefix headers
exceeding the per-decode budget before allocation. The cap is coarse
by design: it bounds peak decoder allocation without forcing every
field to declare a limit. Individual event fields legitimately want
a *tighter* per-field cap — a 16-byte username field has no business
reading a 1 MiB payload, and an adversarial header could otherwise
grow allocation right up to the substrate cap on a small-typed field.

GEN-0040 ships the `Validate` trait; GEN-0041 names S1 (byte-shape
conformance) and S2 (post-decode validity). What is missing is a
sealed set of *wrapper* types attaching per-field MAX as a
const-generic, enforced at both decode and construction.

Four wrappers cover the observed shape of event fields: UTF-8 string
with byte cap; opaque bytes with byte cap; sequence of `T` with
element-count cap; UTF-8 string with both a non-empty floor and a
byte cap. The wrappers express *invariants only* — wire format is
byte-identical to the inner type so a `String` field upgrading to
`EventString<MAX>` does not break the wire. Test #9
(`event_string_wire_compat_with_string`) locks the invariant.

The contract source occasionally refers to "`EventError::Invalid`";
the canonical post-C2 variant is `EventError::InvalidInput`. No new
error variants are introduced.

## Decision

Four bounded wrapper types ship in `pardosa-genome` (module
`bounded`), each carrying a single const-generic `MAX` (and, for
`EventVec`, an element type `T`). Every wrapper implements the full
event trait stack: `sealed::Sealed`, `EventSafe`, `Encode`, `Decode`,
and `Validate`. None implement `From` from their inner type —
construction is fallible (`TryFrom`, or `try_new` for
`NonEmptyEventString`). None implement `DerefMut` — post-construction
mutation could violate the invariant.

The cap mechanism is **not duplicated** here. `read_len_prefix`
charges the decoder cap per GEN-0035:R8; the wrapper decoders apply a
*tighter* per-field check (`n > MAX`) immediately after the cap
charge, before any payload allocation. This is the S3 invariant
(naming continues the GEN-0041 sequence): per-wrapper MAX enforcement
layered on top of the substrate cap.

S3 has two arms, applied at distinct phases:

- **Decode arm.** `Decode::decode` reads the length prefix
  (substrate cap charge), then rejects `n > MAX` with
  `EventError::InvalidInput` before `read_bytes` / element-loop runs.
  `NonEmptyEventString` additionally rejects `n == 0` at the same
  point. This is the defence against adversarial wire — an attacker
  cannot induce allocation up to the substrate cap on a small-MAX
  field.
- **Validate arm.** `Validate::validate` re-checks the same invariant
  on a constructed value. This covers values built via `TryFrom` /
  `try_new` (where the check is in the constructor and could in
  principle be bypassed by an internal mutation path that does not
  exist today but might in future revisions). The re-check is the
  belt-and-braces guarantee Tier-A wrappers want.

`EventBytes` carries `Vec<u8>` as its inner type, not `bytes::Bytes`.
The `bytes` crate is feature-gated in `pardosa-encoding` per GEN-0041
and `pardosa-genome` does not enable that feature. Using `Vec<u8>`
keeps the F sub-mission dependency-clean; a future
`EventBytesShared<MAX>` over `bytes::Bytes` is a feature-gated
addition, not a redesign.

Degenerate const-generic instantiations are permitted at compile time
and rejected at runtime. `EventString<0>` admits only the empty
string; `NonEmptyEventString<0>` is uninhabitable — every
construction path returns `Err`. No `const` assertion is added; the
runtime checks suffice and a compile-time assertion would force every
generic call site to monomorphise with a non-zero MAX.

R1 [4]: each bounded wrapper's `Decode` impl rejects a length-prefix
  header exceeding `MAX` with `EventError::InvalidInput` before any
  per-element decode or payload `read_bytes` runs (S3 decode arm)
R2 [4]: each bounded wrapper's `Validate::validate` re-checks the
  same `len <= MAX` invariant on a constructed value (S3 validate
  arm), covering `TryFrom`-built instances
R3 [4]: `NonEmptyEventString<MAX>` additionally rejects `len == 0`
  at both the decode arm (before payload read) and the validate arm
  (on constructed values)
R4 [4]: the wire format of each bounded wrapper is byte-identical
  to that of its inner type (`String`, `Vec<u8>`, `Vec<T>`) — the
  wrappers express invariants, not a distinct encoding (PM4)
R5 [4]: no bounded wrapper implements `From` from its inner type
  (construction is fallible via `TryFrom` or `try_new`); none
  implement `DerefMut` (post-construction mutation could violate
  the invariant)
R6 [4]: `EventBytes<MAX>` inner type is `Vec<u8>`; a future
  `bytes::Bytes`-backed wrapper is feature-gated and out of scope
  for v0
R7 [4]: bounded wrappers introduce no new `EventError` variants and
  no new workspace crate; the cap mechanism is inherited from
  GEN-0035:R8 and not duplicated

## Consequences

- **Positive:** Per-field MAX rejection at decode defeats small-typed
  allocation amplification — a 16-byte field rejects a 32-byte
  header before allocating the buffer.
- **Positive:** Reusing the substrate `read_len_prefix` cap charge
  keeps the wrappers thin: a one-line `if n > MAX` after the
  substrate charge, no per-wrapper budget threading.
- **Positive:** Wire-identical encoding (R4) means a producer can
  upgrade a `String` field to `EventString<MAX>` without a
  wire-format break (assuming inner contents already fit).
- **Positive:** Sealing via `pardosa_traits::sealed::Sealed`
  (`pub` post-GEN-0036 refactor) lets the wrappers live in
  `pardosa-genome` while still participating in the sealed stack —
  the orphan rule is satisfied because the wrapper types are
  defined here.
- **Negative:** Const-generic instantiations are per-monomorphisation;
  using `EventString<16>`, `EventString<32>`, `EventString<256>`
  generates three copies of each method. LLVM inlines the bound and
  folds the comparison; overhead is marginal.
- **Negative:** Construction-time and validate-time checks are
  belt-and-braces redundancy in the current API (no in-place
  mutation exists). Sub-mission G may add `ValidationCost` to mark
  the validate arm as cheap-and-cached; this ADR does not prescribe
  that optimisation.
- **Negative:** Degenerate `NonEmptyEventString<0>` compiles but
  cannot be constructed. Runtime check is fine for end-user data;
  call sites generating MAX from a const expression should guard
  against `MAX == 0` if construction is on a hot path.
