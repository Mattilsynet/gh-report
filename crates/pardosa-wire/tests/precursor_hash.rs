#![cfg(feature = "blake3")]
use pardosa_wire::precursor_hash_of;
const BLAKE3_EMPTY: [u8; 32] = [
    0xaf, 0x13, 0x49, 0xb9, 0xf5, 0xf9, 0xa1, 0xa6, 0xa0, 0x40, 0x4d, 0xea, 0x36, 0xdc, 0xc9, 0x49,
    0x9b, 0xcb, 0x25, 0xc9, 0xad, 0xc1, 0x12, 0xb7, 0xcc, 0x9a, 0x93, 0xca, 0xe4, 0x1f, 0x32, 0x62,
];
#[test]
fn precursor_hash_of_empty_matches_blake3_known_vector() {
    let h = precursor_hash_of(b"");
    assert_eq!(h, BLAKE3_EMPTY);
    let recomputed: [u8; 32] = blake3::hash(b"").into();
    assert_eq!(h, recomputed);
}
#[test]
fn precursor_hash_of_known_input_stable() {
    let h1 = precursor_hash_of(b"hello pardosa");
    let h2 = precursor_hash_of(b"hello pardosa");
    assert_eq!(h1, h2);
    let h3 = precursor_hash_of(b"hello pardosa!");
    assert_ne!(h1, h3);
}
#[test]
fn precursor_hash_of_output_is_thirty_two_bytes() {
    let h: [u8; 32] = precursor_hash_of(b"any input");
    assert_eq!(h.len(), 32);
}
