//! Golden v5 byte-level vectors for PGNO files.
//!
//! `FORMAT_VERSION` bumped 5 -> 6 (PGN-0021 R5): a deliberate,
//! ADR-sanctioned breaking change — v6 readers cannot open v5
//! containers (oracle-ratified: adr-fmt-e5lzz, "correctly authorized
//! breaking-format cost, not a defect"). These vectors therefore no
//! longer exercise a live round-trip through `Writer`/`Reader` (the
//! current `Writer` cannot emit v5 bytes); they pin the frozen v5
//! wire bytes as a hand-built historical artefact and assert the
//! current `Reader` refuses to open them.
//!
//! See [ADR-0006](../../../docs/adr/0006-pgno-file-format.md) for the
//! canonical v5 file-format contract and
//! [PGN-0021](../../../docs/adr/pardosa/PGN-0021-opaque-semantic-fence-adopter-epoch-gate.md)
//! for the v6 bump.
use pardosa_file::format::{
    FILE_FOOTER_SIZE, FILE_HEADER_SIZE, FOOTER_CHECKSUM_OFFSET, FOOTER_INDEX_OFFSET,
    FOOTER_MAGIC_OFFSET, FOOTER_MESSAGE_COUNT_OFFSET, HEADER_FLAGS_OFFSET, HEADER_MAGIC_OFFSET,
    HEADER_PAGE_CLASS_OFFSET, HEADER_RESERVED_LEN, HEADER_RESERVED_OFFSET, HEADER_SCHEMA_HASH_LEN,
    HEADER_SCHEMA_HASH_OFFSET, HEADER_SCHEMA_SIZE_OFFSET, HEADER_VERSION_OFFSET, INDEX_ENTRY_SIZE,
    MAGIC, messages_offset, pad_to_8,
};
use pardosa_file::{FileError, Reader};
use std::io::Cursor;
use xxhash_rust::xxh64::xxh64;
/// Pins the documented `messages_offset` formula against concrete
/// (version-independent) schema sizes.
#[test]
fn golden_messages_offset_schedule() {
    assert_eq!(messages_offset(0), 40);
    assert_eq!(messages_offset(1), 48);
    assert_eq!(messages_offset(7), 48);
    assert_eq!(messages_offset(8), 48);
    assert_eq!(messages_offset(9), 56);
    assert_eq!(messages_offset(13), 56);
    assert_eq!(messages_offset(17), 64);
    assert_eq!(messages_offset(64), 40 + 64);
}
/// Golden vector A: frozen v5, zero-message, zero-schema bytes,
/// hand-built (the live `Writer` now emits v6). Layout:
///   header (40 bytes) — magic "PGNO" | version=5 | flags=0 |
///   `schema_hash=0..15` | `dict_id=0` | `page_class=0` |
///   `schema_size=0` | reserved(7)=0
///   footer (32 bytes) — `index_offset=40` | `message_count=0` |
///   reserved(4)=0 | magic "PGNO" | xxh64(footer[..24])
#[test]
fn golden_v5_empty_file_byte_literal_refused_by_v6_reader() {
    const EXPECTED_EMPTY: &[u8] = &[
        0x50, 0x47, 0x4E, 0x4F, 0x05, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06,
        0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x28, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x50, 0x47, 0x4E, 0x4F, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC,
    ];
    assert_eq!(EXPECTED_EMPTY.len(), 72, "v5 empty file pinned at 72 bytes");
    assert_eq!(
        &EXPECTED_EMPTY[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 4],
        &MAGIC
    );
    assert_eq!(
        EXPECTED_EMPTY[HEADER_VERSION_OFFSET], 5,
        "frozen v5 version byte"
    );
    let footer_start = EXPECTED_EMPTY.len() - FILE_FOOTER_SIZE;
    let footer = &EXPECTED_EMPTY[footer_start..];
    let checksum_start = FOOTER_CHECKSUM_OFFSET;
    let computed = xxh64(&footer[..checksum_start], 0);
    let mut bytes = EXPECTED_EMPTY.to_vec();
    let footer_off = bytes.len() - FILE_FOOTER_SIZE;
    bytes[footer_off + FOOTER_CHECKSUM_OFFSET..footer_off + FOOTER_CHECKSUM_OFFSET + 8]
        .copy_from_slice(&computed.to_le_bytes());
    let result = Reader::open(Cursor::new(bytes));
    match result {
        Err(FileError::UnsupportedVersion(5)) => {}
        other => panic!("expected UnsupportedVersion(5) for a frozen v5 container, got {other:?}"),
    }
}
/// Golden vector B: frozen v5, schema-source + two-message bytes,
/// hand-built. Layout pin (unchanged from the pre-v6 format):
///   header (40) | `schema_source` ("v5-test") = 7 bytes | pad to 8 = 1
///   zero byte | message 0 = "hi" (offset 48) | message 1 = "world"
///   (offset 50) | index (2 * 24 = 48 bytes, `index_offset` = 55) |
///   footer (32)
#[test]
fn golden_v5_schema_plus_two_messages_byte_literal_refused_by_v6_reader() {
    let schema_hash: u128 = 0xDEAD_BEEF_CAFE_BABE_1122_3344_5566_7788;
    let page_class: u8 = 2;
    let schema = b"v5-test";
    let msg0: &[u8] = b"hi";
    let msg1: &[u8] = b"world";
    let mut bytes = vec![0u8; FILE_HEADER_SIZE];
    bytes[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
    bytes[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2].copy_from_slice(&5u16.to_le_bytes());
    bytes[HEADER_FLAGS_OFFSET..HEADER_FLAGS_OFFSET + 2].copy_from_slice(&0u16.to_le_bytes());
    bytes[HEADER_SCHEMA_HASH_OFFSET..HEADER_SCHEMA_HASH_OFFSET + HEADER_SCHEMA_HASH_LEN]
        .copy_from_slice(&schema_hash.to_le_bytes());
    bytes[HEADER_PAGE_CLASS_OFFSET] = page_class;
    let schema_size = u32::try_from(schema.len()).unwrap();
    bytes[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4]
        .copy_from_slice(&schema_size.to_le_bytes());
    assert!(
        bytes[HEADER_RESERVED_OFFSET..HEADER_RESERVED_OFFSET + HEADER_RESERVED_LEN]
            .iter()
            .all(|&b| b == 0)
    );
    bytes.extend_from_slice(schema);
    let pad_len = pad_to_8(schema.len()) - schema.len();
    bytes.extend(std::iter::repeat_n(0u8, pad_len));
    let msg0_offset = bytes.len() as u64;
    bytes.extend_from_slice(msg0);
    let msg1_offset = bytes.len() as u64;
    bytes.extend_from_slice(msg1);
    let index_offset = bytes.len() as u64;
    let mut e0 = [0u8; INDEX_ENTRY_SIZE];
    e0[0..8].copy_from_slice(&msg0_offset.to_le_bytes());
    e0[8..12].copy_from_slice(&u32::try_from(msg0.len()).unwrap().to_le_bytes());
    e0[16..24].copy_from_slice(&xxh64(msg0, 0).to_le_bytes());
    bytes.extend_from_slice(&e0);
    let mut e1 = [0u8; INDEX_ENTRY_SIZE];
    e1[0..8].copy_from_slice(&msg1_offset.to_le_bytes());
    e1[8..12].copy_from_slice(&u32::try_from(msg1.len()).unwrap().to_le_bytes());
    e1[16..24].copy_from_slice(&xxh64(msg1, 0).to_le_bytes());
    bytes.extend_from_slice(&e1);
    let message_count: u64 = 2;
    let mut footer = [0u8; FILE_FOOTER_SIZE];
    footer[FOOTER_INDEX_OFFSET..FOOTER_INDEX_OFFSET + 8]
        .copy_from_slice(&index_offset.to_le_bytes());
    footer[FOOTER_MESSAGE_COUNT_OFFSET..FOOTER_MESSAGE_COUNT_OFFSET + 8]
        .copy_from_slice(&message_count.to_le_bytes());
    footer[FOOTER_MAGIC_OFFSET..FOOTER_MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
    let cksum = xxh64(&footer[..FOOTER_CHECKSUM_OFFSET], 0);
    footer[FOOTER_CHECKSUM_OFFSET..FOOTER_CHECKSUM_OFFSET + 8]
        .copy_from_slice(&cksum.to_le_bytes());
    bytes.extend_from_slice(&footer);
    assert_eq!(bytes.len(), 135, "frozen v5 file size pinned");
    let result = Reader::open(Cursor::new(bytes));
    match result {
        Err(FileError::UnsupportedVersion(5)) => {}
        other => panic!("expected UnsupportedVersion(5) for a frozen v5 container, got {other:?}"),
    }
}
