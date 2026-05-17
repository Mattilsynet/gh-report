//! SM-4 — schema-source block embedding (GEN-0009 R2).
//!
//! End-to-end tests for the optional `schema_source` block sitting between
//! the 40-byte file header and the first message body. Per `format.rs:33-35`,
//! when `schema_size > 0` a `schema_size`-byte UTF-8 region follows the
//! header and is then zero-padded to an 8-byte boundary; messages begin at
//! `messages_offset(schema_size)`.
//!
//! The Writer surface adds a builder rung `.with_schema_source(s)`; the
//! Reader exposes `schema_source() -> Option<&str>`. These tests pin:
//!  * Header `schema_size` (u32 LE @ offset 29) equals the UTF-8 byte length.
//!  * The block bytes match `s.as_bytes()` exactly at file offset 40.
//!  * Padding bytes (between `schema_size` end and the next 8B boundary) are zero.
//!  * Messages physically start at `messages_offset(schema_size)`.
//!  * Reader.schema_source() returns `Some(s)` (UTF-8 round-trip).
//!  * `schema_source = None` produces a file byte-identical to the SM-3 baseline.
//!  * A size that is NOT a multiple of 8 (13, 17, 41) round-trips correctly.
//!
//! Pad-arithmetic is the named failure mode (abort_if #1): the size-13 case
//! and the size-17 case together expose any off-by-pad bug — both need the
//! pad bytes and the messages_offset shift to be exact.

use std::io::Cursor;

use pardosa_genome::FileError;
use pardosa_genome::file::{Reader, Writer};
use pardosa_genome::format::{
    FILE_FOOTER_SIZE, FILE_HEADER_SIZE, FOOTER_CHECKSUM_OFFSET, HEADER_SCHEMA_SIZE_OFFSET,
    messages_offset, pad_to_8,
};
use xxhash_rust::xxh64::xxh64;

const SCHEMA_HASH: u128 = 0xDEAD_BEEF_FEED_FACE_1234_5678_90AB_CDEFu128;

/// Build a file with `schema_source` set (or unset when `schema` is None) and
/// `messages` written in order. Returns the raw file bytes.
fn build_file(schema: Option<&str>, messages: &[&[u8]]) -> Vec<u8> {
    let mut sink: Vec<u8> = Vec::new();
    {
        let mut w = Writer::new(&mut sink, SCHEMA_HASH);
        if let Some(s) = schema {
            w = w.with_schema_source(s);
        }
        for m in messages {
            w.write_message(m).expect("write_message");
        }
        w.finish().expect("finish");
    }
    sink
}

#[test]
fn schema_size_eq_zero_round_trip_matches_sm3_baseline() {
    // Regression on SM-3: schema_source = None must produce the same file
    // as before SM-4. Same writer construction, no .with_schema_source call.
    let payloads: &[&[u8]] = &[b"alpha", b"beta-message", b"gamma"];

    let no_schema = build_file(None, payloads);

    // Header schema_size LE u32 must be 0; first message must be at offset 40.
    let schema_size = u32::from_le_bytes(
        no_schema[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4]
            .try_into()
            .unwrap(),
    );
    assert_eq!(schema_size, 0, "schema_size must be 0 when source is None");
    assert_eq!(messages_offset(0), FILE_HEADER_SIZE);
    assert_eq!(
        &no_schema[FILE_HEADER_SIZE..FILE_HEADER_SIZE + 5],
        b"alpha",
        "first message must start at offset 40 when schema_size = 0"
    );

    // Reader round-trip: schema_source() is None.
    let mut r = Reader::open(Cursor::new(&no_schema)).expect("open");
    assert_eq!(r.schema_source(), None);
    assert_eq!(r.schema_size(), 0);
    assert_eq!(r.message_count(), 3);
    let got: Vec<Vec<u8>> = r
        .iter_messages()
        .collect::<Result<_, _>>()
        .expect("iter_messages");
    assert_eq!(got, vec![payloads[0], payloads[1], payloads[2]]);
}

