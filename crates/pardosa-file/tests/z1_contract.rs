//! Z1-04: cross-cutting contract tests for the zstd path.
//!
//! - Unknown compression algo returns `UnsupportedCompression`.
//! - Native regions (header magic/version, footer magic, index entries)
//!   remain uncompressed in a zstd-flagged file: their bytes match the
//!   format-constant table verbatim.
//!
//! Per-feature decompression and writer behaviour live in
//! `z1_reader_zstd.rs` / `z1_writer_zstd.rs`. The no-feature negative is
//! in `z1_no_feature_zstd.rs`.
use pardosa_file::format::{
    FILE_FOOTER_SIZE, FILE_HEADER_SIZE, FOOTER_CHECKSUM_OFFSET, FOOTER_INDEX_OFFSET,
    FOOTER_MAGIC_OFFSET, FOOTER_MESSAGE_COUNT_OFFSET, FORMAT_VERSION, HEADER_FLAGS_OFFSET,
    HEADER_MAGIC_OFFSET, HEADER_SCHEMA_SIZE_OFFSET, HEADER_VERSION_OFFSET, MAGIC, messages_offset,
};
use pardosa_file::{FileError, Reader};
use std::io::Cursor;
use xxhash_rust::xxh64::xxh64;
#[test]
fn unknown_compression_algo_returns_unsupported_compression() {
    let mut hdr = [0u8; FILE_HEADER_SIZE];
    hdr[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
    hdr[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2]
        .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    hdr[HEADER_FLAGS_OFFSET..HEADER_FLAGS_OFFSET + 2].copy_from_slice(&2u16.to_le_bytes());
    hdr[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4].copy_from_slice(&[0u8; 4]);
    let mut foot = [0u8; FILE_FOOTER_SIZE];
    let idx_off = u64::try_from(messages_offset(0)).expect("messages_offset(0) fits u64");
    foot[FOOTER_INDEX_OFFSET..FOOTER_INDEX_OFFSET + 8].copy_from_slice(&idx_off.to_le_bytes());
    foot[FOOTER_MESSAGE_COUNT_OFFSET..FOOTER_MESSAGE_COUNT_OFFSET + 8].copy_from_slice(&[0u8; 8]);
    foot[FOOTER_MAGIC_OFFSET..FOOTER_MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
    let cksum = xxh64(&foot[..FOOTER_CHECKSUM_OFFSET], 0);
    foot[FOOTER_CHECKSUM_OFFSET..FOOTER_CHECKSUM_OFFSET + 8].copy_from_slice(&cksum.to_le_bytes());
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&hdr);
    bytes.extend_from_slice(&foot);
    let err = Reader::open(Cursor::new(bytes)).expect_err("algo 0x02 unknown");
    assert!(
        matches!(err, FileError::UnsupportedCompression(2)),
        "expected UnsupportedCompression(2); got {err:?}"
    );
}
#[test]
fn high_flag_bits_are_reserved_zero() {
    let mut hdr = [0u8; FILE_HEADER_SIZE];
    hdr[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
    hdr[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2]
        .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    hdr[HEADER_FLAGS_OFFSET..HEADER_FLAGS_OFFSET + 2].copy_from_slice(&8u16.to_le_bytes());
    hdr[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4].copy_from_slice(&[0u8; 4]);
    let mut foot = [0u8; FILE_FOOTER_SIZE];
    let idx_off = u64::try_from(messages_offset(0)).expect("messages_offset(0) fits u64");
    foot[FOOTER_INDEX_OFFSET..FOOTER_INDEX_OFFSET + 8].copy_from_slice(&idx_off.to_le_bytes());
    foot[FOOTER_MESSAGE_COUNT_OFFSET..FOOTER_MESSAGE_COUNT_OFFSET + 8].copy_from_slice(&[0u8; 8]);
    foot[FOOTER_MAGIC_OFFSET..FOOTER_MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
    let cksum = xxh64(&foot[..FOOTER_CHECKSUM_OFFSET], 0);
    foot[FOOTER_CHECKSUM_OFFSET..FOOTER_CHECKSUM_OFFSET + 8].copy_from_slice(&cksum.to_le_bytes());
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&hdr);
    bytes.extend_from_slice(&foot);
    let err = Reader::open(Cursor::new(bytes)).expect_err("high bits reserved");
    assert!(
        matches!(err, FileError::UnsupportedCompression(_)),
        "expected UnsupportedCompression for high reserved flag bits; got {err:?}"
    );
}
/// Native regions of a `.pgno` v5 file are never compressed, regardless of
/// payload-body algorithm. Specifically: header magic, header version,
/// schema-size field, footer magic, and the index-entry layout all sit
/// at the exact byte offsets pinned in `format.rs`.
#[cfg(feature = "zstd")]
#[test]
fn native_regions_stay_uncompressed_under_algo_zstd() {
    use pardosa_file::format::{FOOTER_RESERVED_LEN, FOOTER_RESERVED_OFFSET, INDEX_ENTRY_SIZE};
    use pardosa_file::{Compression, Writer, WriterOptions};
    let mut buf: Vec<u8> = Vec::new();
    {
        let opts = WriterOptions::default().with_compression(Compression::Zstd9);
        let mut w = Writer::with_options(&mut buf, 0xDEAD_BEEF, opts);
        w.write_message(b"compressible-redundant-redundant-redundant")
            .unwrap();
        w.finish().unwrap();
    }
    assert_eq!(&buf[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 4], &MAGIC);
    assert_eq!(
        u16::from_le_bytes(
            buf[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2]
                .try_into()
                .unwrap()
        ),
        FORMAT_VERSION
    );
    let footer_start = buf.len() - FILE_FOOTER_SIZE;
    let footer = &buf[footer_start..];
    assert_eq!(
        &footer[FOOTER_MAGIC_OFFSET..FOOTER_MAGIC_OFFSET + 4],
        &MAGIC
    );
    assert!(
        footer[FOOTER_RESERVED_OFFSET..FOOTER_RESERVED_OFFSET + FOOTER_RESERVED_LEN]
            .iter()
            .all(|&b| b == 0)
    );
    let claimed = u64::from_le_bytes(
        footer[FOOTER_CHECKSUM_OFFSET..FOOTER_CHECKSUM_OFFSET + 8]
            .try_into()
            .unwrap(),
    );
    let computed = xxh64(&footer[..FOOTER_CHECKSUM_OFFSET], 0);
    assert_eq!(claimed, computed);
    let index_offset = usize::try_from(u64::from_le_bytes(
        footer[FOOTER_INDEX_OFFSET..FOOTER_INDEX_OFFSET + 8]
            .try_into()
            .unwrap(),
    ))
    .expect("golden zstd index offset fits usize");
    let entry = &buf[index_offset..index_offset + INDEX_ENTRY_SIZE];
    assert_eq!(
        u32::from_le_bytes(entry[12..16].try_into().unwrap()),
        0,
        "index-entry reserved u32 must remain zero under zstd"
    );
}
