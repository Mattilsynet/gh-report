//! SM-2 — `pardosa_genome::file::Reader` validation paths.
//!
//! Pins GEN-0011 inline checks #13 (magic), #16 (reserved zeros), #17
//! (footer checksum), #18 (index in-bounds / monotonic / non-overlapping),
//! and #20 (`message_count * INDEX_ENTRY_SIZE` overflow). Each rejection
//! path has a dedicated test asserting the specific [`FileError`] variant;
//! no opaque "Reader rejected something" assertions.
//!
//! SM-2 scope: header + footer + index validation only. Per-message
//! payload reading and per-message xxh64 verification land in SM-3.

use std::io::Cursor;

use pardosa_genome::FileError;
use pardosa_genome::file::{Reader, Writer};
use pardosa_genome::format::{
    FILE_FOOTER_SIZE, FILE_HEADER_SIZE, FOOTER_CHECKSUM_OFFSET, FOOTER_INDEX_OFFSET,
    FOOTER_MAGIC_OFFSET, FOOTER_MESSAGE_COUNT_OFFSET, FOOTER_RESERVED_LEN, FOOTER_RESERVED_OFFSET,
    HEADER_MAGIC_OFFSET, HEADER_RESERVED_LEN, HEADER_RESERVED_OFFSET, HEADER_VERSION_OFFSET,
    INDEX_ENTRY_SIZE,
};
use xxhash_rust::xxh64::xxh64;

const KNOWN_HASH: u128 = 0x0F0E_0D0C_0B0A_0908_0706_0504_0302_0100;

/// Build a 0-message file via the SM-1 Writer.
fn zero_msg_file() -> Vec<u8> {
    let mut buf = Vec::new();
    let w = Writer::new(&mut buf, KNOWN_HASH);
    w.finish().expect("finish");
    buf
}

/// Build a 1-message file via the SM-1 Writer.
fn one_msg_file(payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut w = Writer::new(&mut buf, KNOWN_HASH);
    w.write_message(payload).expect("write_message");
    w.finish().expect("finish");
    buf
}

/// Rebuild the footer checksum after mutating bytes in `buf`. The
/// footer occupies the last 32 bytes; bytes [0..24) of the footer feed
/// the checksum.
fn refresh_footer_checksum(buf: &mut [u8]) {
    let n = buf.len();
    let footer_start = n - FILE_FOOTER_SIZE;
    let footer = &mut buf[footer_start..];
    let cksum = xxh64(&footer[..FOOTER_CHECKSUM_OFFSET], 0);
    footer[FOOTER_CHECKSUM_OFFSET..FOOTER_CHECKSUM_OFFSET + 8]
        .copy_from_slice(&cksum.to_le_bytes());
}

// ---------------------------------------------------------------------------
// Happy paths
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Rejection paths — GEN-0011 #13 (magic) at header
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Rejection paths — version (#13 family)
// ---------------------------------------------------------------------------

#[test]
fn rejects_unsupported_format_version() {
    let mut buf = zero_msg_file();
    // Bump version to 3 (any non-2).
    buf[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2].copy_from_slice(&3u16.to_le_bytes());
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(
        matches!(err, FileError::UnsupportedVersion(3)),
        "got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Rejection paths — GEN-0011 #16 (reserved zeros)
// ---------------------------------------------------------------------------

#[test]
fn rejects_nonzero_header_reserved() {
    let mut buf = zero_msg_file();
    buf[HEADER_RESERVED_OFFSET] = 0x42;
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidReserved), "got {err:?}");

    // Pin: HEADER_RESERVED_LEN bytes total; corrupting the last byte must
    // also reject.
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

// ---------------------------------------------------------------------------
// Rejection paths — GEN-0011 #17 (footer checksum)
// ---------------------------------------------------------------------------

#[test]
fn rejects_bad_footer_checksum() {
    let mut buf = zero_msg_file();
    let n = buf.len();
    let footer_start = n - FILE_FOOTER_SIZE;
    // Flip the checksum byte without recomputing.
    buf[footer_start + FOOTER_CHECKSUM_OFFSET] ^= 0xFF;
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidChecksum), "got {err:?}");
}

#[test]
fn rejects_tampered_index_offset_via_checksum_first() {
    // Mutating index_offset without recomputing the footer checksum trips
    // the checksum check before any index-geometry check fires. This
    // pins the ordering: checksum is the gate.
    let mut buf = one_msg_file(b"x");
    let n = buf.len();
    let footer_start = n - FILE_FOOTER_SIZE;
    buf[footer_start + FOOTER_INDEX_OFFSET] ^= 0x01;
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidChecksum), "got {err:?}");
}

// ---------------------------------------------------------------------------
// Rejection paths — GEN-0011 #18 (index)
// ---------------------------------------------------------------------------

#[test]
fn rejects_index_offset_below_messages_offset() {
    // Construct a 1-msg file, then rewrite the index entry's offset to
    // point inside the header region (below messages_offset). Refresh
    // the footer checksum so we exercise the #18 check, not #17.
    let mut buf = one_msg_file(b"hello-pardosa");
    let n = buf.len();
    let footer_start = n - FILE_FOOTER_SIZE;
    let index_offset = u64::from_le_bytes(
        buf[footer_start + FOOTER_INDEX_OFFSET..footer_start + FOOTER_INDEX_OFFSET + 8]
            .try_into()
            .unwrap(),
    ) as usize;
    let entry_start = index_offset;
    // Rewrite offset to 0 (well below FILE_HEADER_SIZE).
    buf[entry_start..entry_start + 8].copy_from_slice(&0u64.to_le_bytes());
    refresh_footer_checksum(&mut buf);
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidIndex), "got {err:?}");
}

