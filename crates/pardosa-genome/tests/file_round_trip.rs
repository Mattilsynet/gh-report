//! SM-3 — multi-message round-trip + per-message xxh64 integrity.
//!
//! Pins:
//!  * GEN-0011 #14: per-message xxh64 verified at read time.
//!  * GEN-0016 R1: xxh64(seed=0), full 64 bits.
//!  * End-to-end Writer→Reader byte-identical round-trip across multiple
//!    messages with distinct sizes (incl. size-1 and a multiple-of-8).
//!  * messages_offset() helper used (no pad-arithmetic duplication —
//!    SM-3 doesn't touch schema, so messages_offset(0) = 40, but the
//!    Reader path must still derive body offsets from index entries
//!    rather than assuming offset 40, which we exercise indirectly via
//!    distinct payload sizes).
//!
//! SM-3 scope: opaque-bytes round-trip. Per-message *decode* via
//! pardosa-encoding is out of scope (per package L303); SM-3 works on
//! arbitrary `&[u8]` bodies.

use std::io::Cursor;

use pardosa_genome::FileError;
use pardosa_genome::file::{Reader, Writer};
use pardosa_genome::format::FILE_FOOTER_SIZE;
use xxhash_rust::xxh64::xxh64;

const KNOWN_HASH: u128 = 0x0F0E_0D0C_0B0A_0908_0706_0504_0302_0100;

/// Three messages spanning size-1, a multiple-of-8, and an
/// intentionally awkward odd length.
fn fixture_payloads() -> Vec<Vec<u8>> {
    vec![
        b"A".to_vec(),                  // size 1
        b"hello-pardosa!!".to_vec(),    // size 15 (odd, not multiple of 8)
        (0..16u8).collect::<Vec<u8>>(), // size 16 (multiple of 8), distinct content
    ]
}

/// Build a file with the given payloads via SM-1 Writer.
fn build_file(payloads: &[Vec<u8>]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut w = Writer::new(&mut buf, KNOWN_HASH);
        for p in payloads {
            w.write_message(p).expect("write_message");
        }
        w.finish().expect("finish");
    }
    buf
}

// ---------------------------------------------------------------------------
// Happy path — multi-message round-trip
// ---------------------------------------------------------------------------

#[test]
fn multi_message_round_trip_byte_identical() {
    let payloads = fixture_payloads();
    let file = build_file(&payloads);

    let mut r = Reader::open(Cursor::new(&file)).expect("Reader::open");
    assert_eq!(r.message_count(), payloads.len() as u64);
    assert_eq!(r.schema_hash(), KNOWN_HASH);

    // read_message yields each body byte-identically, in order.
    for (i, expected) in payloads.iter().enumerate() {
        let got = r.read_message(i).expect("read_message");
        assert_eq!(&got, expected, "payload {i} mismatch");
    }
}

#[test]
fn iter_messages_yields_payloads_in_order() {
    let payloads = fixture_payloads();
    let file = build_file(&payloads);

    let mut r = Reader::open(Cursor::new(&file)).expect("Reader::open");
    let collected: Vec<Vec<u8>> = r
        .iter_messages()
        .collect::<Result<_, _>>()
        .expect("iter_messages");
    assert_eq!(collected, payloads);
}

#[test]
fn zero_message_round_trip_iter_is_empty() {
    let file = {
        let mut buf = Vec::new();
        let w = Writer::new(&mut buf, KNOWN_HASH);
        w.finish().expect("finish");
        buf
    };
    let mut r = Reader::open(Cursor::new(file)).expect("Reader::open");
    assert_eq!(r.message_count(), 0);
    let collected: Vec<Vec<u8>> = r.iter_messages().collect::<Result<_, _>>().expect("iter");
    assert!(collected.is_empty());
}

#[test]
fn read_message_pins_per_message_xxh64_seed_zero() {
    // Independent of the Reader: hand-verify that the body checksum that
    // the index entry carries equals xxh64(body, 0). This pins GEN-0016 R1
    // at the test layer, not just inside the Reader.
    let payloads = fixture_payloads();
    let file = build_file(&payloads);
    let r = Reader::open(Cursor::new(&file)).expect("open");
    for (i, p) in payloads.iter().enumerate() {
        let entry = r.index()[i];
        assert_eq!(entry.checksum, xxh64(p, 0), "entry {i} checksum");
        assert_eq!(entry.size as usize, p.len(), "entry {i} size");
    }
}

// ---------------------------------------------------------------------------
// Rejection path — per-message checksum mismatch (GEN-0011 #14)
// ---------------------------------------------------------------------------

#[test]
fn rejects_per_message_checksum_mismatch_on_body_corruption() {
    let payloads = fixture_payloads();
    let mut file = build_file(&payloads);

    // Reader passes open-time validation (header/footer/index all sound).
    let r = Reader::open(Cursor::new(&file)).expect("Reader::open pre-corruption");
    let body_offset = r.index()[1].offset as usize;
    let body_size = r.index()[1].size as usize;
    drop(r);

    // Corrupt one byte in the body of message #1 (the 15-byte payload).
    // Flip a middle byte so we don't accidentally land at a boundary.
    file[body_offset + body_size / 2] ^= 0xFF;

    let mut r = Reader::open(Cursor::new(&file)).expect("Reader::open post-corruption");
    // Messages 0 and 2 still verify; message 1 must fail with
    // ChecksumMismatch(1).
    r.read_message(0).expect("msg 0 still verifies");
    let err = r.read_message(1).expect_err("msg 1 must reject");
    assert!(matches!(err, FileError::ChecksumMismatch(1)), "got {err:?}");
    r.read_message(2).expect("msg 2 still verifies");
}

#[test]
fn rejects_per_message_checksum_mismatch_on_index_checksum_corruption() {
    // Inverse: leave the body alone, corrupt the recorded checksum in
    // the index entry. The Reader must still flag mismatch, and the
    // *footer* checksum must be refreshed so we exercise the per-message
    // path, not the open-time #17 path.
    let payloads = fixture_payloads();
    let mut file = build_file(&payloads);
    let r = Reader::open(Cursor::new(&file)).expect("open");
    // Index entry layout: offset:8, size:4, reserved:4, checksum:8.
    // We need the absolute byte offset of entry-1's checksum field.
    let index_start = {
        let n = file.len();
        let footer_start = n - FILE_FOOTER_SIZE;
        u64::from_le_bytes(file[footer_start..footer_start + 8].try_into().unwrap()) as usize
    };
    drop(r);
    let entry1_cksum_at = index_start + 24 /* entry 0 */ + 16 /* offset+size+reserved */;
    file[entry1_cksum_at] ^= 0x01;
    // Refresh footer checksum so #17 doesn't fire first.
    let n = file.len();
    let footer_start = n - FILE_FOOTER_SIZE;
    let footer = &mut file[footer_start..];
    let cksum = xxh64(&footer[..24], 0);
    footer[24..32].copy_from_slice(&cksum.to_le_bytes());

    let mut r = Reader::open(Cursor::new(&file)).expect("Reader::open post-corruption");
    let err = r.read_message(1).expect_err("must reject");
    assert!(matches!(err, FileError::ChecksumMismatch(1)), "got {err:?}");
}

#[test]
fn read_message_out_of_bounds_returns_invalid_index() {
    let payloads = fixture_payloads();
    let file = build_file(&payloads);
    let mut r = Reader::open(Cursor::new(file)).expect("open");
    let err = r.read_message(payloads.len()).expect_err("oob");
    assert!(matches!(err, FileError::InvalidIndex), "got {err:?}");
}