#[test]
fn schema_size_13_round_trips_with_zero_pad_to_16() {
    // 13 bytes is the canonical "not a multiple of 8" case from the brief.
    // pad_to_8(13) == 16, so 3 bytes of zero-pad follow the block, and
    // messages_offset(13) == 40 + 16 == 56.
    let schema = "struct X { y }"; // ascii, 14 bytes — bump down to 13
    let schema = &schema[..13];
    assert_eq!(schema.len(), 13);
    let payloads: &[&[u8]] = &[b"alpha", b"beta-message", b"gamma"];

    let file = build_file(Some(schema), payloads);

    // schema_size LE u32 == 13.
    let schema_size = u32::from_le_bytes(
        file[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4]
            .try_into()
            .unwrap(),
    );
    assert_eq!(schema_size, 13);

    // Block bytes at [40 .. 53] match schema verbatim.
    assert_eq!(
        &file[FILE_HEADER_SIZE..FILE_HEADER_SIZE + 13],
        schema.as_bytes(),
        "schema block bytes must match input UTF-8 verbatim"
    );

    // Pad bytes at [53 .. 56] must all be zero.
    let pad_start = FILE_HEADER_SIZE + 13;
    let pad_end = FILE_HEADER_SIZE + pad_to_8(13);
    assert_eq!(pad_end - pad_start, 3, "pad_to_8(13) - 13 == 3");
    assert!(
        file[pad_start..pad_end].iter().all(|&b| b == 0),
        "schema-block pad bytes must be zero, got {:?}",
        &file[pad_start..pad_end]
    );

    // First message body starts exactly at messages_offset(13) == 56.
    let msgs_start = messages_offset(13);
    assert_eq!(msgs_start, 56);
    assert_eq!(&file[msgs_start..msgs_start + 5], b"alpha");

    // Reader round-trip.
    let mut r = Reader::open(Cursor::new(&file)).expect("open");
    assert_eq!(r.schema_source(), Some(schema));
    assert_eq!(r.schema_size(), 13);
    assert_eq!(r.message_count(), 3);
    let got: Vec<Vec<u8>> = r
        .iter_messages()
        .collect::<Result<_, _>>()
        .expect("iter_messages");
    assert_eq!(got, vec![payloads[0], payloads[1], payloads[2]]);
}

#[test]
fn schema_size_17_round_trips_with_zero_pad_to_24() {
    // Second "not a multiple of 8" case at a different residue class
    // (17 mod 8 == 1, where 13 mod 8 == 5). Catches a class of pad bugs
    // that only show up at certain residues.
    let schema = "struct Y { a: u8 }"; // 18 bytes — slice down to 17
    let schema = &schema[..17];
    assert_eq!(schema.len(), 17);
    let payloads: &[&[u8]] = &[b"only-message"];

    let file = build_file(Some(schema), payloads);

    let schema_size = u32::from_le_bytes(
        file[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4]
            .try_into()
            .unwrap(),
    );
    assert_eq!(schema_size, 17);
    assert_eq!(pad_to_8(17), 24);
    assert_eq!(messages_offset(17), 64);
    assert_eq!(
        &file[FILE_HEADER_SIZE..FILE_HEADER_SIZE + 17],
        schema.as_bytes()
    );
    // Pad: [57..64].
    assert!(
        file[FILE_HEADER_SIZE + 17..FILE_HEADER_SIZE + 24]
            .iter()
            .all(|&b| b == 0)
    );
    assert_eq!(&file[64..64 + 12], b"only-message");

    let mut r = Reader::open(Cursor::new(&file)).expect("open");
    assert_eq!(r.schema_source(), Some(schema));
    let got: Vec<Vec<u8>> = r.iter_messages().collect::<Result<_, _>>().unwrap();
    assert_eq!(got, vec![b"only-message".to_vec()]);
}

#[test]
fn schema_size_multiple_of_8_has_no_pad_bytes() {
    // pad_to_8(40) == 40, so zero extra bytes follow the block.
    let schema = "a".repeat(40);
    let payloads: &[&[u8]] = &[b"m0", b"m1"];

    let file = build_file(Some(&schema), payloads);

    let schema_size = u32::from_le_bytes(
        file[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4]
            .try_into()
            .unwrap(),
    );
    assert_eq!(schema_size, 40);
    assert_eq!(pad_to_8(40), 40, "exact-multiple is its own pad target");
    assert_eq!(messages_offset(40), 80);
    assert_eq!(
        &file[FILE_HEADER_SIZE..FILE_HEADER_SIZE + 40],
        schema.as_bytes()
    );
    // First message body sits at offset 80 — no gap bytes.
    assert_eq!(&file[80..82], b"m0");

    let r = Reader::open(Cursor::new(&file)).expect("open");
    assert_eq!(r.schema_source(), Some(schema.as_str()));
}

