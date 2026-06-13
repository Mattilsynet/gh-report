//! Z1-levels: Writer compression level selection.
//!
//! Verifies that `Compression::Zstd9` and `Compression::Zstd19`:
//!
//! - Both set the header flags low bits to `ALGO_ZSTD = 0x01`.
//! - Round-trip through `Reader` for arbitrary payloads.
//! - Produce different stored byte sequences for a sufficiently
//!   compressible payload (level 19 is at least as small as level 9
//!   in practice; we assert *inequality* of the stored bytes rather
//!   than a strict size ordering, since zstd does not formally
//!   guarantee one).
//!
//! See [ADR-0006](../../../docs/adr/0006-pgno-file-format.md).
#![cfg(feature = "zstd")]
use pardosa_file::format::{ALGO_ZSTD, FILE_FOOTER_SIZE, HEADER_FLAGS_OFFSET, messages_offset};
use pardosa_file::{Compression, Reader, Writer, WriterOptions};
use std::io::Cursor;
fn build(compression: Compression, schema_hash: u128, messages: &[&[u8]]) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    let opts = WriterOptions::default().with_compression(compression);
    let mut w = Writer::with_options(&mut buf, schema_hash, opts);
    for m in messages {
        w.write_message(m).expect("write_message");
    }
    w.finish().expect("finish");
    buf
}
fn stored_body_region(bytes: &[u8]) -> &[u8] {
    let msgs_start = messages_offset(0);
    let footer_start = bytes.len() - FILE_FOOTER_SIZE;
    let index_offset = usize::try_from(u64::from_le_bytes(
        bytes[footer_start..footer_start + 8].try_into().unwrap(),
    ))
    .expect("zstd level test index offset fits usize");
    &bytes[msgs_start..index_offset]
}
fn assert_header_algo_zstd(bytes: &[u8]) {
    let flags = u16::from_le_bytes(
        bytes[HEADER_FLAGS_OFFSET..HEADER_FLAGS_OFFSET + 2]
            .try_into()
            .unwrap(),
    );
    assert_eq!(flags & 0b111, u16::from(ALGO_ZSTD));
    assert_eq!(flags & !0b111, 0, "high flag bits reserved-zero");
}
#[test]
fn zstd9_round_trips_and_sets_algo_zstd() {
    let m0: &[u8] = b"first-message-payload-level-9";
    let m1: &[u8] = b"second-message-payload-level-9";
    let bytes = build(Compression::Zstd9, 0x9999, &[m0, m1]);
    assert_header_algo_zstd(&bytes);
    let mut r = Reader::open(Cursor::new(bytes)).expect("reader opens zstd9 file");
    assert_eq!(r.message_count(), 2);
    assert_eq!(r.read_message(0).unwrap(), m0);
    assert_eq!(r.read_message(1).unwrap(), m1);
}
#[test]
fn zstd19_round_trips_and_sets_algo_zstd() {
    let m0: &[u8] = b"first-message-payload-level-19";
    let m1: &[u8] = b"second-message-payload-level-19";
    let bytes = build(Compression::Zstd19, 0x1919, &[m0, m1]);
    assert_header_algo_zstd(&bytes);
    let mut r = Reader::open(Cursor::new(bytes)).expect("reader opens zstd19 file");
    assert_eq!(r.message_count(), 2);
    assert_eq!(r.read_message(0).unwrap(), m0);
    assert_eq!(r.read_message(1).unwrap(), m1);
}
#[test]
fn zstd9_and_zstd19_differ_on_compressible_payload() {
    let payload: Vec<u8> = b"abcdefgh".repeat(1024);
    let fast_level_bytes = build(Compression::Zstd9, 0, &[&payload]);
    let max_level_bytes = build(Compression::Zstd19, 0, &[&payload]);
    let fast_level_stored = stored_body_region(&fast_level_bytes);
    let max_level_stored = stored_body_region(&max_level_bytes);
    assert_ne!(
        fast_level_stored, max_level_stored,
        "level 9 and level 19 must produce distinct stored bytes on a compressible payload"
    );
    assert!(fast_level_stored.len() < payload.len());
    assert!(max_level_stored.len() < payload.len());
}
#[test]
fn default_writer_options_remain_uncompressed() {
    assert_eq!(WriterOptions::default().compression(), Compression::None);
}
