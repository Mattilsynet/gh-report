use std::fmt;
use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};

/// Validated aggregate instance identifier — the stream partition key.
///
/// Identifies a specific aggregate instance within an event store.
/// Each aggregate's event stream is keyed by its `AggregateId`.
/// The `(AggregateId, sequence)` tuple is the globally unique
/// coordinate for any single event.
///
/// # ID assignment
///
/// Aggregate IDs are assigned by the [`EventStore`](crate::EventStore)
/// via its `create` method. The store auto-increments from 1. Callers
/// never invent IDs — they receive them from the store on creation and
/// pass them back on subsequent commands.
/// (CHE-0020: infrastructure-owned identity.)
///
/// # Non-zero invariant
///
/// Backed by `NonZeroU64` — zero is not a valid aggregate ID.
/// Store-assigned IDs start from 1, so zero never occurs in practice.
/// This eliminates the `AggregateId(0)` hole at the type level with
/// zero runtime cost (niche optimization allows `Option<AggregateId>`
/// to be the same size as `AggregateId`).
/// (CHE-0011: `AggregateId` as `NonZeroU64`.)
///
/// # Single-writer assumption
///
/// Cherry-pit assumes single-writer aggregates: each aggregate instance
/// is owned by exactly one process. This makes sequential numeric IDs
/// safe without distributed coordination.
/// (CHE-0006: single-writer per aggregate.)
///
/// # Examples
///
/// ```
/// use std::num::NonZeroU64;
/// use cherry_pit_core::AggregateId;
///
/// // Construct from NonZeroU64 (the only infallible path).
/// let nz = NonZeroU64::new(42).unwrap();
/// let id = AggregateId::new(nz);
/// assert_eq!(id.get(), 42);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AggregateId(NonZeroU64);

impl AggregateId {
    /// Create an aggregate ID from a `NonZeroU64`.
    #[must_use]
    pub const fn new(id: NonZeroU64) -> Self {
        Self(id)
    }

    /// Extract the inner `NonZeroU64` value.
    #[must_use]
    pub const fn into_inner(self) -> NonZeroU64 {
        self.0
    }

    /// Extract the inner value as a plain `u64`.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Display for AggregateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// CHE-0064:R2 — hand-rolled Encode (no derive) per PAR-0024:R5.
// Delegates verbatim to the inner `NonZeroU64`'s canonical encoding
// (pardosa-encoding/src/lib.rs Encode for NonZeroU64). Wire-identical
// to a bare `u64` LE — niche-optimisation of AggregateId is irrelevant
// on the wire.
impl pardosa_encoding::Encode for AggregateId {
    fn encode(&self, out: &mut Vec<u8>) {
        self.0.encode(out);
    }
}

// Symmetric Decode — delegates to NonZeroU64's Decode, which rejects the
// 0u64 niche violation as EventError::InvalidInput. Hand-rolled per
// PAR-0024:R5 (Decode mirrors Encode; the encoding crate's traits are
// deliberately not `#[derive]`-able).
impl pardosa_encoding::Decode for AggregateId {
    fn decode(d: &mut pardosa_encoding::Decoder<'_>) -> Result<Self, pardosa_encoding::EventError> {
        let inner = <NonZeroU64 as pardosa_encoding::Decode>::decode(d)?;
        Ok(Self(inner))
    }
}

impl From<NonZeroU64> for AggregateId {
    fn from(id: NonZeroU64) -> Self {
        Self(id)
    }
}

impl From<AggregateId> for NonZeroU64 {
    fn from(id: AggregateId) -> Self {
        id.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> AggregateId {
        AggregateId::new(NonZeroU64::new(n).unwrap())
    }

    #[test]
    fn display_outputs_inner_u64() {
        assert_eq!(id(42).to_string(), "42");
        assert_eq!(id(1).to_string(), "1");
        assert_eq!(id(u64::MAX).to_string(), u64::MAX.to_string());
    }

    #[test]
    fn copy_semantics() {
        let original = id(1);
        let copy = original; // Copy, not move
        assert_eq!(original, copy); // original still usable
    }

    #[test]
    fn from_non_zero_u64() {
        let nz = NonZeroU64::new(7).unwrap();
        let aggregate_id: AggregateId = nz.into();
        assert_eq!(aggregate_id.get(), 7);
    }

    #[test]
    fn into_non_zero_u64() {
        let aggregate_id = id(99);
        let nz: NonZeroU64 = aggregate_id.into();
        assert_eq!(nz.get(), 99);
    }

    #[test]
    fn ord_matches_u64_ordering() {
        let a = id(1);
        let b = id(2);
        let c = id(2);
        assert!(a < b);
        assert_eq!(b, c);
    }

    #[test]
    fn serde_json_roundtrip() {
        let aggregate_id = id(42);
        let json = serde_json::to_string(&aggregate_id).unwrap();
        assert_eq!(json, "42");
        let back: AggregateId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, aggregate_id);
    }

