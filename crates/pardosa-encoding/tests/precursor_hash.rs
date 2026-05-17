//! PAR-0021 R1 BLAKE3 precursor-hash helper tests.
//!
//! These tests are feature-gated under `blake3`; the no-feature build of
//! `pardosa-encoding` does not see this file's body (the `cfg` gate on the
//! module turns it into a no-op when `--features blake3` is not active).

#![cfg(feature = "blake3")]

use pardosa_encoding::precursor_hash_of;

/// PAR-0021 R1 wire-contract pin: empty-input BLAKE3 digest. Known vector
/// from the BLAKE3 reference (RFC, test vectors). Pinning the literal here
/// guards against accidentally swapping the hash function or its
/// canonicalisation later.
const BLAKE3_EMPTY: [u8; 32] = [
    0xaf, 0x13, 0x49, 0xb9, 0xf5, 0xf9, 0xa1, 0xa6, 0xa0, 0x40, 0x4d, 0xea, 0x36, 0xdc, 0xc9, 0x49,
    0x9b, 0xcb, 0x25, 0xc9, 0xad, 0xc1, 0x12, 0xb7, 0xcc, 0x9a, 0x93, 0xca, 0xe4, 0x1f, 0x32, 0x62,
];

#[test]
fn precursor_hash_of_empty_matches_blake3_known_vector() {
    // Two assertions: (a) byte-equal to the spec literal, (b) byte-equal to
    // a freshly-computed `blake3::hash(b"")`. (b) defends against the
    // (unlikely) scenario that the literal above was transcribed wrong;
    // (a) defends against a future helper change that would silently shift
    // the wire identity.
    let h = precursor_hash_of(b"");
    assert_eq!(h, BLAKE3_EMPTY);
    let recomputed: [u8; 32] = blake3::hash(b"").into();
    assert_eq!(h, recomputed);
}

#[test]
fn precursor_hash_of_known_input_stable() {
    // Determinism: same bytes → same hash, across two calls.
    let h1 = precursor_hash_of(b"hello pardosa");
    let h2 = precursor_hash_of(b"hello pardosa");
    assert_eq!(h1, h2);

    // Sensitivity: one trailing byte differs → hash differs. Guards against
    // a degenerate impl that ignored the tail (e.g. constant return).
    let h3 = precursor_hash_of(b"hello pardosa!");
    assert_ne!(h1, h3);
}

#[test]
fn precursor_hash_of_output_is_thirty_two_bytes() {
    // Type-level pin: the return type is exactly `[u8; 32]`. The `let _:`
    // binding is the compile-time assertion; the runtime `len()` check
    // mirrors GEN-0035 / PAR-0021 R1 wire-byte expectation.
    let h: [u8; 32] = precursor_hash_of(b"any input");
    assert_eq!(h.len(), 32);
}
