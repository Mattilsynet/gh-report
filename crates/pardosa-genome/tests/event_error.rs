//! F1 reframed: in-house EventError encoding produces a single discriminant
//! byte per variant.
//!
//! Package contract v2.1 §4.5: EventError is a `repr(u8)` enum with literal
//! discriminants 0..=10. The in-house canonical encoding (GEN-0035) emits
//! the discriminant byte as the entire payload for unit-like variants —
//! byte-1 of the encoded form equals the discriminant value. This file
//! pins the `Internal` discriminant at `7u8` and asserts the single-byte
//! encoding contract.

use pardosa_genome::{Encode, EventError};

#[test]
fn event_error_internal_encodes_to_single_byte_0x07() {
    let mut buf = Vec::new();
    EventError::Internal.encode(&mut buf);
    assert_eq!(buf.len(), 1, "EventError encoding must be a single byte");
    assert_eq!(
        buf[0], 7u8,
        "EventError::Internal must encode to discriminant 7"
    );
}

#[test]
fn event_error_all_variants_encode_to_pinned_discriminants() {
    // Package contract v2.1 §4.5: 11 variants, repr(u8) literal 0..=10.
    let cases: [(EventError, u8); 11] = [
        (EventError::InvalidInput, 0),
        (EventError::NotFound, 1),
        (EventError::Conflict, 2),
        (EventError::Unauthorized, 3),
        (EventError::PermissionDenied, 4),
        (EventError::Unavailable, 5),
        (EventError::Timeout, 6),
        (EventError::Internal, 7),
        (EventError::ResourceExhausted, 8),
        (EventError::Cancelled, 9),
        (EventError::DataLoss, 10),
    ];
    for (variant, expected_byte) in cases {
        let mut buf = Vec::new();
        variant.encode(&mut buf);
        assert_eq!(buf.len(), 1, "variant {variant:?} must encode to 1 byte");
        assert_eq!(
            buf[0], expected_byte,
            "variant {variant:?} must encode to {expected_byte}"
        );
    }
}