#[test]
fn empty_schema_string_writes_block_of_size_zero() {
    // GEN-0009 R2: the block is optional. An empty &str is still "Some",
    // but len() == 0, so schema_size == 0 and messages_offset == 40. This
    // is observably indistinguishable from None on the wire — by design.
    let file = build_file(Some(""), &[b"x"]);
    let schema_size = u32::from_le_bytes(
        file[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4]
            .try_into()
            .unwrap(),
    );
    assert_eq!(schema_size, 0);
    assert_eq!(&file[FILE_HEADER_SIZE..FILE_HEADER_SIZE + 1], b"x");

    // Reader policy: schema_size == 0 → schema_source() == None (no block
    // to expose). An empty-string round-trip is *not* required by spec.
    let r = Reader::open(Cursor::new(&file)).expect("open");
    assert_eq!(r.schema_source(), None);
    assert_eq!(r.schema_size(), 0);
}

#[test]
fn schema_block_with_multibyte_utf8_round_trips() {
    // GEN-0009 R2 says "plain UTF-8". Pin that non-ASCII bytes survive.
    let schema = "// café — π ≈ 3.14"; // multi-byte chars; 22 bytes UTF-8
    let payloads: &[&[u8]] = &[b"hello"];

    let file = build_file(Some(schema), payloads);

    let schema_size = u32::from_le_bytes(
        file[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4]
            .try_into()
            .unwrap(),
    );
    assert_eq!(schema_size as usize, schema.len());
    assert_eq!(
        &file[FILE_HEADER_SIZE..FILE_HEADER_SIZE + schema.len()],
        schema.as_bytes()
    );

    let mut r = Reader::open(Cursor::new(&file)).expect("open");
    assert_eq!(r.schema_source(), Some(schema));
    let got: Vec<Vec<u8>> = r.iter_messages().collect::<Result<_, _>>().unwrap();
    assert_eq!(got, vec![b"hello".to_vec()]);
}

#[test]
fn rejects_non_utf8_schema_block() {
    // GEN-0009 R2 mandates UTF-8 for the schema block. A producer that
    // emits arbitrary bytes there is a wire violation; Reader::open must
    // surface it as FileError::InvalidSchemaSource (kept distinct from
    // DeError per GEN-0026 R3).
    //
    // Construction: start from a valid file whose schema_source is 7
    // bytes of clean ASCII ("ok-utf8"), then overwrite those 7 bytes
    // in-place with an invalid UTF-8 sequence. 0xFF and 0xFE are never
    // valid as a UTF-8 lead byte (RFC 3629 §3), so the very first
    // String::from_utf8 advance over the block must fail.
    //
    // The header's schema_size is unchanged (= 7), so Reader reads
    // exactly the corrupted bytes. We then refresh the footer checksum
    // — the schema block is NOT covered by the footer checksum (footer
    // checksum only spans footer[0..24]), so technically we could skip
    // this, but doing it explicitly pins that the rejection path is
    // InvalidSchemaSource and not InvalidChecksum.
    let mut file = build_file(Some("ok-utf8"), &[b"payload"]);

    // Sanity: open works pre-corruption.
    Reader::open(Cursor::new(&file)).expect("baseline file is well-formed");

    // Overwrite the 7-byte schema region with a non-UTF-8 sequence.
    // 0xFF is a forbidden lead byte; 0xC0/0xC1 are over-long forms;
    // a lone 0x80 is a stray continuation byte. Mix all three so the
    // failure surfaces regardless of which byte the validator hits first.
    let bad: [u8; 7] = [0xFF, 0xFE, 0xC0, 0xC1, 0x80, 0xFF, 0x80];
    file[FILE_HEADER_SIZE..FILE_HEADER_SIZE + 7].copy_from_slice(&bad);

    // Refresh footer checksum (xxh64 seed=0 over footer[0..24]) so we
    // pin the UTF-8 check as the failure cause, not InvalidChecksum.
    // Reuses the writer.rs::write_footer / file_reader::refresh_footer_checksum pattern.
    let n = file.len();
    let footer_start = n - FILE_FOOTER_SIZE;
    let footer = &mut file[footer_start..];
    let cksum = xxh64(&footer[..FOOTER_CHECKSUM_OFFSET], 0);
    footer[FOOTER_CHECKSUM_OFFSET..FOOTER_CHECKSUM_OFFSET + 8]
        .copy_from_slice(&cksum.to_le_bytes());

    let err = Reader::open(Cursor::new(&file)).expect_err("must reject non-UTF-8 schema");
    assert!(
        matches!(err, FileError::InvalidSchemaSource),
        "expected FileError::InvalidSchemaSource, got {err:?}"
    );
}
