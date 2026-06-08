//! Z1-02 no-feature path: a file with `ALGO_ZSTD` in flags must surface
//! `FileError::CompressionNotAvailable` when the `zstd` Cargo feature is
//! not enabled in this build. Pinned outside `z1_reader_zstd.rs` so it
//! exercises the negative path under `--no-default-features`.
//!
//! See ADR-0006 and the Z1 mission brief.
#![cfg(not(feature = "zstd"))]
use pardosa_file::format::{
    ALGO_ZSTD, FILE_FOOTER_SIZE, FILE_HEADER_SIZE, FOOTER_CHECKSUM_OFFSET, FOOTER_INDEX_OFFSET,
    FOOTER_MAGIC_OFFSET, FOOTER_MESSAGE_COUNT_OFFSET, FORMAT_VERSION, HEADER_FLAGS_OFFSET,
    HEADER_MAGIC_OFFSET, HEADER_SCHEMA_SIZE_OFFSET, HEADER_VERSION_OFFSET, MAGIC, messages_offset,
};
use pardosa_file::{FileError, Reader};
use std::io::Cursor;
use xxhash_rust::xxh64::xxh64;
/// qf9h.28: the Display surface of `FileError::CompressionNotAvailable`
/// must name the missing `zstd` Cargo feature *and* mention that a
/// feature is involved, so operators can act on a log line without
/// consulting the variant name. Pinned as part of the public Display
/// contract per ADR-0007.
#[test]
fn no_feature_zstd_file_returns_compression_not_available() {
    let mut hdr = [0u8; FILE_HEADER_SIZE];
    hdr[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
    hdr[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2]
        .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    hdr[HEADER_FLAGS_OFFSET..HEADER_FLAGS_OFFSET + 2]
        .copy_from_slice(&u16::from(ALGO_ZSTD).to_le_bytes());
    hdr[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4].copy_from_slice(&[0u8; 4]);
    let mut foot = [0u8; FILE_FOOTER_SIZE];
    let idx_off = messages_offset(0) as u64;
    foot[FOOTER_INDEX_OFFSET..FOOTER_INDEX_OFFSET + 8].copy_from_slice(&idx_off.to_le_bytes());
    foot[FOOTER_MESSAGE_COUNT_OFFSET..FOOTER_MESSAGE_COUNT_OFFSET + 8].copy_from_slice(&[0u8; 8]);
    foot[FOOTER_MAGIC_OFFSET..FOOTER_MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
    let cksum = xxh64(&foot[..FOOTER_CHECKSUM_OFFSET], 0);
    foot[FOOTER_CHECKSUM_OFFSET..FOOTER_CHECKSUM_OFFSET + 8].copy_from_slice(&cksum.to_le_bytes());
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&hdr);
    bytes.extend_from_slice(&foot);
    let err = Reader::open(Cursor::new(bytes)).expect_err("must reject");
    assert!(
        matches!(err, FileError::CompressionNotAvailable),
        "expected CompressionNotAvailable; got {err:?}"
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("zstd"),
        "Display must name the missing `zstd` Cargo feature; got: {msg:?}"
    );
    assert!(
        msg.contains("feature"),
        "Display must mention that a Cargo feature is missing; got: {msg:?}"
    );
}
