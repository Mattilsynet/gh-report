# GEN-0041. Foreign-Crate v0 Floor — Uuid, Bytes, ArrayVec

Date: 2026-05-15
Last-reviewed: 2026-05-15
Tier: A
Status: Accepted

## Related

References: GEN-0035, GEN-0036, GEN-0039

## Context

Event payloads beyond primitives, strings, and std containers need
three concrete foreign-crate types to participate in the in-house
canonical encoding (GEN-0035) without escaping the sealed-trait
substrate (GEN-0036): `uuid::Uuid` for identity, `bytes::Bytes` for
opaque byte payloads with cheap clone, and `arrayvec::ArrayVec<T, N>`
for capacity-bounded inline sequences.

GEN-0035 fixes the wire format; GEN-0036 fixes how types prove they
have been blessed. Neither names two complementary invariants the
foreign-floor impls must satisfy. **S1 — byte-shape conformance:**
every foreign-type encoding obeys GEN-0035's length-prefix and
fixed-width rules unchanged. **S2 — post-decode validity:** types
that carry a capacity in their type also reject a wire-supplied
length exceeding that capacity, surfacing the violation through the
frozen `EventError::InvalidInput` channel before any per-element
decode runs.

Opt-in types (chrono, time, rust\_decimal, NonEmpty collections) each
carry an ADR-shaped decision (timezone, decimal canonicalisation,
non-empty placement). Deferred to a successor ADR.

## Decision

Three foreign types join the sealed event-type stack at v0:

1. **`uuid::Uuid` — 16 verbatim bytes**, no length prefix. Encode
   emits `Uuid::as_bytes()`; decode reads a 16-byte fixed-width run
   and reconstructs via `Uuid::from_bytes`. Matches GEN-0035's
   fixed-size-array rule (S1).
2. **`bytes::Bytes` — `[len:u32 LE][bytes]`**, wire-identical to
   `Vec<u8>` / `&[u8]` / `[u8]`. The encoding cares about the byte
   payload, not the ownership flavour (S1).
3. **`arrayvec::ArrayVec<T, N> where T: EventSafe + Encode + Decode`
   — `[len:u32 LE]` followed by per-element encoding**. Decode reads
   the length, rejects with `EventError::InvalidInput` when `len > N`
   *before* any element decode, then fills the bounded vec by
   `try_push`. The capacity check is S2 in its narrow form: the
   decoder enforces a runtime invariant carried by the type itself.

Placement is split by the orphan rule. `Encode` and `Decode` impls
live in `pardosa-encoding` behind feature gates `uuid`, `bytes`,
`arrayvec`. The matching `sealed::Sealed` and `EventSafe` impls live
in `pardosa-traits` behind the same flag names, with the trait
crate's features pulling through to the encoding crate's. No new
crate is introduced; no `EventError` variants are added.

R1 [4]: `uuid::Uuid` encodes as 16 verbatim bytes with no length
  prefix; decode reads a 16-byte fixed-width run
R2 [4]: `bytes::Bytes` encodes as `[len:u32 LE][bytes]`, wire-
  identical to `Vec<u8>`/`[u8]`
R3 [4]: `arrayvec::ArrayVec<T, N>` encodes as `[len:u32 LE]` + per-
  element encode; decode rejects `len > N` with
  `EventError::InvalidInput` before any per-element decode runs (S2)
R4 [4]: foreign-floor impls live behind feature gates `uuid`,
  `bytes`, `arrayvec` in both `pardosa-encoding` and
  `pardosa-traits`, with the latter pulling through to the former
R5 [4]: foreign-floor impls introduce no new `EventError` variants
  and no new workspace crate
R6 [4]: opt-in foreign types (`chrono`, `time`, `rust_decimal`,
  non-empty collections) are out of scope for the v0 floor and
  belong to a successor ADR

## Consequences

- **Positive:** Three high-traffic event-payload types reach the
  sealed stack under a frozen error surface and a frozen wire spec.
  Sub-mission F's bounded wrappers can build on `ArrayVec` without
  reinventing the capacity check.
- **Positive:** Splitting impls across `pardosa-encoding`
  (`Encode`/`Decode`) and `pardosa-traits` (`Sealed`/`EventSafe`)
  matches each trait's defining crate, keeping every foreign impl
  orphan-rule-clean.
- **Positive:** Naming S1 and S2 here gives later foreign-type ADRs
  a vocabulary to point at when arguing byte-shape or post-decode
  validity, without amending GEN-0035.
- **Negative:** Two crates now carry symmetric feature flags; adding
  a fourth foreign type means touching both manifests. Acceptable
  cost for orphan-rule cleanliness.
- **Negative:** `ArrayVec`'s capacity is part of its type, so the S2
  check is per-monomorphisation. Capacity is a const generic; LLVM
  folds the comparison, but the check still appears in every
  generated decode site.
