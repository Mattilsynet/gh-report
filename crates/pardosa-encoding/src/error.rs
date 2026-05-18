//! Canonical event-level error surface for pardosa (GEN-0039).
//!
//! `EventError` carries pinned `repr(u8)` discriminants 0..=10 that are
//! part of the wire contract. The variants are also the return type of
//! [`crate::Decode::decode`] following the C2 migration: all decoder-local
//! failure modes collapse to `EventError::InvalidInput`.

use alloc::vec::Vec;

use crate::{Decode, Decoder, Encode};

/// Canonical event-level error surface for pardosa (GEN-0039).
///
/// `repr(u8)` with literal discriminants pinned 0..=10. The in-house
/// canonical encoding emits a single byte equal to the discriminant for
/// each variant (F4 wire contract: byte-1 of an encoded `EventError`
/// equals the discriminant value).
///
/// Variant ordering and discriminant values are part of the wire
/// contract — see GEN-0039. Renumbering is a breaking change.
///
/// `EventError` is also the return type of [`Decode::decode`] following
/// the C2 migration (sub-mission `adr-fmt-vggv`): all decoder-local
/// failure modes (truncated input, cap exceeded, invalid discriminant,
/// invalid UTF-8, non-canonical map, trailing bytes) collapse to
/// `EventError::InvalidInput`, matching the pre-migration bridge
/// semantics from `pardosa_traits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
#[non_exhaustive]
pub enum EventError {
    /// Caller-supplied data violated a documented input invariant.
    /// All decoder-local failures (truncated input, malformed tag,
    /// non-canonical ordering, cap exceeded, trailing bytes, invalid
    /// UTF-8, unknown discriminant) surface as this variant.
    InvalidInput = 0,
    /// The addressed entity does not exist.
    NotFound = 1,
    /// The operation conflicts with the current state (e.g. version mismatch,
    /// duplicate key, concurrent write race).
    Conflict = 2,
    /// Caller is not authenticated.
    Unauthorized = 3,
    /// Caller is authenticated but lacks permission for the operation.
    PermissionDenied = 4,
    /// A required dependency is temporarily unavailable; retry may succeed.
    Unavailable = 5,
    /// The operation did not complete within its deadline.
    Timeout = 6,
    /// An internal invariant was violated. Carries no caller-actionable
    /// detail; surface only as an opaque failure.
    Internal = 7,
    /// A resource quota or limit was exceeded (memory, message size, rate).
    ResourceExhausted = 8,
    /// The operation was explicitly cancelled before completion.
    Cancelled = 9,
    /// Underlying storage reported irrecoverable data loss for the
    /// affected entity.
    DataLoss = 10,
}

impl EventError {
    /// Return the wire discriminant byte for this variant.
    ///
    /// Equivalent to the first (and only) byte of `EventError::encode`.
    /// Pinned by GEN-0039; renumbering is a breaking change.
    #[must_use]
    pub const fn discriminant(self) -> u8 {
        // `repr(u8)` makes the cast a no-op at the bit level.
        self as u8
    }
}

impl Encode for EventError {
    fn encode(&self, out: &mut Vec<u8>) {
        // Single byte per GEN-0039 F4 wire contract. `repr(u8)` makes
        // `self.discriminant()` bit-identical to the variant's pinned
        // discriminant.
        out.push(self.discriminant());
    }
}

impl Decode for EventError {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        // Exhaustive match on the pinned 0..=10 wire bytes. Unknown
        // discriminants surface as `InvalidInput`, matching the
        // post-C2 convention for all decoder-local failures.
        let byte = u8::decode(d)?;
        match byte {
            0 => Ok(EventError::InvalidInput),
            1 => Ok(EventError::NotFound),
            2 => Ok(EventError::Conflict),
            3 => Ok(EventError::Unauthorized),
            4 => Ok(EventError::PermissionDenied),
            5 => Ok(EventError::Unavailable),
            6 => Ok(EventError::Timeout),
            7 => Ok(EventError::Internal),
            8 => Ok(EventError::ResourceExhausted),
            9 => Ok(EventError::Cancelled),
            10 => Ok(EventError::DataLoss),
            _ => Err(EventError::InvalidInput),
        }
    }
}