#[test]
fn rejects_index_entry_extending_past_message_region() {
    // 1-msg file with a payload of size 4, then rewrite the entry's size
    // to be huge so offset+size > index_offset.
    let mut buf = one_msg_file(&[0xAA; 4]);
    let n = buf.len();
    let footer_start = n - FILE_FOOTER_SIZE;
    let index_offset = u64::from_le_bytes(
        buf[footer_start + FOOTER_INDEX_OFFSET..footer_start + FOOTER_INDEX_OFFSET + 8]
            .try_into()
            .unwrap(),
    ) as usize;
    // Bump size to u32::MAX so offset + size massively exceeds index_offset.
    buf[index_offset + 8..index_offset + 12].copy_from_slice(&u32::MAX.to_le_bytes());
    refresh_footer_checksum(&mut buf);
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidIndex), "got {err:?}");
}

#[test]
fn rejects_nonzero_index_entry_reserved() {
    let mut buf = one_msg_file(b"x");
    let n = buf.len();
    let footer_start = n - FILE_FOOTER_SIZE;
    let index_offset = u64::from_le_bytes(
        buf[footer_start + FOOTER_INDEX_OFFSET..footer_start + FOOTER_INDEX_OFFSET + 8]
            .try_into()
            .unwrap(),
    ) as usize;
    // Index entry layout: offset:8, size:4, reserved:4, checksum:8.
    buf[index_offset + 12..index_offset + 16].copy_from_slice(&1u32.to_le_bytes());
    refresh_footer_checksum(&mut buf);
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidReserved), "got {err:?}");
}

// ---------------------------------------------------------------------------
// Rejection paths — GEN-0011 #18 multi-entry (non-monotonic, overlap)
// ---------------------------------------------------------------------------

/// Build a 2-message file via the SM-1 Writer.
fn two_msg_file(p0: &[u8], p1: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut w = Writer::new(&mut buf, KNOWN_HASH);
    w.write_message(p0).expect("write_message 0");
    w.write_message(p1).expect("write_message 1");
    w.finish().expect("finish");
    buf
}

/// Read the LE u64 `index_offset` field from a constructed file's footer.
fn read_index_offset(buf: &[u8]) -> usize {
    let footer_start = buf.len() - FILE_FOOTER_SIZE;
    u64::from_le_bytes(
        buf[footer_start + FOOTER_INDEX_OFFSET..footer_start + FOOTER_INDEX_OFFSET + 8]
            .try_into()
            .unwrap(),
    ) as usize
}

#[test]
fn rejects_non_monotonic_index_entries() {
    // Two valid messages, then swap the two entries' (offset, size) pairs
    // so the second entry now points before the first. Geometry of the
    // file is still valid byte-for-byte (we didn't move payload bytes),
    // but the *index* is non-monotonic and the prev_end walk must catch it.
    let mut buf = two_msg_file(b"AAAA", b"BBBB");
    let index_offset = read_index_offset(&buf);
    // Entry layout per slot: offset:8, size:4, reserved:4, checksum:8 (24 total).
    // Swap the first 12 bytes (offset+size) of slot 0 and slot 1; keep
    // checksums in place — they're not verified in SM-2.
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
    // Two payloads of size 8. Bloat entry 0's size so its body extends
    // into entry 1's body — the prev_end walk must catch the overlap.
    let mut buf = two_msg_file(&[0xAA; 8], &[0xBB; 8]);
    let index_offset = read_index_offset(&buf);
    // Bump slot-0 size from 8 to 16, which makes slot-0 end at
    // offset_0 + 16 == offset_1 + 8 > offset_1.
    let slot0_size_at = index_offset + 8;
    buf[slot0_size_at..slot0_size_at + 4].copy_from_slice(&16u32.to_le_bytes());
    refresh_footer_checksum(&mut buf);
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::InvalidIndex), "got {err:?}");
}

// ---------------------------------------------------------------------------
// Rejection paths — GEN-0011 #20 (count × INDEX_ENTRY_SIZE overflow)
// ---------------------------------------------------------------------------

#[test]
fn rejects_message_count_overflow() {
    // Synthesise a footer whose message_count, when multiplied by
    // INDEX_ENTRY_SIZE (24), overflows u64. Need count > u64::MAX / 24.
    // The simplest tripwire: u64::MAX itself.
    let mut buf = zero_msg_file();
    let n = buf.len();
    let footer_start = n - FILE_FOOTER_SIZE;
    buf[footer_start + FOOTER_MESSAGE_COUNT_OFFSET..footer_start + FOOTER_MESSAGE_COUNT_OFFSET + 8]
        .copy_from_slice(&u64::MAX.to_le_bytes());
    refresh_footer_checksum(&mut buf);
    let err = Reader::open(Cursor::new(buf)).expect_err("must reject");
    assert!(matches!(err, FileError::IndexOverflow), "got {err:?}");
}

// ---------------------------------------------------------------------------
// Truncation / short-file paths
// ---------------------------------------------------------------------------

#[test]
fn rejects_file_shorter_than_min_size() {
    // Anything below MIN_FILE_SIZE (72) cannot contain both header and
    // footer; surface as Io or InvalidIndex — pin to *either* family, just
    // ensure we don't panic or accept it.
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
        "got {err:?}",
    );
}
