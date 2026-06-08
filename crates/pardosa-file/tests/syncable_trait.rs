//! `Syncable` trait surface test — SM-2 increment 1.
//!
//! The trait makes the durability operation explicit at the substrate
//! boundary. `Vec<u8>` and `BufWriter<File>` impls degrade to `flush`
//! (memory has no power-loss surface; `BufWriter` forwards to the
//! inner sink); `std::fs::File` delegates to `File::sync_data`.
use pardosa_file::Syncable;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use tempfile::NamedTempFile;
#[test]
fn vec_u8_is_syncable_and_sync_data_is_a_flush() {
    let mut buf: Vec<u8> = Vec::new();
    buf.write_all(b"hello").expect("write_all");
    <Vec<u8> as Syncable>::sync_data(&mut buf).expect("sync_data on Vec<u8>");
    assert_eq!(buf, b"hello");
}
#[test]
fn file_is_syncable_and_sync_data_returns_ok_on_open_writable_file() {
    let tmp = NamedTempFile::new().expect("tempfile");
    let mut f = OpenOptions::new()
        .write(true)
        .open(tmp.path())
        .expect("open writable");
    f.write_all(b"payload").expect("write");
    <std::fs::File as Syncable>::sync_data(&mut f).expect("File::sync_data");
}
#[test]
fn bufwriter_is_syncable_when_inner_is_syncable() {
    let tmp = NamedTempFile::new().expect("tempfile");
    let f = OpenOptions::new()
        .write(true)
        .open(tmp.path())
        .expect("open writable");
    let mut bw = BufWriter::new(f);
    bw.write_all(b"buffered-payload").expect("write");
    <BufWriter<std::fs::File> as Syncable>::sync_data(&mut bw).expect("BufWriter sync_data");
}
#[test]
fn mut_ref_is_syncable_when_inner_is_syncable() {
    let mut buf: Vec<u8> = Vec::new();
    let mut r: &mut Vec<u8> = &mut buf;
    <&mut Vec<u8> as Syncable>::sync_data(&mut r).expect("sync_data on &mut Vec<u8>");
}
#[test]
fn writer_sync_data_on_empty_writer_writes_lazy_header() {
    use pardosa_file::Writer;
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut w = Writer::new(&mut buf, 0xDEAD_BEEFu128);
        w.sync_data().expect("sync_data on empty Writer");
    }
    assert!(
        !buf.is_empty(),
        "sync_data on an empty Writer must have flushed the lazy header"
    );
}
/// W2: `Syncable::set_len` shrinks a `Vec<u8>` sink to the requested
/// length, dropping any trailing bytes from a prior longer write.
/// Memory-backed impl invariant: in-place truncation, no error.
#[test]
fn set_len_truncates_vec_u8() {
    let mut buf: Vec<u8> = vec![1, 2, 3, 4, 5, 6, 7, 8];
    <Vec<u8> as Syncable>::set_len(&mut buf, 3).expect("set_len");
    assert_eq!(buf, [1, 2, 3]);
}
/// W2: `Syncable::set_len` on a `Cursor<Vec<u8>>` mirrors the
/// `Vec<u8>` semantics — the inner buffer is truncated in place.
#[test]
fn set_len_truncates_cursor_vec_u8() {
    use std::io::Cursor;
    let mut c: Cursor<Vec<u8>> = Cursor::new(vec![9u8; 16]);
    <Cursor<Vec<u8>> as Syncable>::set_len(&mut c, 4).expect("set_len");
    assert_eq!(c.get_ref().len(), 4);
}
/// W2: `Syncable::set_len` truncates a `std::fs::File` on disk via
/// the underlying `File::set_len`; bytes past `len` are gone.
#[test]
fn set_len_truncates_file() {
    use std::io::{Read, Seek, SeekFrom};
    let tmp = NamedTempFile::new().expect("tempfile");
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(tmp.path())
        .expect("open rw");
    f.write_all(&[0x55u8; 32]).expect("write");
    <std::fs::File as Syncable>::set_len(&mut f, 8).expect("File::set_len");
    f.seek(SeekFrom::Start(0)).expect("seek");
    let mut got = Vec::new();
    f.read_to_end(&mut got).expect("read");
    assert_eq!(got.len(), 8);
}
