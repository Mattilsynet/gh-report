//! v6 `adopter_epoch` vectors (PGN-0021 R3/R5).
//!
//! Covers: version-6 round-trip with epoch absent (byte-identical to
//! the pre-OSF layout, PGN-0021 R8), the presence discriminant
//! (`None` vs `Some(&[])` must be distinguishable on disk, PGN-0021
//! R3), a populated round-trip, and the opaque byte-equality helper
//! (PGN-0021 R4).
use pardosa_file::format::{
    FORMAT_VERSION, HEADER_EPOCH_PRESENT_FLAG, HEADER_FLAGS_OFFSET, HEADER_VERSION_OFFSET,
    epoch_bytes_eq,
};
use pardosa_file::{Reader, Writer};
use std::io::Cursor;
fn build(epoch: Option<&[u8]>, messages: &[&[u8]]) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut w = Writer::new(&mut buf, 0x1234_5678_9ABC_DEF0_1122_3344_5566_7788);
        if let Some(e) = epoch {
            w = w.with_epoch(e);
        }
        for m in messages {
            w.write_message(m).expect("write_message");
        }
        w.finish().expect("finish");
    }
    buf
}
#[test]
fn v6_epoch_absent_matches_pre_osf_layout() {
    let bytes = build(None, &[]);
    assert_eq!(
        u16::from_le_bytes(
            bytes[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2]
                .try_into()
                .unwrap()
        ),
        FORMAT_VERSION
    );
    assert_eq!(FORMAT_VERSION, 6, "FORMAT_VERSION pinned at v6");
    let flags = u16::from_le_bytes(
        bytes[HEADER_FLAGS_OFFSET..HEADER_FLAGS_OFFSET + 2]
            .try_into()
            .unwrap(),
    );
    assert_eq!(
        flags & HEADER_EPOCH_PRESENT_FLAG,
        0,
        "epoch-absent: presence bit clear"
    );
    assert_eq!(
        bytes.len(),
        72,
        "epoch=None contributes zero extra bytes (R8)"
    );
    let r = Reader::open(Cursor::new(bytes)).expect("v6 reader opens epoch-absent container");
    assert_eq!(r.epoch(), None);
    assert_eq!(r.message_count(), 0);
}
#[test]
fn v6_epoch_some_round_trips() {
    let bytes = build(Some(b"16.0"), &[b"hello"]);
    let mut r =
        Reader::open(Cursor::new(bytes)).expect("v6 reader opens epoch-populated container");
    assert_eq!(r.epoch(), Some(b"16.0".as_slice()));
    assert_eq!(r.read_message(0).unwrap(), b"hello");
}
/// Feynman gap #1: `None` and `Some(&[])` must be byte-distinguishable
/// on disk, never conflated via a length==0 check (PGN-0021 R3).
#[test]
fn v6_presence_discriminant_none_vs_some_empty() {
    let none_bytes = build(None, &[]);
    let some_empty_bytes = build(Some(b""), &[]);
    assert_ne!(
        none_bytes, some_empty_bytes,
        "None and Some(empty) must produce distinct on-disk bytes"
    );
    let none_flags = u16::from_le_bytes(
        none_bytes[HEADER_FLAGS_OFFSET..HEADER_FLAGS_OFFSET + 2]
            .try_into()
            .unwrap(),
    );
    let some_flags = u16::from_le_bytes(
        some_empty_bytes[HEADER_FLAGS_OFFSET..HEADER_FLAGS_OFFSET + 2]
            .try_into()
            .unwrap(),
    );
    assert_eq!(none_flags & HEADER_EPOCH_PRESENT_FLAG, 0);
    assert_eq!(
        some_flags & HEADER_EPOCH_PRESENT_FLAG,
        HEADER_EPOCH_PRESENT_FLAG
    );
    let mut r_none = Reader::open(Cursor::new(none_bytes)).expect("opens");
    let mut r_some = Reader::open(Cursor::new(some_empty_bytes)).expect("opens");
    assert_eq!(r_none.epoch(), None);
    assert_eq!(r_some.epoch(), Some(&[][..]));
    assert_ne!(r_none.epoch(), r_some.epoch());
    let _ = (&mut r_none, &mut r_some);
}
#[test]
fn epoch_bytes_eq_none_none_is_equal() {
    assert!(epoch_bytes_eq(None, None));
}
#[test]
fn epoch_bytes_eq_none_vs_some_is_unequal() {
    assert!(!epoch_bytes_eq(None, Some(b"")));
    assert!(!epoch_bytes_eq(Some(b""), None));
    assert!(!epoch_bytes_eq(None, Some(b"16.0")));
}
#[test]
fn epoch_bytes_eq_some_same_bytes_is_equal() {
    assert!(epoch_bytes_eq(Some(b"16.0"), Some(b"16.0")));
}
#[test]
fn epoch_bytes_eq_some_different_bytes_is_unequal() {
    assert!(!epoch_bytes_eq(Some(b"16.0"), Some(b"15.0")));
    assert!(!epoch_bytes_eq(Some(b"16.0"), Some(b"16.00")));
}
#[test]
fn v6_reserved_and_version_gate_invariants_hold() {
    let bytes = build(Some(b"e"), &[b"m"]);
    assert_eq!(
        Reader::open(Cursor::new(bytes.clone()))
            .expect("valid v6 container opens")
            .epoch(),
        Some(b"e".as_slice())
    );
    let mut tampered = bytes;
    tampered[HEADER_VERSION_OFFSET] = 5;
    let err = Reader::open(Cursor::new(tampered)).expect_err("v6 reader refuses version 5");
    assert!(matches!(
        err,
        pardosa_file::FileError::UnsupportedVersion(5)
    ));
}
