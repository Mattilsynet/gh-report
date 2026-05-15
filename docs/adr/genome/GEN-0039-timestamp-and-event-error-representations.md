# GEN-0039. Timestamp and EventError Representations

Date: 2026-05-15
Last-reviewed: 2026-05-15
Tier: A
Status: Accepted

## Related

References: GEN-0013, GEN-0035, GEN-0036

## Context

The v2 typing refresh introduces two pub types in `pardosa-traits`
named by F's bounded wrappers and downstream events: a canonical
timestamp newtype and a canonical error enum.

**Timestamp.** Raw `u64` exposes the bit layout and offers no niche
for `Option`. Sub-nanosecond granularity is exotic; coarser
granularity loses ordering for interleaved high-frequency events and
is asymmetric with system-clock APIs. Signed time admits "before
epoch" semantics events do not need.

**EventError.** Package contract v2.1 §4.5 fixes the variant set
and ordering: 11 caller-relevant failure classes. Discriminant
encoding: varint `u32 LE` versus fixed `repr(u8)`. `u8` is the
smallest representation covering the design space (growth-friendly
to 255). Pinning literal discriminants 0..=10 makes byte-1 of the
encoded form trivially equal the discriminant — a property F's
diagnostic code relies on without reading the encoder.

## Decision

### Timestamp

`Timestamp` is a newtype around `core::num::NonZeroU64` representing
**non-zero epoch nanoseconds** (UNIX epoch by default — callers
choosing a different epoch document that locally; the type itself is
epoch-agnostic).

```rust
pub struct Timestamp(NonZeroU64);
impl Timestamp {
    pub const fn from_nanos(nanos: u64) -> Option<Self> { /* … */ }
    pub const fn as_nanos(self) -> u64 { /* … */ }
}
```

Three properties earn their keep:

1. **`NonZeroU64` niche.** `Option<Timestamp>` is the same size as
   `Timestamp` — zero is reserved as the `None` sentinel at the
   option layer rather than wasting a representable value inside the
   newtype.
2. **No `Default` impl.** "Zero time" is meaningless for an event;
   the absence of `Default` prevents accidental construction of a
   sentinel timestamp that compares equal across unrelated events.
3. **Nanosecond granularity.** ~584 years of unsigned range from any
   chosen epoch; smallest resolution that survives high-frequency
   event interleaving.

### EventError

`EventError` is a `repr(u8)` enum with 11 variants and literal
discriminants pinned 0..=10:

| Discriminant | Variant |
|---|---|
| 0 | `InvalidInput` |
| 1 | `NotFound` |
| 2 | `Conflict` |
| 3 | `Unauthorized` |
| 4 | `PermissionDenied` |
| 5 | `Unavailable` |
| 6 | `Timeout` |
| 7 | `Internal` |
| 8 | `ResourceExhausted` |
| 9 | `Cancelled` |
| 10 | `DataLoss` |

The in-house canonical encoding (GEN-0035) emits the discriminant
byte as the entire payload for these unit-like variants: an encoded
`EventError` is exactly one byte and byte-1 equals the discriminant
value. The `Encode` impl in `pardosa-traits` pushes
`self.discriminant()` to the output buffer.

Variant ordering and discriminant values are part of the wire
contract. Appending new variants at discriminant 11+ is a
forward-compatible (Tier-A) revision. Renumbering existing variants
is a breaking change requiring a superseding ADR.

`#[non_exhaustive]` is applied so external crates cannot exhaustively
match on `EventError` — variant growth at discriminant 11+ remains
non-breaking for callers.

R1 [4]: `Timestamp` is `NonZeroU64` epoch nanoseconds, no `Default`
  impl, and `Option<Timestamp>` size-equal to `Timestamp` via the
  niche
R2 [4]: `EventError` is `repr(u8)` with 11 variants and literal
  discriminants pinned 0..=10 per the table above
R3 [4]: The in-house canonical encoding of any `EventError` variant
  is exactly one byte equal to the variant's pinned discriminant
R4 [4]: Variant renumbering is a breaking change requiring a
  superseding ADR; appending variants at discriminant 11+ is
  forward-compatible

## Consequences

- **Positive:** `Timestamp`'s niche gives free `Option` packing for
  every event type that carries an optional timestamp.
- **Positive:** `EventError`'s single-byte encoding makes error
  variants cheap to log, sample, and aggregate without a decoding
  pass.
- **Positive:** Both types live in `pardosa-traits` — substrate
  crate with zero external deps — so foreign-crate impls (E
  sub-mission) and bounded wrappers (F) can name them without
  pulling in `pardosa-genome`.
- **Negative:** `NonZeroU64` forces every `Timestamp::from_nanos`
  call site through an `Option` unwrap or pattern-match. The
  ergonomic cost is small relative to the niche-optimisation
  benefit and the safety of unrepresentable-zero invariant.
- **Negative:** The pinned discriminant table is a wire contract:
  any future reordering breaks every consumer that has serialized
  the old layout to durable storage. Renumbering is therefore not
  a refactor — it is a versioning event.
