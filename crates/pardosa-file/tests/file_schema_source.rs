use pardosa_file::FileError;
use pardosa_file::format::{
    FILE_FOOTER_SIZE, FILE_HEADER_SIZE, FOOTER_CHECKSUM_OFFSET, HEADER_SCHEMA_SIZE_OFFSET,
    messages_offset, pad_to_8,
};
use pardosa_file::{Reader, Writer};
use std::io::Cursor;
use xxhash_rust::xxh64::xxh64;
const SCHEMA_HASH: u128 = 0xDEAD_BEEF_FEED_FACE_1234_5678_90AB_CDEFu128;
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
    let payloads: &[&[u8]] = &[b"alpha", b"beta-message", b"gamma"];
    let no_schema = build_file(None, payloads);
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
    let schema = "struct X { y }";
    let schema = &schema[..13];
    assert_eq!(schema.len(), 13);
    let payloads: &[&[u8]] = &[b"alpha", b"beta-message", b"gamma"];
    let file = build_file(Some(schema), payloads);
    let schema_size = u32::from_le_bytes(
        file[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4]
            .try_into()
            .unwrap(),
    );
    assert_eq!(schema_size, 13);
    assert_eq!(
        &file[FILE_HEADER_SIZE..FILE_HEADER_SIZE + 13],
        schema.as_bytes(),
        "schema block bytes must match input UTF-8 verbatim"
    );
    let pad_start = FILE_HEADER_SIZE + 13;
    let pad_end = FILE_HEADER_SIZE + pad_to_8(13);
    assert_eq!(pad_end - pad_start, 3, "pad_to_8(13) - 13 == 3");
    assert!(
        file[pad_start..pad_end].iter().all(|&b| b == 0),
        "schema-block pad bytes must be zero, got {:?}",
        &file[pad_start..pad_end]
    );
    let msgs_start = messages_offset(13);
    assert_eq!(msgs_start, 56);
    assert_eq!(&file[msgs_start..msgs_start + 5], b"alpha");
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
    let schema = "struct Y { a: u8 }";
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
    assert_eq!(&file[80..82], b"m0");
    let r = Reader::open(Cursor::new(&file)).expect("open");
    assert_eq!(r.schema_source(), Some(schema.as_str()));
}
#[test]
fn empty_schema_string_writes_block_of_size_zero() {
    let file = build_file(Some(""), &[b"x"]);
    let schema_size = u32::from_le_bytes(
        file[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4]
            .try_into()
            .unwrap(),
    );
    assert_eq!(schema_size, 0);
    assert_eq!(&file[FILE_HEADER_SIZE..=FILE_HEADER_SIZE], b"x");
    let r = Reader::open(Cursor::new(&file)).expect("open");
    assert_eq!(r.schema_source(), None);
    assert_eq!(r.schema_size(), 0);
}
#[test]
fn schema_block_with_multibyte_utf8_round_trips() {
    let schema = "// café — π ≈ 3.14";
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
    let mut file = build_file(Some("ok-utf8"), &[b"payload"]);
    Reader::open(Cursor::new(&file)).expect("baseline file is well-formed");
    let bad: [u8; 7] = [0xFF, 0xFE, 0xC0, 0xC1, 0x80, 0xFF, 0x80];
    file[FILE_HEADER_SIZE..FILE_HEADER_SIZE + 7].copy_from_slice(&bad);
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
fn refresh_footer_checksum(file: &mut [u8]) {
    let n = file.len();
    let footer_start = n - FILE_FOOTER_SIZE;
    let footer = &mut file[footer_start..];
    let cksum = xxh64(&footer[..FOOTER_CHECKSUM_OFFSET], 0);
    footer[FOOTER_CHECKSUM_OFFSET..FOOTER_CHECKSUM_OFFSET + 8]
        .copy_from_slice(&cksum.to_le_bytes());
}
#[test]
fn rejects_non_zero_pad_width_7() {
    let mut file = build_file(Some("a"), &[b"payload"]);
    Reader::open(Cursor::new(&file)).expect("baseline file is well-formed");
    let pad_start = FILE_HEADER_SIZE + 1;
    let pad_end = FILE_HEADER_SIZE + pad_to_8(1);
    assert_eq!(pad_end - pad_start, 7);
    file[pad_end - 1] = 0xAB;
    refresh_footer_checksum(&mut file);
    let err = Reader::open(Cursor::new(&file)).expect_err("must reject non-zero pad");
    assert!(
        matches!(err, FileError::InvalidReserved),
        "expected FileError::InvalidReserved, got {err:?}"
    );
}
#[test]
fn rejects_non_zero_pad_width_5() {
    let mut file = build_file(Some("abc"), &[b"payload"]);
    Reader::open(Cursor::new(&file)).expect("baseline file is well-formed");
    let pad_start = FILE_HEADER_SIZE + 3;
    let pad_end = FILE_HEADER_SIZE + pad_to_8(3);
    assert_eq!(pad_end - pad_start, 5);
    file[pad_start] = 0x01;
    refresh_footer_checksum(&mut file);
    let err = Reader::open(Cursor::new(&file)).expect_err("must reject non-zero pad");
    assert!(
        matches!(err, FileError::InvalidReserved),
        "expected FileError::InvalidReserved, got {err:?}"
    );
}
#[test]
fn rejects_non_zero_pad_width_1() {
    let mut file = build_file(Some("schema!"), &[b"payload"]);
    Reader::open(Cursor::new(&file)).expect("baseline file is well-formed");
    let pad_start = FILE_HEADER_SIZE + 7;
    let pad_end = FILE_HEADER_SIZE + pad_to_8(7);
    assert_eq!(pad_end - pad_start, 1);
    file[pad_start] = 0xFF;
    refresh_footer_checksum(&mut file);
    let err = Reader::open(Cursor::new(&file)).expect_err("must reject non-zero pad");
    assert!(
        matches!(err, FileError::InvalidReserved),
        "expected FileError::InvalidReserved, got {err:?}"
    );
}
#[test]
fn accepts_zero_width_pad() {
    let schema = "abcdefgh";
    assert_eq!(schema.len(), 8);
    assert_eq!(pad_to_8(8) - 8, 0);
    let file = build_file(Some(schema), &[b"payload"]);
    let mut r = Reader::open(Cursor::new(&file)).expect("zero-width pad must accept");
    assert_eq!(r.schema_source(), Some(schema));
    let got: Vec<Vec<u8>> = r.iter_messages().collect::<Result<_, _>>().unwrap();
    assert_eq!(got, vec![b"payload".to_vec()]);
}
