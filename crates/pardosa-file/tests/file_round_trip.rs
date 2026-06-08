use pardosa_file::FileError;
use pardosa_file::format::FILE_FOOTER_SIZE;
use pardosa_file::{Reader, Writer};
use std::io::Cursor;
use xxhash_rust::xxh64::xxh64;
const KNOWN_HASH: u128 = 0x0F0E_0D0C_0B0A_0908_0706_0504_0302_0100;
fn fixture_payloads() -> Vec<Vec<u8>> {
    vec![
        b"A".to_vec(),
        b"hello-pardosa!!".to_vec(),
        (0..16u8).collect::<Vec<u8>>(),
    ]
}
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
#[test]
fn multi_message_round_trip_byte_identical() {
    let payloads = fixture_payloads();
    let file = build_file(&payloads);
    let mut r = Reader::open(Cursor::new(&file)).expect("Reader::open");
    assert_eq!(r.message_count(), payloads.len() as u64);
    assert_eq!(r.schema_hash(), KNOWN_HASH);
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
    let payloads = fixture_payloads();
    let file = build_file(&payloads);
    let r = Reader::open(Cursor::new(&file)).expect("open");
    for (i, p) in payloads.iter().enumerate() {
        let entry = r.index()[i];
        assert_eq!(entry.checksum, xxh64(p, 0), "entry {i} checksum");
        assert_eq!(entry.size as usize, p.len(), "entry {i} size");
    }
}
#[test]
fn rejects_per_message_checksum_mismatch_on_body_corruption() {
    let payloads = fixture_payloads();
    let mut file = build_file(&payloads);
    let r = Reader::open(Cursor::new(&file)).expect("Reader::open pre-corruption");
    let body_offset =
        usize::try_from(r.index()[1].offset).expect("offset fits usize on test target");
    let body_size = r.index()[1].size as usize;
    drop(r);
    file[body_offset + body_size / 2] ^= 0xFF;
    let mut r = Reader::open(Cursor::new(&file)).expect("Reader::open post-corruption");
    r.read_message(0).expect("msg 0 still verifies");
    let err = r.read_message(1).expect_err("msg 1 must reject");
    assert!(matches!(err, FileError::ChecksumMismatch(1)), "got {err:?}");
    r.read_message(2).expect("msg 2 still verifies");
}
#[test]
fn rejects_per_message_checksum_mismatch_on_index_checksum_corruption() {
    let payloads = fixture_payloads();
    let mut file = build_file(&payloads);
    let r = Reader::open(Cursor::new(&file)).expect("open");
    let index_start = {
        let n = file.len();
        let footer_start = n - FILE_FOOTER_SIZE;
        usize::try_from(u64::from_le_bytes(
            file[footer_start..footer_start + 8].try_into().unwrap(),
        ))
        .expect("index offset fits usize on test target")
    };
    drop(r);
    let entry1_cksum_at = index_start + 24 + 16;
    file[entry1_cksum_at] ^= 0x01;
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
