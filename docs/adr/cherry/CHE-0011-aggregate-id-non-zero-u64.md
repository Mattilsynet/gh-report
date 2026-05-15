# CHE-0011. AggregateId as NonZeroU64 Newtype

Date: 2026-04-24
Last-reviewed: 2026-05-12
Tier: B
Status: Accepted

## Related

References: CHE-0002

## Context

Aggregates need identifiers. Options considered:

1. **UUID** — globally unique, no coordination needed. 128 bits, no
   Copy without Clone, serialization overhead.
2. **Plain `u64`** — simple, fast, Copy. But zero is constructible
   and never assigned by the store (IDs start from 1).
3. **`NonZeroU64` newtype** — same as `u64` but eliminates the zero
   hole at the type level. Niche optimization makes
   `Option<AggregateId>` the same size as `AggregateId`.

The original design used `u64`. During architectural review,
`AggregateId(0)` was identified as a latent invariant violation:
constructible via `AggregateId::new(0)` or `From<u64>` but never
assigned by the store.

## Decision

`AggregateId` wraps `NonZeroU64`. The constructor takes `NonZeroU64`
directly. Neither `From<u64>` nor `TryFrom<u64>` is provided: callers
presenting a raw `u64` (parsers, FFI, deserializers) must construct
`NonZeroU64` themselves at the integer-entry boundary. Serde routes
through `NonZeroU64`'s own validator on the wire, so zero is rejected
automatically on deserialize without an `AggregateId`-level bridge.

Store-assigned IDs auto-increment from 1 via `NonZeroU64`.

R1 [6]: AggregateId wraps NonZeroU64, eliminating zero as a valid
  identifier at the type level
R2 [6]: Do not expose From<u64> or TryFrom<u64>; require callers to
  construct NonZeroU64 at the integer-entry boundary so the zero-check
  lives at the source, not at the AggregateId seam

## Consequences

- Zero is no longer a valid aggregate ID at the type level.
- `Option<AggregateId>` benefits from niche optimization (same size as `AggregateId`).
- Copy semantics preserved — `NonZeroU64` is `Copy`.
- Serde deserializes as `u64` but rejects zero automatically.
- No `u64` → `AggregateId` trait impls — callers convert through `NonZeroU64::new` (or `NonZeroU64::try_from`) at the point a raw integer enters the system.
- IDs are not globally unique across aggregate types — cross-context references require domain-level external IDs.
