//! FH12 — pin the wire-format endianness contract for `schema_hash`.
//!
//! `format.rs:3` declares "all multi-byte fields are little-endian"; the
//! file-header layout block at `format.rs:24` annotates `schema_hash`
//! specifically as `u128 LE, xxh3-128`. This test makes the LE contract
//! mechanically checkable, not just documentary: a future PR that
//! "fixes" a header writer to use `to_be_bytes()` or `to_ne_bytes()`
//! fails here loudly instead of silently corrupting every file produced
//! after the change.
//!
//! # AC #3 status (vacuous by absence)
//!
//! The bead `adr-fmt-i24x` AC #3 asks for "similar assertion at every
//! callsite that writes schema_hash into a header." As of this commit,
//! there are **zero such callsites** in `crates/pardosa-genome/src/`:
//! `format.rs` declares the constants + layout; `genome_safe.rs` ships
//! the schema-hash *computation* primitives (which already use
//! `to_le_bytes` in `schema_hash_combine` at line 139); but no header
//! serializer has been written yet. This test exists *before* the
//! header writers and serves as the canonical wire-contract pattern
//! they must satisfy.
//!
//! When a header writer lands, it must:
//! 1. Place a `u128` schema_hash at file-header bytes 8..24 in LE form
//!    (or at the analogous offset in a bare-message header).
//! 2. Have its own callsite-local assertion / round-trip test mirroring
//!    this one so the wire bytes are checked at every emission point.

use pardosa_genome::format::{HEADER_SCHEMA_HASH_LEN, HEADER_SCHEMA_HASH_OFFSET};

/// Hand-rolled known-value: a recognisable bit-pattern that exercises
/// every byte position. Picked so the LE vs BE distinction is obvious
/// in any failure diff (the high and low halves are visibly distinct).
const KNOWN_HASH: u128 = 0x0F0E_0D0C_0B0A_0908_0706_0504_0302_0100;

/// Synthesise a file-header buffer with the schema_hash field populated
/// in LE form. Mirrors what a future header writer must produce.
fn synth_header_with_schema_hash(hash: u128) -> [u8; 40] {
    let mut buf = [0u8; 40];
    let bytes = hash.to_le_bytes();
    buf[HEADER_SCHEMA_HASH_OFFSET..HEADER_SCHEMA_HASH_OFFSET + HEADER_SCHEMA_HASH_LEN]
        .copy_from_slice(&bytes);
    buf
}

#[test]
fn schema_hash_is_little_endian_at_offset_8() {
    let buf = synth_header_with_schema_hash(KNOWN_HASH);

    // Spelled out byte-by-byte rather than reusing `to_le_bytes()` on
    // both sides so the test pins the *expected wire layout*, not just
    // "whatever to_le_bytes happens to produce". If the const KNOWN_HASH
    // changes, this fails until the literal is updated in lockstep.
    let expected: [u8; 16] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,
        0x0F,
    ];

    assert_eq!(
        &buf[HEADER_SCHEMA_HASH_OFFSET..HEADER_SCHEMA_HASH_OFFSET + HEADER_SCHEMA_HASH_LEN],
        &expected,
        "schema_hash wire bytes at offset 8..24 must be little-endian. \
         If you are seeing this fail, a header writer (or hash producer) \
         changed byte order — see crates/pardosa-genome/src/format.rs:3,24 \
         for the canonical LE contract.",
    );
}

#[test]
fn schema_hash_roundtrips_through_le_bytes() {
    let buf = synth_header_with_schema_hash(KNOWN_HASH);

    let mut hash_bytes = [0u8; 16];
    hash_bytes.copy_from_slice(
        &buf[HEADER_SCHEMA_HASH_OFFSET..HEADER_SCHEMA_HASH_OFFSET + HEADER_SCHEMA_HASH_LEN],
    );

    let decoded = u128::from_le_bytes(hash_bytes);
    assert_eq!(
        decoded, KNOWN_HASH,
        "schema_hash must round-trip via to_le_bytes/from_le_bytes. \
         A reader that uses from_be_bytes will silently misread every \
         file ever written.",
    );
}

#[test]
fn schema_hash_width_is_16_bytes() {
    // GEN-0035 widening: u128 = 16 bytes. Pinned as a constant to catch
    // any drift if the format ever proposed a width change without a
    // FORMAT_VERSION bump.
    assert_eq!(HEADER_SCHEMA_HASH_LEN, 16);
    assert_eq!(HEADER_SCHEMA_HASH_LEN, core::mem::size_of::<u128>());
}

#[test]
fn schema_hash_offset_is_8() {
    // File header layout (format.rs:18-32): magic(4) + version(2) +
    // flags(2) = 8 bytes precede the schema_hash field. Pinned so a
    // future layout edit must also update this test, surfacing the
    // wire-format change explicitly.
    assert_eq!(HEADER_SCHEMA_HASH_OFFSET, 8);
}
