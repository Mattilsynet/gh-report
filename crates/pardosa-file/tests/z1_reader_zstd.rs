//! Z1-02: Reader decompression path.
//!
//! Constructs compressed `.pgno` files by hand (raw `zstd`
//! per-message bodies + raw native structures) and drives
//! `Reader::open_with_options` / `read_message`:
//!
//! - `Reader::open` accepts `ALGO_ZSTD` when feature on.
//! - `read_message` validates stored-body checksum **before**
//!   decompression, then returns original bytes.
//! - `read_message` enforces `max_decompressed_message_bytes`.
//! - Nonzero `dict_id` still rejected on compressed files.
//!
//! No-feature counterpart (`CompressionNotAvailable`) lives
//! in `z1_no_feature_zstd.rs`. ADR-0006.
#![cfg(feature = "zstd")]
use pardosa_file::format::{
    ALGO_ZSTD, FILE_FOOTER_SIZE, FILE_HEADER_SIZE, FOOTER_CHECKSUM_OFFSET, FOOTER_INDEX_OFFSET,
    FOOTER_MAGIC_OFFSET, FOOTER_MESSAGE_COUNT_OFFSET, FORMAT_VERSION, HEADER_DICT_ID_OFFSET,
    HEADER_FLAGS_OFFSET, HEADER_MAGIC_OFFSET, HEADER_SCHEMA_HASH_OFFSET, HEADER_SCHEMA_SIZE_OFFSET,
    HEADER_VERSION_OFFSET, INDEX_ENTRY_SIZE, MAGIC, messages_offset,
};
use pardosa_file::{FileError, Reader, ReaderOptions};
use std::io::Cursor;
use xxhash_rust::xxh64::xxh64;
/// Build a `.pgno` v5 byte buffer with the supplied algo byte in flags,
/// raw header/index/footer, and pre-compressed message bodies.
/// Caller supplies `stored_bodies` already encoded as they will appear in
/// the file (typically zstd-compressed). The index uses xxh64 of stored
/// bytes per ADR-0006 §4.
fn build_raw(schema_hash: u128, algo: u8, dict_id: u32, stored_bodies: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut hdr = [0u8; FILE_HEADER_SIZE];
    hdr[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
    hdr[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2]
        .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    hdr[HEADER_FLAGS_OFFSET..HEADER_FLAGS_OFFSET + 2]
        .copy_from_slice(&u16::from(algo).to_le_bytes());
    hdr[HEADER_SCHEMA_HASH_OFFSET..HEADER_SCHEMA_HASH_OFFSET + 16]
        .copy_from_slice(&schema_hash.to_le_bytes());
    hdr[HEADER_DICT_ID_OFFSET..HEADER_DICT_ID_OFFSET + 4].copy_from_slice(&dict_id.to_le_bytes());
    hdr[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4].copy_from_slice(&[0u8; 4]);
    out.extend_from_slice(&hdr);
    let msgs_start = messages_offset(0) as u64;
    assert_eq!(msgs_start, FILE_HEADER_SIZE as u64);
    let mut cursor = msgs_start;
    let mut entries: Vec<(u64, u32, u64)> = Vec::with_capacity(stored_bodies.len());
    for body in stored_bodies {
        out.extend_from_slice(body);
        let size = u32::try_from(body.len()).expect("body size fits u32");
        let cksum = xxh64(body, 0);
        entries.push((cursor, size, cksum));
        cursor += u64::from(size);
    }
    let index_offset = cursor;
    for (offset, size, cksum) in &entries {
        let mut e = [0u8; INDEX_ENTRY_SIZE];
        e[0..8].copy_from_slice(&offset.to_le_bytes());
        e[8..12].copy_from_slice(&size.to_le_bytes());
        e[16..24].copy_from_slice(&cksum.to_le_bytes());
        out.extend_from_slice(&e);
    }
    let mut foot = [0u8; FILE_FOOTER_SIZE];
    foot[FOOTER_INDEX_OFFSET..FOOTER_INDEX_OFFSET + 8].copy_from_slice(&index_offset.to_le_bytes());
    let count = u64::try_from(stored_bodies.len()).unwrap();
    foot[FOOTER_MESSAGE_COUNT_OFFSET..FOOTER_MESSAGE_COUNT_OFFSET + 8]
        .copy_from_slice(&count.to_le_bytes());
    foot[FOOTER_MAGIC_OFFSET..FOOTER_MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
    let cksum = xxh64(&foot[..FOOTER_CHECKSUM_OFFSET], 0);
    foot[FOOTER_CHECKSUM_OFFSET..FOOTER_CHECKSUM_OFFSET + 8].copy_from_slice(&cksum.to_le_bytes());
    out.extend_from_slice(&foot);
    out
}
fn zstd_encode(payload: &[u8]) -> Vec<u8> {
    zstd::bulk::compress(payload, 0).expect("zstd compress")
}
#[test]
fn reader_opens_zstd_flagged_file_when_feature_enabled() {
    let original = b"event-payload-zstd-z1-02".repeat(8);
    let compressed = zstd_encode(&original);
    let bytes = build_raw(0xCAFE_F00D, ALGO_ZSTD, 0, &[compressed]);
    let r = Reader::open(Cursor::new(bytes)).expect("open ALGO_ZSTD file with feature");
    assert_eq!(r.message_count(), 1);
}
#[test]
fn read_message_decompresses_zstd_payload_round_trip() {
    let original = b"event-payload-zstd-z1-02".repeat(8);
    let compressed = zstd_encode(&original);
    assert_ne!(
        compressed, original,
        "test precondition: zstd output differs from input"
    );
    let bytes = build_raw(0x1234_5678, ALGO_ZSTD, 0, &[compressed]);
    let mut r = Reader::open(Cursor::new(bytes)).expect("open");
    let decoded = r.read_message(0).expect("read_message decompresses");
    assert_eq!(decoded, original);
}
#[test]
fn read_message_validates_checksum_over_stored_bytes_before_decompression() {
    let original = b"x".repeat(32);
    let mut compressed = zstd_encode(&original);
    let mut bytes = build_raw(0xAA, ALGO_ZSTD, 0, &[compressed.clone()]);
    let body_offset = FILE_HEADER_SIZE;
    bytes[body_offset] ^= 0xFF;
    let mut r = Reader::open(Cursor::new(bytes)).expect("open");
    let err = r.read_message(0).expect_err("checksum guards stored bytes");
    assert!(
        matches!(err, FileError::ChecksumMismatch(0)),
        "expected ChecksumMismatch(0); got {err:?}"
    );
    compressed[0] ^= 0x01;
    let bytes2 = build_raw(0xAA, ALGO_ZSTD, 0, &[compressed]);
    let mut r2 = Reader::open(Cursor::new(bytes2)).expect("open");
    let err2 = r2
        .read_message(0)
        .expect_err("zstd-decode of corrupt body fails");
    assert!(
        !matches!(err2, FileError::ChecksumMismatch(_)),
        "decode error must not surface as ChecksumMismatch when stored cksum is valid; got {err2:?}"
    );
}
#[test]
fn read_message_enforces_max_decompressed_cap() {
    let original = vec![0xABu8; 8 * 1024];
    let compressed = zstd_encode(&original);
    let bytes = build_raw(0xBB, ALGO_ZSTD, 0, &[compressed]);
    let opts = ReaderOptions::default().with_max_decompressed_message_bytes(64);
    let mut r = Reader::open_with_options(Cursor::new(bytes), opts).expect("open");
    let err = r.read_message(0).expect_err("cap must trip");
    assert!(
        matches!(err, FileError::DecompressedTooLarge { limit: 64 }),
        "expected DecompressedTooLarge {{ limit: 64 }}; got {err:?}"
    );
}
#[test]
fn reader_rejects_nonzero_dict_id_even_with_zstd_flag() {
    let original = b"x".repeat(8);
    let compressed = zstd_encode(&original);
    let bytes = build_raw(0xCC, ALGO_ZSTD, 1, &[compressed]);
    let err = Reader::open(Cursor::new(bytes)).expect_err("dict_id must be zero");
    assert!(
        matches!(err, FileError::InvalidReserved),
        "expected InvalidReserved for nonzero dict_id; got {err:?}"
    );
}