    #[test]
    fn serde_json_zero_rejected() {
        let result = serde_json::from_str::<AggregateId>("0");
        assert!(result.is_err());
    }

    #[test]
    fn serde_msgpack_roundtrip() {
        let aggregate_id = id(42);
        let bytes = rmp_serde::to_vec(&aggregate_id).unwrap();
        let back: AggregateId = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(back, aggregate_id);
    }

    #[test]
    fn serde_msgpack_zero_rejected() {
        // Serialize a raw 0u64 and attempt to deserialize as AggregateId.
        let bytes = rmp_serde::to_vec(&0u64).unwrap();
        let result = rmp_serde::from_slice::<AggregateId>(&bytes);
        assert!(result.is_err());
    }

    #[test]
    fn serde_msgpack_wire_format_matches_raw_u64() {
        // Verify NonZeroU64 serializes identically to u64 in msgpack.
        let raw_bytes = rmp_serde::to_vec(&42u64).unwrap();
        let id_bytes = rmp_serde::to_vec(&id(42)).unwrap();
        assert_eq!(raw_bytes, id_bytes);
    }

    #[test]
    fn pardosa_encoding_roundtrip() {
        // δ.3a-pre — symmetric Encode/Decode via pardosa-encoding.
        // Wire-identical to bare u64 LE per the Encode comment above.
        for v in [1u64, 42, u64::MAX] {
            let id = id(v);
            let bytes = pardosa_encoding::to_vec(&id);
            assert_eq!(bytes.len(), 8, "AggregateId encodes to 8 LE bytes");
            let back: AggregateId = pardosa_encoding::from_bytes(&bytes).expect("decode");
            assert_eq!(back, id);
        }
    }

    #[test]
    fn pardosa_encoding_rejects_zero() {
        // NonZeroU64::decode rejects 0u64 as InvalidInput; AggregateId
        // inherits the rejection through delegation.
        let bytes = pardosa_encoding::to_vec(&0u64);
        let err =
            pardosa_encoding::from_bytes::<AggregateId>(&bytes).expect_err("zero must be rejected");
        assert_eq!(err, pardosa_encoding::EventError::InvalidInput);
    }

    #[test]
    fn hash_consistent() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(id(1));
        set.insert(id(1));
        set.insert(id(2));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn const_new() {
        // Verify new() is usable in const context.
        const NZ: NonZeroU64 = match NonZeroU64::new(1) {
            Some(v) => v,
            None => panic!("zero"),
        };
        const ID: AggregateId = AggregateId::new(NZ);
        assert_eq!(ID.get(), 1);
    }

    #[test]
    fn get_returns_raw_u64() {
        let aggregate_id = id(42);
        assert_eq!(aggregate_id.get(), 42);
    }

    #[test]
    fn option_aggregate_id_same_size() {
        // NonZeroU64 niche optimization: Option<AggregateId> is same size.
        assert_eq!(
            std::mem::size_of::<AggregateId>(),
            std::mem::size_of::<Option<AggregateId>>()
        );
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn aggregate_id_nonzero_roundtrip(val in 1..=u64::MAX) {
                let nz = NonZeroU64::new(val).unwrap();
                let id = AggregateId::new(nz);
                prop_assert_eq!(id.get(), val);

                let back: NonZeroU64 = id.into();
                prop_assert_eq!(back.get(), val);
            }

            #[test]
            fn aggregate_id_json_roundtrip(val in 1..=u64::MAX) {
                let id = AggregateId::new(NonZeroU64::new(val).unwrap());
                let json = serde_json::to_string(&id).unwrap();
                let back: AggregateId = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(back, id);
            }

            #[test]
            fn aggregate_id_msgpack_roundtrip(val in 1..=u64::MAX) {
                let id = AggregateId::new(NonZeroU64::new(val).unwrap());
                let bytes = rmp_serde::to_vec(&id).unwrap();
                let back: AggregateId = rmp_serde::from_slice(&bytes).unwrap();
                prop_assert_eq!(back, id);
            }

            #[test]
            fn aggregate_id_pardosa_encoding_roundtrip(val in 1..=u64::MAX) {
                // δ.3a-pre — symmetric pardosa-encoding round-trip.
                let id = AggregateId::new(NonZeroU64::new(val).unwrap());
                let bytes = pardosa_encoding::to_vec(&id);
                let back: AggregateId =
                    pardosa_encoding::from_bytes(&bytes).expect("decode");
                prop_assert_eq!(back, id);
            }
        }
    }
}
