use pardosa_file::FileError;
use pardosa_file::format::{
    FILE_FOOTER_SIZE, FILE_HEADER_SIZE, FOOTER_CHECKSUM_OFFSET, FOOTER_INDEX_OFFSET,
    FOOTER_MAGIC_OFFSET, FOOTER_MESSAGE_COUNT_OFFSET, FOOTER_RESERVED_LEN, FOOTER_RESERVED_OFFSET,
    HEADER_MAGIC_OFFSET, HEADER_RESERVED_LEN, HEADER_RESERVED_OFFSET, HEADER_VERSION_OFFSET,
    INDEX_ENTRY_SIZE,
};
use pardosa_file::{Reader, Writer};
use std::io::Cursor;
use xxhash_rust::xxh64::xxh64;
const KNOWN_HASH: u128 = 0x0F0E_0D0C_0B0A_0908_0706_0504_0302_0100;
fn zero_msg_file() -> Vec<u8> {
    let mut buf = Vec::new();
    let w = Writer::new(&mut buf, KNOWN_HASH);
    w.finish().expect("finish");
    buf
}
fn one_msg_file(payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut w = Writer::new(&mut buf, KNOWN_HASH);
    w.write_message(payload).expect("write_message");
    w.finish().expect("finish");
    buf
}
fn refresh_footer_checksum(buf: &mut [u8]) {
    let n = buf.len();
    let footer_start = n - FILE_FOOTER_SIZE;
    let footer = &mut buf[footer_start..];
    let cksum = xxh64(&footer[..FOOTER_CHECKSUM_OFFSET], 0);
    footer[FOOTER_CHECKSUM_OFFSET..FOOTER_CHECKSUM_OFFSET + 8]
        .copy_from_slice(&cksum.to_le_bytes());
}
#[test]
fn opens_zero_message_writer_output() {
    let buf = zero_msg_file();
    let r = Reader::open(Cursor::new(buf)).expect("Reader::open zero-msg");
    assert_eq!(r.message_count(), 0);
    assert_eq!(r.schema_hash(), KNOWN_HASH);
    assert_eq!(r.page_class(), 0);
    assert!(r.schema_source().is_none(), "SM-2 schema_source is None");
}
#[test]
fn opens_one_message_writer_output() {
    let buf = one_msg_file(b"hello-pardosa");
    let r = Reader::open(Cursor::new(buf)).expect("Reader::open one-msg");
    assert_eq!(r.message_count(), 1);
    assert_eq!(r.schema_hash(), KNOWN_HASH);
    assert_eq!(r.page_class(), 0);
    assert!(r.schema_source().is_none());
}
#[test]
fn opens_writer_output_with_nondefault_page_class() {
    let mut buf = Vec::new();
    let w = Writer::new(&mut buf, KNOWN_HASH).with_page_class(3);
    w.finish().expect("finish");
    let r = Reader::open(Cursor::new(buf)).expect("Reader::open");
    assert_eq!(r.page_class(), 3);
}
#[test]
fn rejects_bad_header_magic() {
    let mut buf = zero_msg_file();
    buf[HEADER_MAGIC_OFFSET] ^= 0xFF;
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidMagic), "got {err:?}");
}
#[test]
fn rejects_bad_footer_magic() {
    let mut buf = zero_msg_file();
    let n = buf.len();
    let footer_start = n - FILE_FOOTER_SIZE;
    buf[footer_start + FOOTER_MAGIC_OFFSET] ^= 0xFF;
    refresh_footer_checksum(&mut buf);
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidMagic), "got {err:?}");
}
#[test]
fn rejects_unsupported_format_version() {
    let mut buf = zero_msg_file();
    buf[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2].copy_from_slice(&2u16.to_le_bytes());
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(
        matches!(err, FileError::UnsupportedVersion(2)),
        "got {err:?}"
    );
}
#[test]
fn rejects_nonzero_header_reserved() {
    let mut buf = zero_msg_file();
    buf[HEADER_RESERVED_OFFSET] = 0x42;
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidReserved), "got {err:?}");
    let mut buf = zero_msg_file();
    buf[HEADER_RESERVED_OFFSET + HEADER_RESERVED_LEN - 1] = 0x01;
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidReserved), "got {err:?}");
}
#[test]
fn rejects_nonzero_footer_reserved() {
    let mut buf = zero_msg_file();
    let n = buf.len();
    let footer_start = n - FILE_FOOTER_SIZE;
    buf[footer_start + FOOTER_RESERVED_OFFSET] = 0x42;
    refresh_footer_checksum(&mut buf);
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidReserved), "got {err:?}");
    let mut buf = zero_msg_file();
    let footer_start = buf.len() - FILE_FOOTER_SIZE;
    buf[footer_start + FOOTER_RESERVED_OFFSET + FOOTER_RESERVED_LEN - 1] = 0x01;
    refresh_footer_checksum(&mut buf);
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidReserved), "got {err:?}");
}
#[test]
fn rejects_bad_footer_checksum() {
    let mut buf = zero_msg_file();
    let n = buf.len();
    let footer_start = n - FILE_FOOTER_SIZE;
    buf[footer_start + FOOTER_CHECKSUM_OFFSET] ^= 0xFF;
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidChecksum), "got {err:?}");
}
#[test]
fn rejects_tampered_index_offset_via_checksum_first() {
    let mut buf = one_msg_file(b"x");
    let n = buf.len();
    let footer_start = n - FILE_FOOTER_SIZE;
    buf[footer_start + FOOTER_INDEX_OFFSET] ^= 0x01;
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidChecksum), "got {err:?}");
}
#[test]
fn rejects_index_offset_below_messages_offset() {
    let mut buf = one_msg_file(b"hello-pardosa");
    let index_offset = read_index_offset(&buf);
    let entry_start = index_offset;
    buf[entry_start..entry_start + 8].copy_from_slice(&0u64.to_le_bytes());
    refresh_footer_checksum(&mut buf);
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidIndex), "got {err:?}");
}
#[test]
fn rejects_index_entry_extending_past_message_region() {
    let mut buf = one_msg_file(&[0xAA; 4]);
    let index_offset = read_index_offset(&buf);
    buf[index_offset + 8..index_offset + 12].copy_from_slice(&u32::MAX.to_le_bytes());
    refresh_footer_checksum(&mut buf);
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidIndex), "got {err:?}");
}
#[test]
fn rejects_nonzero_index_entry_reserved() {
    let mut buf = one_msg_file(b"x");
    let index_offset = read_index_offset(&buf);
    buf[index_offset + 12..index_offset + 16].copy_from_slice(&1u32.to_le_bytes());
    refresh_footer_checksum(&mut buf);
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidReserved), "got {err:?}");
}
fn two_msg_file(p0: &[u8], p1: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut w = Writer::new(&mut buf, KNOWN_HASH);
    w.write_message(p0).expect("write_message 0");
    w.write_message(p1).expect("write_message 1");
    w.finish().expect("finish");
    buf
}
fn read_index_offset(buf: &[u8]) -> usize {
    let footer_start = buf.len() - FILE_FOOTER_SIZE;
    usize::try_from(u64::from_le_bytes(
        buf[footer_start + FOOTER_INDEX_OFFSET..footer_start + FOOTER_INDEX_OFFSET + 8]
            .try_into()
            .unwrap(),
    ))
    .expect("index offset fits usize on test target")
}
#[test]
fn rejects_non_monotonic_index_entries() {
    let mut buf = two_msg_file(b"AAAA", b"BBBB");
    let index_offset = read_index_offset(&buf);
    let (slot0, slot1) = (index_offset, index_offset + INDEX_ENTRY_SIZE);
    let mut tmp = [0u8; 12];
    tmp.copy_from_slice(&buf[slot0..slot0 + 12]);
    let slot1_bytes: [u8; 12] = buf[slot1..slot1 + 12].try_into().unwrap();
    buf[slot0..slot0 + 12].copy_from_slice(&slot1_bytes);
    buf[slot1..slot1 + 12].copy_from_slice(&tmp);
    refresh_footer_checksum(&mut buf);
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidIndex), "got {err:?}");
}
#[test]
fn rejects_overlapping_index_entries() {
    let mut buf = two_msg_file(&[0xAA; 8], &[0xBB; 8]);
    let index_offset = read_index_offset(&buf);
    let slot0_size_at = index_offset + 8;
    buf[slot0_size_at..slot0_size_at + 4].copy_from_slice(&16u32.to_le_bytes());
    refresh_footer_checksum(&mut buf);
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidIndex), "got {err:?}");
}
#[test]
fn rejects_message_count_overflow() {
    use pardosa_file::ReaderOptions;
    let mut buf = zero_msg_file();
    let n = buf.len();
    let footer_start = n - FILE_FOOTER_SIZE;
    buf[footer_start + FOOTER_MESSAGE_COUNT_OFFSET..footer_start + FOOTER_MESSAGE_COUNT_OFFSET + 8]
        .copy_from_slice(&u64::MAX.to_le_bytes());
    refresh_footer_checksum(&mut buf);
    let opts = ReaderOptions::default().with_max_message_count(u64::MAX);
    let err = Reader::open_with_options(Cursor::new(buf), opts).expect_err("must reject");
    assert!(matches!(err, FileError::IndexOverflow), "got {err:?}");
}
#[test]
fn rejects_file_shorter_than_min_size() {
    let short = vec![0u8; FILE_HEADER_SIZE + FILE_FOOTER_SIZE - 1];
    let err = Reader::open(Cursor::new(short)).expect_err("must reject");
    assert!(
        matches!(
            err,
            FileError::Io(_) | FileError::InvalidIndex | FileError::InvalidMagic
        ),
        "got {err:?}"
    );
}
#[test]
fn rejects_empty_file() {
    let err = Reader::open(Cursor::new(Vec::<u8>::new())).expect_err("must reject");
    assert!(
        matches!(err, FileError::Io(_) | FileError::InvalidIndex),
        "got {err:?}"
    );
}
/// W7 (roadmap correctness 2026-05-24): a tampered header advertising
/// a `schema_size` far larger than any plausible schema source must
/// be rejected with a typed [`FileError::SchemaSourceTooLarge`]
/// **before** the reader speculatively allocates a multi-GiB buffer.
/// The default cap (16 MiB) is well below the 4 GiB attacker-controlled
/// ceiling of the raw `u32` header field.
#[test]
fn rejects_oversized_schema_size_before_allocation() {
    use pardosa_file::format::HEADER_SCHEMA_SIZE_OFFSET;
    let mut buf = zero_msg_file();
    let oversized: u32 = 1_073_741_824;
    buf[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4]
        .copy_from_slice(&oversized.to_le_bytes());
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject oversized schema_size");
    match err {
        FileError::SchemaSourceTooLarge { claimed, limit } => {
            assert_eq!(claimed, oversized, "claimed must echo header bytes");
            assert!(claimed > limit, "claimed must exceed limit (got {limit})");
        }
        other => panic!("expected SchemaSourceTooLarge, got {other:?}"),
    }
}
/// W7: a configurable lower cap takes effect via
/// `ReaderOptions::with_max_schema_source_bytes`. A legitimate file
/// whose embedded schema source exceeds the caller's cap must be
/// rejected with the same typed error.
#[test]
fn rejects_schema_size_above_configured_cap() {
    use pardosa_file::ReaderOptions;
    let schema = "schema source longer than the cap configured below";
    let schema_len = schema.len();
    let mut buf = Vec::new();
    let w = Writer::new(&mut buf, KNOWN_HASH).with_schema_source(schema);
    w.finish().expect("finish");
    let cap: u32 = u32::try_from(schema_len).unwrap() - 1;
    let opts = ReaderOptions::default().with_max_schema_source_bytes(cap);
    let err = Reader::open_with_options(Cursor::new(buf), opts)
        .expect_err("must reject schema_size > configured cap");
    match err {
        FileError::SchemaSourceTooLarge { claimed, limit } => {
            assert_eq!(claimed as usize, schema_len);
            assert_eq!(limit, cap);
        }
        other => panic!("expected SchemaSourceTooLarge, got {other:?}"),
    }
}
/// W7: a tampered footer advertising a `message_count` far larger
/// than any plausible index must be rejected with
/// [`FileError::IndexTooLarge`] **before** `Vec::with_capacity` is
/// called with that count. The default cap (~44.7M entries =
/// 1 GiB / 24-byte entry) is well below `u64::MAX`.
#[test]
fn rejects_oversized_message_count_before_allocation() {
    let mut buf = zero_msg_file();
    let n = buf.len();
    let footer_start = n - FILE_FOOTER_SIZE;
    let oversized: u64 = 1_000_000_000;
    buf[footer_start + FOOTER_MESSAGE_COUNT_OFFSET..footer_start + FOOTER_MESSAGE_COUNT_OFFSET + 8]
        .copy_from_slice(&oversized.to_le_bytes());
    refresh_footer_checksum(&mut buf);
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject oversized message_count");
    match err {
        FileError::IndexTooLarge { claimed, limit } => {
            assert_eq!(claimed, oversized);
            assert!(claimed > limit);
        }
        other => panic!("expected IndexTooLarge, got {other:?}"),
    }
}
/// qf9h.5: a tampered footer whose `index_offset` is near `u64::MAX`
/// must not cause arithmetic overflow when the reader computes
/// `index_end + FILE_FOOTER_SIZE`. The prior unchecked addition would
/// panic in debug builds and silently wrap in release builds — both
/// surprises. Reader must reject with a typed `IndexOverflow` instead.
#[test]
fn rejects_index_offset_overflow_when_combined_with_footer_size() {
    let mut buf = zero_msg_file();
    let footer_start = buf.len() - FILE_FOOTER_SIZE;
    let evil_index_offset: u64 = u64::MAX;
    buf[footer_start + FOOTER_INDEX_OFFSET..footer_start + FOOTER_INDEX_OFFSET + 8]
        .copy_from_slice(&evil_index_offset.to_le_bytes());
    refresh_footer_checksum(&mut buf);
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject overflowing index_end");
    assert!(
        matches!(err, FileError::IndexOverflow | FileError::InvalidIndex),
        "got {err:?}"
    );
}
/// qf9h.5: a legitimate-shaped footer where `index_offset` lands
/// *inside* the footer region (i.e. `index_offset >= file_len -
/// FILE_FOOTER_SIZE`) must be rejected as `InvalidIndex`. Index data
/// may not overlap the footer.
#[test]
fn rejects_index_offset_overlapping_footer_region() {
    let mut buf = one_msg_file(b"x");
    let n = buf.len() as u64;
    let footer_start_u64 = n - FILE_FOOTER_SIZE as u64;
    let footer_start = buf.len() - FILE_FOOTER_SIZE;
    let evil = footer_start_u64 - 1;
    buf[footer_start + FOOTER_INDEX_OFFSET..footer_start + FOOTER_INDEX_OFFSET + 8]
        .copy_from_slice(&evil.to_le_bytes());
    refresh_footer_checksum(&mut buf);
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject footer-overlapping index");
    assert!(matches!(err, FileError::InvalidIndex), "got {err:?}");
}
/// qf9h.5: a tampered footer whose `index_offset` exceeds `file_len`
/// must be rejected as `InvalidIndex` rather than producing an
/// arithmetic surprise or attempting to seek past EOF without a
/// typed error.
#[test]
fn rejects_index_offset_past_file_end() {
    let mut buf = one_msg_file(b"hi");
    let n = buf.len() as u64;
    let footer_start = buf.len() - FILE_FOOTER_SIZE;
    let evil = n + 1024;
    buf[footer_start + FOOTER_INDEX_OFFSET..footer_start + FOOTER_INDEX_OFFSET + 8]
        .copy_from_slice(&evil.to_le_bytes());
    refresh_footer_checksum(&mut buf);
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject index past EOF");
    assert!(
        matches!(err, FileError::InvalidIndex | FileError::IndexOverflow),
        "got {err:?}"
    );
}
/// qf9h.5: file truncated below the minimum (header+footer) must be
/// rejected with a typed error before any footer arithmetic.
#[test]
fn rejects_file_truncated_within_footer() {
    let mut buf = one_msg_file(b"truncate-me");
    buf.truncate(buf.len() - 4);
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject truncated footer");
    assert!(
        matches!(
            err,
            FileError::InvalidMagic | FileError::InvalidChecksum | FileError::InvalidIndex
        ),
        "got {err:?}"
    );
}
/// W7: a configurable lower cap takes effect via
/// `ReaderOptions::with_max_message_count`. Even a legitimate file
/// whose message count exceeds the caller's cap must be rejected.
#[test]
fn rejects_message_count_above_configured_cap() {
    use pardosa_file::ReaderOptions;
    let buf = one_msg_file(b"hi");
    let opts = ReaderOptions::default().with_max_message_count(0);
    let err = Reader::open_with_options(Cursor::new(buf), opts)
        .expect_err("must reject message_count > cap=0");
    match err {
        FileError::IndexTooLarge { claimed, limit } => {
            assert_eq!(claimed, 1);
            assert_eq!(limit, 0);
        }
        other => panic!("expected IndexTooLarge, got {other:?}"),
    }
}
