//! Z1-03: Writer compression path.
//!
//! A `Writer` with `Compression::Zstd9`:
//!
//! - Sets header flags low bits to `ALGO_ZSTD = 0x01`.
//! - Compresses only body bytes; stored bytes differ from
//!   input for non-trivial payloads.
//! - Records `offset`/`size`/`checksum` over stored
//!   (compressed) bytes (ADR-0006 §4).
//! - Leaves `schema_source` raw UTF-8 + zero padding.
//! - Round-trips via matching `Reader`.
//!
//! Level 9 is representative; level-19 + differentials live
//! in `z1_writer_zstd_levels.rs`. See ADR-0006.
#![allow(
    clippy::cast_possible_truncation,
    reason = "test code reads small fixed-size u64 header fields known to fit \
              in usize on the target platforms exercised in CI."
)]
#![cfg(feature = "zstd")]
use pardosa_file::format::{
    ALGO_ZSTD, FILE_FOOTER_SIZE, FILE_HEADER_SIZE, HEADER_FLAGS_OFFSET, HEADER_PAGE_CLASS_OFFSET,
    INDEX_ENTRY_SIZE, messages_offset,
};
use pardosa_file::{Compression, Reader, Writer, WriterOptions};
use std::io::Cursor;
use xxhash_rust::xxh64::xxh64;
fn build_zstd(schema_hash: u128, schema_source: Option<&str>, messages: &[&[u8]]) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    let opts = WriterOptions::default().with_compression(Compression::Zstd9);
    let mut w = Writer::with_options(&mut buf, schema_hash, opts);
    if let Some(s) = schema_source {
        w = w.with_schema_source(s);
    }
    for m in messages {
        w.write_message(m).expect("write_message");
    }
    w.finish().expect("finish");
    buf
}
#[test]
fn writer_zstd_sets_algo_zstd_flag_in_header() {
    let bytes = build_zstd(0xAB, None, &[b"hello-zstd-payload"]);
    let flags = u16::from_le_bytes(
        bytes[HEADER_FLAGS_OFFSET..HEADER_FLAGS_OFFSET + 2]
            .try_into()
            .unwrap(),
    );
    assert_eq!(flags & 0b111, u16::from(ALGO_ZSTD));
    assert_eq!(flags & !0b111, 0, "high flag bits reserved-zero");
}
#[test]
fn writer_zstd_stored_body_differs_from_input_for_non_trivial_payload() {
    let payload: Vec<u8> = b"abcdefgh".repeat(128);
    let bytes = build_zstd(0xCD, None, &[&payload]);
    let msgs_start = messages_offset(0);
    let footer_start = bytes.len() - FILE_FOOTER_SIZE;
    let index_offset =
        u64::from_le_bytes(bytes[footer_start..footer_start + 8].try_into().unwrap()) as usize;
    let stored = &bytes[msgs_start..index_offset];
    assert_ne!(stored, payload.as_slice(), "zstd-stored body must differ");
    assert!(
        stored.len() < payload.len(),
        "redundant payload must compress: stored={}, raw={}",
        stored.len(),
        payload.len()
    );
}
#[test]
fn writer_zstd_index_size_and_checksum_refer_to_stored_bytes() {
    let payload: Vec<u8> = b"xyz".repeat(64);
    let bytes = build_zstd(0xEE, None, &[&payload]);
    let msgs_start = messages_offset(0);
    let footer_start = bytes.len() - FILE_FOOTER_SIZE;
    let index_offset =
        u64::from_le_bytes(bytes[footer_start..footer_start + 8].try_into().unwrap()) as usize;
    let stored = &bytes[msgs_start..index_offset];
    let e0 = &bytes[index_offset..index_offset + INDEX_ENTRY_SIZE];
    let entry_offset = u64::from_le_bytes(e0[0..8].try_into().unwrap()) as usize;
    let entry_size = u32::from_le_bytes(e0[8..12].try_into().unwrap()) as usize;
    let entry_cksum = u64::from_le_bytes(e0[16..24].try_into().unwrap());
    assert_eq!(entry_offset, msgs_start);
    assert_eq!(entry_size, stored.len(), "size is stored-bytes length");
    assert_eq!(
        entry_cksum,
        xxh64(stored, 0),
        "checksum is over stored bytes"
    );
}
#[test]
fn writer_zstd_schema_source_is_raw_utf8_plus_zero_padding() {
    let schema = "raw-utf8-schema";
    let bytes = build_zstd(0xFF, Some(schema), &[b"any"]);
    let schema_start = FILE_HEADER_SIZE;
    let schema_end = schema_start + schema.len();
    assert_eq!(&bytes[schema_start..schema_end], schema.as_bytes());
    let pad_len = ((schema.len() + 7) & !7) - schema.len();
    assert!(
        bytes[schema_end..schema_end + pad_len]
            .iter()
            .all(|&b| b == 0),
        "schema_source padding must be zero"
    );
}
#[test]
fn writer_zstd_round_trip_via_reader() {
    let m0: &[u8] = b"first-message-payload";
    let m1: &[u8] = b"second-message-payload";
    let bytes = build_zstd(0x77, Some("schema"), &[m0, m1]);
    let mut r = Reader::open(Cursor::new(bytes)).expect("reader opens zstd-written file");
    assert_eq!(r.message_count(), 2);
    assert_eq!(r.schema_source(), Some("schema"));
    assert_eq!(r.read_message(0).unwrap(), m0);
    assert_eq!(r.read_message(1).unwrap(), m1);
}
#[test]
fn writer_zstd_with_page_class_preserves_page_class_byte() {
    let bytes = build_zstd(0x11, None, &[b"p"]);
    assert_eq!(bytes[HEADER_PAGE_CLASS_OFFSET], 0);
}
