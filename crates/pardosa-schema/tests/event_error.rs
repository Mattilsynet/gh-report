use pardosa_schema::{Encode, StatusCode};
#[test]
fn status_code_internal_encodes_to_single_byte_0x07() {
    let mut buf = Vec::new();
    StatusCode::Internal.encode(&mut buf);
    assert_eq!(buf.len(), 1, "StatusCode encoding must be a single byte");
    assert_eq!(
        buf[0], 7u8,
        "StatusCode::Internal must encode to discriminant 7"
    );
}
#[test]
fn status_code_all_variants_encode_to_pinned_discriminants() {
    let cases: [(StatusCode, u8); 11] = [
        (StatusCode::InvalidInput, 0),
        (StatusCode::NotFound, 1),
        (StatusCode::Conflict, 2),
        (StatusCode::Unauthorized, 3),
        (StatusCode::PermissionDenied, 4),
        (StatusCode::Unavailable, 5),
        (StatusCode::Timeout, 6),
        (StatusCode::Internal, 7),
        (StatusCode::ResourceExhausted, 8),
        (StatusCode::Cancelled, 9),
        (StatusCode::DataLoss, 10),
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
