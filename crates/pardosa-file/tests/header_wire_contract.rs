use pardosa_file::format::{HEADER_SCHEMA_HASH_LEN, HEADER_SCHEMA_HASH_OFFSET};
const KNOWN_HASH: u128 = 0x0F0E_0D0C_0B0A_0908_0706_0504_0302_0100;
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
    assert_eq!(HEADER_SCHEMA_HASH_LEN, 16);
    assert_eq!(HEADER_SCHEMA_HASH_LEN, core::mem::size_of::<u128>());
}
#[test]
fn schema_hash_offset_is_8() {
    assert_eq!(HEADER_SCHEMA_HASH_OFFSET, 8);
}
