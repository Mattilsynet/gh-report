//! PAR-0021 canonical-bytes pin tests.
//!
//! Hand-rolled `impl Encode` lives in `crates/pardosa/src/event.rs` (Path-B per
//! F2c brief — pardosa runtime does not consume `pardosa-derive`, see
//! PAR-0024 R5). These tests pin the wire layout against `pardosa-encoding`'s
//! primitive `Encode` impls so any future drift in field order / type widths
//! is caught at the byte level rather than only at semantic call sites.

use pardosa::event::{DomainId, Event, Index};

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

#[test]
fn event_string_canonical_bytes_pin() {
    // Pin the full GEN-0035 wire layout of Event<String> against a manually
    // assembled byte vector. Field order MUST match the struct declaration in
    // crates/pardosa/src/event.rs:217: event_id, timestamp, domain_id,
    // detached, precursor, precursor_hash, domain_event. Any reorder breaks
    // PAR-0021 R1 (precursor identity) for previously-committed lines.
    let event = Event::<String>::new(
        1,
        2,
        DomainId::new(3),
        false,
        Index::new(4),
        [0u8; 32],
        "hi".to_string(),
    );

    let mut expected: Vec<u8> = Vec::new();
    expected.extend_from_slice(&1u64.to_le_bytes()); // event_id
    expected.extend_from_slice(&2i64.to_le_bytes()); // timestamp
    expected.extend_from_slice(&3u64.to_le_bytes()); // domain_id
    expected.push(0u8); // detached = false → 0
    expected.extend_from_slice(&4u64.to_le_bytes()); // precursor
    expected.extend_from_slice(&[0u8; 32]); // precursor_hash
    // domain_event "hi": String per pardosa-encoding/src/lib.rs:447 = u32 LE
    // length prefix + UTF-8 bytes.
    expected.extend_from_slice(&2u32.to_le_bytes());
    expected.extend_from_slice(b"hi");

    assert_eq!(pardosa_encoding::to_vec(&event), expected);
}

#[test]
fn event_genesis_canonical_bytes_pin() {
    // Genesis-event shape: precursor = Index::NONE (u64::MAX), precursor_hash
    // = [0u8; 32]. Pins that NONE serialises as u64::MAX LE inside the event
    // canonical bytes — important because PAR-0021 R2 carries the genesis
    // marker through the precursor field.
    let event = Event::<String>::new(
        0,
        0,
        DomainId::new(1),
        false,
        Index::NONE,
        [0u8; 32],
        String::new(),
    );

    let mut expected: Vec<u8> = Vec::new();
    expected.extend_from_slice(&0u64.to_le_bytes()); // event_id
    expected.extend_from_slice(&0i64.to_le_bytes()); // timestamp
    expected.extend_from_slice(&1u64.to_le_bytes()); // domain_id
    expected.push(0u8); // detached
    expected.extend_from_slice(&u64::MAX.to_le_bytes()); // Index::NONE
    expected.extend_from_slice(&[0u8; 32]); // precursor_hash
    expected.extend_from_slice(&0u32.to_le_bytes()); // empty String length
    // (no payload bytes for empty string)

    assert_eq!(pardosa_encoding::to_vec(&event), expected);
}
