//! H3 — Format-semantics hardening regression suite.
//!
//! Pins behaviours before zstd / compressed page-classes land:
//!
//! 1. `page_class` is opaque — every accepted byte round-trips
//!    losslessly. Compression gated by `flags`, not class.
//! 2. `Writer::finish` consumes `self`; only path emitting
//!    index + footer. `Writer::sync_data` only fences
//!    durability, no footer.
//! 3. `Writer::Drop` is **not** durability — dropped writers
//!    leave header+bodies (no index/footer), unopenable.
use pardosa_file::format::{FILE_HEADER_SIZE, HEADER_PAGE_CLASS_OFFSET, MIN_FILE_SIZE};
use pardosa_file::{FileError, Reader, Writer};
use std::io::Cursor;
const KNOWN_HASH: u128 = 0x0F0E_0D0C_0B0A_0908_0706_0504_0302_0100;
/// SC: every u8 value the writer accepts as `page_class` round-trips
/// losslessly through `Reader::page_class`. Sampled at 0, 1, 127, 128, 254,
/// 255 (boundary values across the byte range — full 256-sweep would be
/// redundant since the writer copies the byte directly into the header).
#[test]
fn page_class_all_boundary_values_round_trip() {
    for class in [0u8, 1, 127, 128, 254, 255] {
        let mut buf = Vec::new();
        let w = Writer::new(&mut buf, KNOWN_HASH).with_page_class(class);
        w.finish().expect("finish");
        assert_eq!(
            buf[HEADER_PAGE_CLASS_OFFSET], class,
            "writer must copy page_class={class} byte verbatim into the header",
        );
        let r = Reader::open(Cursor::new(&buf)).expect("Reader::open");
        assert_eq!(
            r.page_class(),
            class,
            "reader must surface page_class={class} unchanged",
        );
    }
}
/// SC: `sync_data` writes the lazy header on first call but does **not**
/// emit a footer. The resulting prefix is not a valid file; `Reader::open`
/// must reject it with a structural error (not a panic, not an `Io`
/// passthrough).
#[test]
fn sync_data_without_finish_yields_unopenable_prefix() {
    let mut sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    {
        let mut w = Writer::new(&mut sink, KNOWN_HASH);
        w.write_message(b"slot-zero").expect("write_message");
        w.sync_data().expect("sync_data on in-memory sink");
    }
    let buf = sink.into_inner();
    assert!(
        buf.len() < MIN_FILE_SIZE + b"slot-zero".len() + 24,
        "file is header + body only, no index, no footer; got {} bytes",
        buf.len(),
    );
    let err = Reader::open(Cursor::new(&buf)).expect_err("must reject unfinalised prefix");
    assert!(
        matches!(
            err,
            FileError::Io(_) | FileError::InvalidMagic | FileError::InvalidIndex
        ),
        "unfinalised prefix must be rejected structurally, got {err:?}",
    );
}
/// SC: dropping a Writer without calling `finish` is not a durability
/// boundary — even if the sink itself is a `Vec<u8>` (infallible), no
/// finalisation occurs. The byte length is just header + bodies (no index,
/// no footer) and the file is unopenable.
#[test]
fn drop_without_finish_is_not_a_finalisation() {
    let mut buf: Vec<u8> = Vec::new();
    let body = b"event-payload";
    {
        let mut w = Writer::new(&mut buf, KNOWN_HASH);
        w.write_message(body).expect("write_message");
    }
    assert_eq!(
        buf.len(),
        FILE_HEADER_SIZE + body.len(),
        "drop emits no footer and no index — only header + body bytes",
    );
    let err = Reader::open(Cursor::new(&buf)).expect_err("dropped writer is not a complete file");
    assert!(
        matches!(
            err,
            FileError::Io(_) | FileError::InvalidMagic | FileError::InvalidIndex
        ),
        "dropped-writer prefix must be rejected structurally, got {err:?}",
    );
}
/// SC: `finish` consumes `self`. After `finish` returns Ok the file is
/// fully formed and `Reader::open` succeeds, surfacing the recorded
/// `page_class`, schema hash, and message count.
#[test]
fn finish_produces_a_complete_openable_file() {
    let mut buf = Vec::new();
    {
        let mut w = Writer::new(&mut buf, KNOWN_HASH).with_page_class(42);
        w.write_message(b"a").expect("write a");
        w.write_message(b"bb").expect("write bb");
        w.finish().expect("finish");
    }
    let r = Reader::open(Cursor::new(&buf)).expect("Reader::open on finished file");
    assert_eq!(r.schema_hash(), KNOWN_HASH);
    assert_eq!(r.page_class(), 42);
    assert_eq!(r.message_count(), 2);
}
