//! PAR-0021 canonical-bytes pin tests.
//!
//! Hand-rolled `impl Encode` lives in `crates/pardosa/src/event.rs` (Path-B per
//! F2c brief — pardosa runtime does not consume `pardosa-derive`, see
//! PAR-0024 R5). These tests pin the wire layout against `pardosa-encoding`'s
//! primitive `Encode` impls so any future drift in field order / type widths
//! is caught at the byte level rather than only at semantic call sites.

use pardosa::event::{DomainId, Index};

#[test]
fn index_encodes_as_u64_le() {
    // GEN-0037 tuple-struct rule: single-field tuple newtype encodes as the
    // inner field. Pin against `7u64.to_le_bytes()` so any accidental change
    // to wrapping (e.g. adding a length prefix or sign-extending) is loud.
    assert_eq!(
        pardosa_encoding::to_vec(&Index::new(7)),
        7u64.to_le_bytes().to_vec()
    );
}

#[test]
fn domain_id_encodes_as_u64_le() {
    assert_eq!(
        pardosa_encoding::to_vec(&DomainId::new(0xDEAD_BEEF_u64)),
        0xDEAD_BEEF_u64.to_le_bytes().to_vec()
    );
}

#[test]
fn index_none_encodes_as_u64_max_le() {
    // Index::NONE is the u64::MAX sentinel and must round-trip through Encode
    // for chained-event canonical bytes — the precursor field of a genesis
    // event carries NONE on the wire.
    let bytes = pardosa_encoding::to_vec(&Index::NONE);
    assert_eq!(bytes, u64::MAX.to_le_bytes().to_vec());
}
