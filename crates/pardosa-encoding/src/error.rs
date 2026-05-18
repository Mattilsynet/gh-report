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

#[cfg(test)]
mod tests {
    use super::EventError;
    use crate::{from_bytes, to_vec};

    // -----------------------------------------------------------------
    // EventError wire-contract pins (GEN-0039 / footgun FH6 = F11+F12)
    // -----------------------------------------------------------------
    //
    // These tests freeze the wire byte for every `EventError` variant.
    // If a future edit reorders or renumbers the enum, the assertions
    // below break loudly instead of the wire silently shifting.

    #[test]
    fn event_error_discriminants_pinned() {
        // One assert per variant — GEN-0039 wire contract, byte 1.
        assert_eq!(EventError::InvalidInput.discriminant(), 0);
        assert_eq!(EventError::NotFound.discriminant(), 1);
        assert_eq!(EventError::Conflict.discriminant(), 2);
        assert_eq!(EventError::Unauthorized.discriminant(), 3);
        assert_eq!(EventError::PermissionDenied.discriminant(), 4);
        assert_eq!(EventError::Unavailable.discriminant(), 5);
        assert_eq!(EventError::Timeout.discriminant(), 6);
        assert_eq!(EventError::Internal.discriminant(), 7);
        assert_eq!(EventError::ResourceExhausted.discriminant(), 8);
        assert_eq!(EventError::Cancelled.discriminant(), 9);
        assert_eq!(EventError::DataLoss.discriminant(), 10);
    }

    #[test]
    fn event_error_roundtrip_every_variant() {
        // Symmetric Encode/Decode for every variant 0..=10.
        for v in [
            EventError::InvalidInput,
            EventError::NotFound,
            EventError::Conflict,
            EventError::Unauthorized,
            EventError::PermissionDenied,
            EventError::Unavailable,
            EventError::Timeout,
            EventError::Internal,
            EventError::ResourceExhausted,
            EventError::Cancelled,
            EventError::DataLoss,
        ] {
            let bytes = to_vec(&v);
            assert_eq!(bytes.len(), 1, "EventError encodes to one byte");
            assert_eq!(bytes[0], v.discriminant());
            let back: EventError = from_bytes(&bytes).expect("decode");
            assert_eq!(v, back);
        }
    }

    #[test]
    fn event_error_unknown_discriminant_rejected() {
        // Discriminants 11..=255 are not assigned; decode must reject.
        for b in 11u8..=255 {
            let err = from_bytes::<EventError>(&[b]).unwrap_err();
            assert_eq!(err, EventError::InvalidInput);
        }
    }
}
