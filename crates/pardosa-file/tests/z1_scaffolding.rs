//! Z1-01: API scaffolding for optional zstd compression.
//!
//! Pins the public surface that z1-02 (reader) and z1-03
//! (writer) depend on:
//!
//! - `WriterOptions` / `ReaderOptions` exist + constructable.
//! - `Writer::with_options` / `Reader::open_with_options` exist.
//! - `Writer::new` / `Reader::open` stay source-compatible
//!   (default uncompressed).
//! - `ReaderOptions::with_max_decompressed_message_bytes`
//!   exists; default 1 GiB.
//! - `FileError::DecompressedTooLarge` variant exists.
//!
//! No zstd behaviour here; defaults round-trip uncompressed
//! unchanged. ADR-0006.
use pardosa_file::{FileError, Reader, ReaderOptions, Writer, WriterOptions};
use std::io::Cursor;
#[test]
fn writer_options_default_is_uncompressed_and_byte_identical() {
    let schema_hash: u128 = 0x1122_3344_5566_7788_99AA_BBCC_DDEE_FF00;
    let mut buf_legacy: Vec<u8> = Vec::new();
    {
        let mut w = Writer::new(&mut buf_legacy, schema_hash);
        w.write_message(b"hello").expect("legacy write_message");
        w.finish().expect("legacy finish");
    }
    let mut buf_opts: Vec<u8> = Vec::new();
    {
        let mut w = Writer::with_options(&mut buf_opts, schema_hash, WriterOptions::default());
        w.write_message(b"hello").expect("opts write_message");
        w.finish().expect("opts finish");
    }
    assert_eq!(
        buf_legacy, buf_opts,
        "WriterOptions::default() must produce byte-identical output to Writer::new \
         (preserves golden uncompressed vectors)"
    );
}
#[test]
fn reader_open_with_default_options_round_trips_uncompressed_file() {
    let schema_hash: u128 = 0xAAAA_BBBB_CCCC_DDDD_EEEE_FFFF_0000_1111;
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut w = Writer::new(&mut buf, schema_hash);
        w.write_message(b"alpha").unwrap();
        w.write_message(b"beta").unwrap();
        w.finish().unwrap();
    }
    let mut r =
        Reader::open_with_options(Cursor::new(buf), ReaderOptions::default()).expect("open_opts");
    assert_eq!(r.message_count(), 2);
    assert_eq!(r.read_message(0).unwrap(), b"alpha");
    assert_eq!(r.read_message(1).unwrap(), b"beta");
}
#[test]
fn reader_options_default_max_decompressed_is_one_gib() {
    let opts = ReaderOptions::default();
    assert_eq!(
        opts.max_decompressed_message_bytes(),
        1024 * 1024 * 1024,
        "default decompression cap pinned at 1 GiB"
    );
}
#[test]
fn reader_options_with_max_decompressed_is_builder() {
    let opts = ReaderOptions::default().with_max_decompressed_message_bytes(512);
    assert_eq!(opts.max_decompressed_message_bytes(), 512);
}
#[test]
fn file_error_decompressed_too_large_variant_exists_and_displays() {
    let e = FileError::DecompressedTooLarge { limit: 1024 };
    let s = format!("{e}");
    assert!(
        s.contains("decompressed") && s.contains("1024"),
        "Display should mention the cap; got {s:?}"
    );
}
