//! Domain-shape validation for substrate vocabulary.
//!
//! * [`Validate`] — open trait (ADR-0014).
//! * [`ValidationCost`] — advisory cost hint.
//!
//! # Contract
//!
//! Sole call site: `ValidatedEventStream` /
//! `stream_validated`. Validates every event regardless of
//! `COST`.
//!
//! 1. `Free` / `Cheap` (default): constant cost; in-tree
//!    impls are constructor-validated.
//! 2. `Bounded` / `Unbounded`: still validated per event;
//!    cost is informational. Route through `stream_checked`
//!    for volume, or amortise at construction.
//! 3. `COST` is `const`; advertise worst case.
//! 4. Open trait; future gating would be additive
//!    (ADR-0009-minor).
//!
//! Mission `rescue-pardosa-4kd3`.
use crate::{Decode, DecodeError, Decoder, Encode, EventSafe};
use alloc::vec::Vec;
use core::num::NonZeroU64;
/// Nonzero-u64 nanoseconds since UNIX epoch — **payload
/// vocabulary**.
///
/// `Timestamp` lives in `pardosa-wire` so adopter payloads
/// embedding a timestamp get a byte-equivalent `EventSafe`
/// field with stable `GenomeSafe` schema-hash contribution.
/// Substrate never reads it — wall-clock ordering is not a
/// substrate concern (ADR-0013); monotonic `event_id` is the
/// ordering primitive.
///
/// Wire form = inner `NonZeroU64`: 8 LE bytes, zero rejected
/// at `Decode`. `NonZero` is a structural domain invariant.
///
/// External clocks: route their `as_nanosecond()` through
/// `Timestamp::from_nanos`. Pinned by
/// `tests/h4_api_semantics.rs::timestamp_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Timestamp(NonZeroU64);
impl Timestamp {
    #[must_use]
    pub const fn from_nanos(nanos: u64) -> Option<Self> {
        match NonZeroU64::new(nanos) {
            Some(nz) => Some(Self(nz)),
            None => None,
        }
    }
    #[must_use]
    pub const fn as_nanos(self) -> u64 {
        self.0.get()
    }
}
impl crate::sealed::Sealed for Timestamp {}
impl EventSafe for Timestamp {}
impl Encode for Timestamp {
    fn encode(&self, out: &mut Vec<u8>) {
        self.0.encode(out);
    }
}
impl Decode for Timestamp {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let nz = NonZeroU64::decode(d)?;
        Ok(Self(nz))
    }
}
/// Advisory cost class for a `Validate::validate()` call.
///
/// **Informational only.** The substrate does not branch on this value;
/// see the module-level "Contract" section for the load-bearing rule.
/// `#[non_exhaustive]` so new cost classes can be added in
/// ADR-0009-minor releases; removing a variant is a major break.
///
/// All in-tree implementors use [`ValidationCost::Cheap`]. The
/// `Bounded` and `Unbounded` variants exist for downstream adopters
/// whose payload types cannot be constructor-validated.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationCost {
    /// No work — the type's domain invariants are upheld by its
    /// constructor and `validate()` is a literal `Ok(())`.
    Free,
    /// Constant-time re-check on an already-constructed value. The
    /// in-tree default; appropriate for all constructor-validated
    /// newtypes (substrate vocabulary, bounded containers, totally-
    /// ordered floats, surrogate-rejecting chars).
    Cheap,
    /// Up to `ops` elementary operations; suitable for downstream
    /// payload types that must scan a fixed-size buffer or evaluate a
    /// bounded predicate set. Substrate makes no use of `ops`; it is
    /// purely informational for adopters reasoning about their replay
    /// budgets. The exact unit of "operation" is impl-defined.
    Bounded {
        /// Upper bound on elementary operations per `validate()` call.
        ops: u32,
    },
    /// Cost not bounded by a static constant — crypto, recursive
    /// walks, externally-sourced size.
    ///
    /// **Substrate pin (`rescue-pardosa-4kd3`):**
    /// `pardosa::stream_validated` still invokes `validate()`
    /// per event for `Unbounded` payloads. Adopters route
    /// through `stream_checked` (envelope-only) or lift the
    /// heavy check into construction (impl becomes `Cheap`).
    ///
    /// Typed escape hatch: some downstream payloads can't be
    /// constructor-validated. Removing this would be
    /// ADR-0009-major and would force mis-advertisement
    /// (`Bounded { ops: u32::MAX }`).
    Unbounded,
}
/// Domain-shape validation for substrate vocabulary (ADR-0014,
/// F6).
///
/// **OPEN trait**: downstream crates may impl it. In-tree
/// impls live in `pardosa-schema`:
///
/// * `EventString<MAX>`, `EventBytes<MAX>`, `EventVec<T,MAX>`,
///   `NonEmptyEventString<MAX>` (`bounded.rs`).
/// * `RealF32`, `RealF64`, `OrderedF32`, `OrderedF64`
///   (`floats.rs`).
/// * `CharScalar` (`char_scalar.rs`).
///
/// All in-tree impls are constructor-validated; `COST =
/// Cheap` (or `Free`) — `validate()` is a constant-time
/// re-check. Substrate boundary for callers deserialising
/// foreign payloads.
pub trait Validate {
    type Error;
    const COST: ValidationCost = ValidationCost::Cheap;
    /// Validate `self` against its domain invariants.
    ///
    /// # Errors
    /// Returns `Self::Error` when an invariant is violated. The error type is implementation-defined.
    fn validate(&self) -> Result<(), Self::Error>;
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{from_bytes, to_vec};
    #[test]
    fn timestamp_roundtrip_byte_identical() {
        let ts = Timestamp::from_nanos(1_700_000_000_000_000_000).expect("nonzero");
        let bytes = to_vec(&ts);
        let back: Timestamp = from_bytes(&bytes).expect("decode");
        assert_eq!(ts, back);
        let nz_bytes = to_vec(&ts.0);
        assert_eq!(bytes, nz_bytes);
    }
    #[test]
    fn timestamp_decode_zero_rejected() {
        let zero_bytes = to_vec(&0u64);
        let err = from_bytes::<Timestamp>(&zero_bytes).expect_err("zero must reject");
        assert!(matches!(err, DecodeError::InvalidValue));
    }
    #[test]
    fn timestamp_decode_nonzero_accepted() {
        let bytes = to_vec(&42u64);
        let ts: Timestamp = from_bytes(&bytes).expect("nonzero decodes");
        assert_eq!(ts.as_nanos(), 42);
    }
}
