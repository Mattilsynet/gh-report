use crate::error::PardosaError;
use pardosa_schema::{GenomeSafe, schema_hash_bytes};
use pardosa_wire::{Decode, DecodeError, Decoder, Encode, EventSafe};
use serde::{Deserialize, Serialize};
use std::fmt;
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[repr(transparent)]
pub struct Index(u64);
impl Index {
    pub const ZERO: Index = Index(0);
    /// Construct an `Index` from a raw `u64`.
    ///
    /// Removed from the default-feature public API (ADR-0017 §D1 /
    /// ADR-0003 §1): `Index` is a substrate-owned monotonic position;
    /// external callers fabricating one violate the
    /// "indices are substrate-issued" invariant. Production mint
    /// paths are [`Index::checked_next`] and the internal [`Decode`]
    /// impl (via the substrate-internal `Index::from_decoded`
    /// constructor). Available under `feature = "test-support"`
    /// (and `cfg(test)`) for raw / tamper fixtures and adopter-facing
    /// CLI / debug tooling.
    #[must_use]
    #[cfg(any(test, feature = "test-support"))]
    pub const fn new(v: u64) -> Self {
        Index(v)
    }
    /// Substrate-internal constructor used by the [`Decode`] impl
    /// (and by the persistence rebuild path) — the one legitimate
    /// raw-`u64` entry point on the default feature set.
    #[must_use]
    pub(crate) const fn from_decoded(v: u64) -> Self {
        Index(v)
    }
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
    /// Return the next `Index` value.
    ///
    /// # Errors
    /// Returns `PardosaError::IndexOverflow` on `u64` wrap.
    pub fn checked_next(self) -> Result<Index, PardosaError> {
        self.0
            .checked_add(1)
            .map(Index)
            .ok_or(PardosaError::IndexOverflow)
    }
}
impl fmt::Display for Index {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
/// Conversion-failure type for `TryFrom<Index> for usize` (W3,
/// roadmap correctness 2026-05-24). Preserves the raw decoded
/// `u64` so callers can fold it into a local typed-error variant
/// (`CheckedReplayKind::PrecursorOutOfBounds`,
/// `LinevecAppendKind::PrecursorIndexOutOfBounds`,
/// `BrokenPrecursorChain`, …) without lossy truncation.
///
/// Structurally unreachable on the only supported target
/// (`lib.rs` gates a 64-bit `target_pointer_width`); the type
/// exists so substrate-internal indexing sites stay panic-free
/// even where the compiler cannot prove the equality of widths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Index value {0} does not fit in usize on this target")]
pub struct IndexTooLargeForUsize(pub u64);
impl TryFrom<Index> for usize {
    type Error = IndexTooLargeForUsize;
    fn try_from(i: Index) -> Result<usize, Self::Error> {
        usize::try_from(i.0).map_err(|_| IndexTooLargeForUsize(i.0))
    }
}
impl Encode for Index {
    fn encode(&self, out: &mut Vec<u8>) {
        self.0.encode(out);
    }
}
impl Decode for Index {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        u64::decode(d).map(Index::from_decoded)
    }
}
impl pardosa_wire::sealed::Sealed for Index {}
impl EventSafe for Index {}
impl GenomeSafe for Index {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"Index");
    const SCHEMA_SOURCE: &'static str = "Index";
}
