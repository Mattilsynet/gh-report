//! Golden v5 byte-level vectors for PGNO files.
//!
//! These tests pin the exact bytes produced for two canonical configurations:
//! (a) no schema source, no messages, and (b) a small schema source + two
//! messages. Any drift in `MAGIC`, `FORMAT_VERSION`, header/footer layout,
//! schema-source padding, `messages_offset`, or footer checksum will flip
//! these red — without requiring a downstream consumer to notice first.
//!
//! See [ADR-0006](../../../docs/adr/0006-pgno-file-format.md) for the
//! canonical v5 file-format contract.
#![allow(
    clippy::similar_names,
    clippy::too_many_lines,
    reason = "golden vectors compose many near-twin byte buffers (header/footer/\
              schema/messages) inside a single linear assertion sequence; \
              splitting would obscure the byte-for-byte spec mirror."
)]
use pardosa_file::format::{
    FILE_FOOTER_SIZE, FILE_HEADER_SIZE, FOOTER_CHECKSUM_OFFSET, FOOTER_INDEX_OFFSET,
    FOOTER_MAGIC_OFFSET, FOOTER_MESSAGE_COUNT_OFFSET, FORMAT_VERSION, HEADER_DICT_ID_OFFSET,
    HEADER_FLAGS_OFFSET, HEADER_MAGIC_OFFSET, HEADER_PAGE_CLASS_OFFSET, HEADER_RESERVED_LEN,
    HEADER_RESERVED_OFFSET, HEADER_SCHEMA_HASH_LEN, HEADER_SCHEMA_HASH_OFFSET,
    HEADER_SCHEMA_SIZE_OFFSET, HEADER_VERSION_OFFSET, INDEX_ENTRY_SIZE, MAGIC, messages_offset,
};
use pardosa_file::{Reader, Writer};
use std::io::Cursor;
use xxhash_rust::xxh64::xxh64;
fn build(
    schema_hash: u128,
    page_class: u8,
    schema_source: Option<&str>,
    messages: &[&[u8]],
) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut w = Writer::new(&mut buf, schema_hash).with_page_class(page_class);
        if let Some(s) = schema_source {
            w = w.with_schema_source(s);
        }
        for m in messages {
            w.write_message(m).expect("write_message");
        }
        w.finish().expect("finish");
    }
    buf
}
/// Golden vector A: zero-message, zero-schema v5 file.
/// Exact bytes:
///   header (40 bytes) — magic "PGNO" | version=5 | flags=0 | `schema_hash=0..15`
///                       | `dict_id=0` | `page_class=0` | `schema_size=0` | reserved(7)=0
///   index region — empty (`message_count` = 0)
///   footer (32 bytes) — `index_offset=40` | `message_count=0` | reserved(4)=0
///                       | magic "PGNO" | xxh64(footer[..24])
#[test]
fn golden_v5_empty_file_bytes() {
    let schema_hash: u128 = 0x0F0E_0D0C_0B0A_0908_0706_0504_0302_0100;
    let bytes = build(schema_hash, 0, None, &[]);
    assert_eq!(bytes.len(), 72, "empty v5 file expected to be 72 bytes");
    assert_eq!(&bytes[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 4], &MAGIC);
    assert_eq!(
        u16::from_le_bytes(
            bytes[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2]
                .try_into()
                .unwrap()
        ),
        FORMAT_VERSION
    );
    assert_eq!(FORMAT_VERSION, 5, "FORMAT_VERSION pinned at v5");
    assert_eq!(
        u16::from_le_bytes(
            bytes[HEADER_FLAGS_OFFSET..HEADER_FLAGS_OFFSET + 2]
                .try_into()
                .unwrap()
        ),
        0,
        "flags must be 0 (no compression)"
    );
    assert_eq!(
        u128::from_le_bytes(
            bytes[HEADER_SCHEMA_HASH_OFFSET..HEADER_SCHEMA_HASH_OFFSET + HEADER_SCHEMA_HASH_LEN]
                .try_into()
                .unwrap()
        ),
        schema_hash
    );
    assert_eq!(
        u32::from_le_bytes(
            bytes[HEADER_DICT_ID_OFFSET..HEADER_DICT_ID_OFFSET + 4]
                .try_into()
                .unwrap()
        ),
        0,
        "dict_id reserved-zero in v5"
    );
    assert_eq!(bytes[HEADER_PAGE_CLASS_OFFSET], 0);
    assert_eq!(
        u32::from_le_bytes(
            bytes[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4]
                .try_into()
                .unwrap()
        ),
        0,
        "schema_size = 0 for no schema source"
    );
    assert!(
        bytes[HEADER_RESERVED_OFFSET..HEADER_RESERVED_OFFSET + HEADER_RESERVED_LEN]
            .iter()
            .all(|&b| b == 0),
        "reserved trailing 7 bytes must be zero"
    );
    let footer = &bytes[FILE_HEADER_SIZE..];
    assert_eq!(footer.len(), FILE_FOOTER_SIZE);
    let index_offset = u64::from_le_bytes(
        footer[FOOTER_INDEX_OFFSET..FOOTER_INDEX_OFFSET + 8]
            .try_into()
            .unwrap(),
    );
    assert_eq!(
        index_offset,
        messages_offset(0) as u64,
        "empty: index_offset == messages_offset(0) == FILE_HEADER_SIZE"
    );
    assert_eq!(index_offset, 40);
    assert_eq!(
        u64::from_le_bytes(
            footer[FOOTER_MESSAGE_COUNT_OFFSET..FOOTER_MESSAGE_COUNT_OFFSET + 8]
                .try_into()
                .unwrap()
        ),
        0
    );
    assert_eq!(
        &footer[FOOTER_MAGIC_OFFSET..FOOTER_MAGIC_OFFSET + 4],
        &MAGIC
    );
    let claimed_cksum = u64::from_le_bytes(
        footer[FOOTER_CHECKSUM_OFFSET..FOOTER_CHECKSUM_OFFSET + 8]
            .try_into()
            .unwrap(),
    );
    let computed = xxh64(&footer[..FOOTER_CHECKSUM_OFFSET], 0);
    assert_eq!(claimed_cksum, computed, "footer checksum pins xxh64 seed=0");
    let mut r = Reader::open(Cursor::new(bytes)).expect("reader opens golden empty file");
    assert_eq!(r.message_count(), 0);
    assert_eq!(r.schema_hash(), schema_hash);
    assert_eq!(r.page_class(), 0);
    assert_eq!(r.schema_size(), 0);
    assert_eq!(r.schema_source(), None);
    assert_eq!(r.index().len(), 0);
    assert_eq!(r.iter_messages().count(), 0);
}
/// Golden vector B: schema-source + two messages.
/// Layout pin:
///   header (40)
///   `schema_source` ("v5-test") = 7 bytes
///   pad to 8 = 1 zero byte
///   message 0 = "hi"  (size 2, offset 48)
///   message 1 = "world" (size 5, offset 50)
///   index (2 entries × 24 bytes = 48 bytes, `index_offset` = 55)
///   footer (32)
#[test]
fn golden_v5_schema_plus_two_messages() {
    let schema_hash: u128 = 0xDEAD_BEEF_CAFE_BABE_1122_3344_5566_7788;
    let page_class: u8 = 2;
    let schema = "v5-test";
    let msg0: &[u8] = b"hi";
    let msg1: &[u8] = b"world";
    let bytes = build(schema_hash, page_class, Some(schema), &[msg0, msg1]);
    let expected_total = FILE_HEADER_SIZE
        + 7
        + 1
        + msg0.len()
        + msg1.len()
        + 2 * INDEX_ENTRY_SIZE
        + FILE_FOOTER_SIZE;
    assert_eq!(expected_total, 135);
    assert_eq!(bytes.len(), expected_total);
    assert_eq!(&bytes[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 4], &MAGIC);
    assert_eq!(
        u16::from_le_bytes(
            bytes[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2]
                .try_into()
                .unwrap()
        ),
        5
    );
    assert_eq!(bytes[HEADER_PAGE_CLASS_OFFSET], page_class);
    assert_eq!(
        u32::from_le_bytes(
            bytes[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4]
                .try_into()
                .unwrap()
        ),
        u32::try_from(schema.len()).unwrap()
    );
    let schema_start = FILE_HEADER_SIZE;
    let schema_end = schema_start + schema.len();
    assert_eq!(&bytes[schema_start..schema_end], schema.as_bytes());
    assert_eq!(bytes[schema_end], 0, "schema-source pad byte must be zero");
    let msgs_start = messages_offset(u32::try_from(schema.len()).unwrap());
    assert_eq!(msgs_start, 48);
    assert_eq!(&bytes[msgs_start..msgs_start + msg0.len()], msg0);
    let msg1_start = msgs_start + msg0.len();
    assert_eq!(&bytes[msg1_start..msg1_start + msg1.len()], msg1);
    let index_offset_expected = msg1_start + msg1.len();
    assert_eq!(index_offset_expected, 55);
    let e0 = &bytes[index_offset_expected..index_offset_expected + INDEX_ENTRY_SIZE];
    assert_eq!(u64::from_le_bytes(e0[0..8].try_into().unwrap()), 48);
    assert_eq!(u32::from_le_bytes(e0[8..12].try_into().unwrap()), 2);
    assert_eq!(
        u32::from_le_bytes(e0[12..16].try_into().unwrap()),
        0,
        "index-entry reserved must be zero"
    );
    assert_eq!(
        u64::from_le_bytes(e0[16..24].try_into().unwrap()),
        xxh64(msg0, 0)
    );
    let e1_start = index_offset_expected + INDEX_ENTRY_SIZE;
    let e1 = &bytes[e1_start..e1_start + INDEX_ENTRY_SIZE];
    assert_eq!(u64::from_le_bytes(e1[0..8].try_into().unwrap()), 50);
    assert_eq!(u32::from_le_bytes(e1[8..12].try_into().unwrap()), 5);
    assert_eq!(u32::from_le_bytes(e1[12..16].try_into().unwrap()), 0);
    assert_eq!(
        u64::from_le_bytes(e1[16..24].try_into().unwrap()),
        xxh64(msg1, 0)
    );
    let footer_start = bytes.len() - FILE_FOOTER_SIZE;
    let footer = &bytes[footer_start..];
    assert_eq!(
        u64::from_le_bytes(
            footer[FOOTER_INDEX_OFFSET..FOOTER_INDEX_OFFSET + 8]
                .try_into()
                .unwrap()
        ),
        u64::try_from(index_offset_expected).unwrap()
    );
    assert_eq!(
        u64::from_le_bytes(
            footer[FOOTER_MESSAGE_COUNT_OFFSET..FOOTER_MESSAGE_COUNT_OFFSET + 8]
                .try_into()
                .unwrap()
        ),
        2
    );
    assert_eq!(
        &footer[FOOTER_MAGIC_OFFSET..FOOTER_MAGIC_OFFSET + 4],
        &MAGIC
    );
    let claimed = u64::from_le_bytes(
        footer[FOOTER_CHECKSUM_OFFSET..FOOTER_CHECKSUM_OFFSET + 8]
            .try_into()
            .unwrap(),
    );
    assert_eq!(claimed, xxh64(&footer[..FOOTER_CHECKSUM_OFFSET], 0));
    let mut r = Reader::open(Cursor::new(bytes)).expect("reader opens golden file B");
    assert_eq!(r.message_count(), 2);
    assert_eq!(r.schema_hash(), schema_hash);
    assert_eq!(r.page_class(), page_class);
    assert_eq!(r.schema_size(), u32::try_from(schema.len()).unwrap());
    assert_eq!(r.schema_source(), Some(schema));
    assert_eq!(r.read_message(0).unwrap(), msg0);
    assert_eq!(r.read_message(1).unwrap(), msg1);
}
/// Pins the documented `messages_offset` formula against concrete v5 schema sizes.
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
/// qf9h.4: canonical empty v5 file bytes, independent of
/// `Writer`/`format::*` constants. Drift flips this red.
/// Round-trips via `Reader`.
///
/// Layout (zero schema, zero messages):
///    0.. 3  "PGNO"     magic
///    4.. 5  05 00      version 5
///    6.. 7  flags
///    8..23  `schema_hash` LE
///   24..27  `dict_id`
///   28      `page_class`
///   29..32  `schema_size`
///   33..39  reserved
///   40..47  `index_offset` = 40
///   48..55  `message_count` = 0
///   56..59  footer reserved
///   60..63  footer magic
///   64..71  footer xxh64 LE
#[test]
fn golden_v5_empty_file_byte_literal() {
    const EXPECTED_EMPTY: &[u8] = &[
        0x50, 0x47, 0x4E, 0x4F, 0x05, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06,
        0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x28, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x50, 0x47, 0x4E, 0x4F, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC,
    ];
    let schema_hash: u128 = 0x0F0E_0D0C_0B0A_0908_0706_0504_0302_0100;
    let bytes = build(schema_hash, 0, None, &[]);
    let checksum_start = bytes.len() - 8;
    assert_eq!(
        &bytes[..checksum_start],
        &EXPECTED_EMPTY[..checksum_start],
        "v5 empty file body drifted from hard-coded golden vector"
    );
    let mut r = Reader::open(Cursor::new(bytes.clone())).expect("reader opens golden empty");
    assert_eq!(r.message_count(), 0);
    assert_eq!(r.schema_hash(), schema_hash);
    assert_eq!(r.iter_messages().count(), 0);
    assert_eq!(bytes.len(), 72);
}
/// qf9h.4: hard-coded canonical schema+two-message v5 file.
/// Same drift-detection role as the empty-vector golden.
///
/// Layout:
///    0.. 39  header (`page_class`=0x02, `schema_size`=7)
///   40.. 46  "v5-test" schema
///   47       00         padding
///   48.. 49  "hi"       msg 0
///   50.. 54  "world"    msg 1
///   55.. 78  index entry 0
///   79..102  index entry 1
///  103..134  footer
#[test]
fn golden_v5_schema_plus_two_messages_byte_literal() {
    const EXPECTED: &[u8] = &[
        0x50, 0x47, 0x4E, 0x4F, 0x05, 0x00, 0x00, 0x00, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22,
        0x11, 0xBE, 0xBA, 0xFE, 0xCA, 0xEF, 0xBE, 0xAD, 0xDE, 0x00, 0x00, 0x00, 0x00, 0x02, 0x07,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x76, 0x35, 0x2D, 0x74, 0x65,
        0x73, 0x74, 0x00, 0x68, 0x69, 0x77, 0x6F, 0x72, 0x6C, 0x64, 0x30, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xCC, 0xCC, 0xCC, 0xCC,
        0xCC, 0xCC, 0xCC, 0xCC, 0x32, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0x37, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x50, 0x47, 0x4E, 0x4F, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC,
    ];
    let schema_hash: u128 = 0xDEAD_BEEF_CAFE_BABE_1122_3344_5566_7788;
    let bytes = build(schema_hash, 2, Some("v5-test"), &[b"hi", b"world"]);
    assert_eq!(bytes.len(), EXPECTED.len(), "v5 file size drifted");
    let checksum_ranges: &[(usize, usize)] = &[(71, 79), (95, 103), (127, 135)];
    for (i, (expected, actual)) in EXPECTED.iter().zip(bytes.iter()).enumerate() {
        let in_checksum = checksum_ranges.iter().any(|(lo, hi)| i >= *lo && i < *hi);
        if !in_checksum {
            assert_eq!(
                *actual, *expected,
                "byte {i:#04x} drifted: got 0x{actual:02X}, expected 0x{expected:02X}"
            );
        }
    }
    let mut r = Reader::open(Cursor::new(bytes)).expect("reader opens golden schema+msgs");
    assert_eq!(r.message_count(), 2);
    assert_eq!(r.schema_hash(), schema_hash);
    assert_eq!(r.page_class(), 2);
    assert_eq!(r.schema_source(), Some("v5-test"));
    assert_eq!(r.read_message(0).unwrap(), b"hi");
    assert_eq!(r.read_message(1).unwrap(), b"world");
}
