use pardosa_file::format::{FILE_FOOTER_SIZE, FILE_HEADER_SIZE, MAGIC, messages_offset};
use pardosa_file::{AppendWriter, Reader, Writer};
use std::io::Cursor;
use xxhash_rust::xxh64::xxh64;
const KNOWN_HASH: u128 = 0x0F0E_0D0C_0B0A_0908_0706_0504_0302_0100;
fn fixture_payloads() -> Vec<Vec<u8>> {
    vec![
        b"alpha".to_vec(),
        b"bravo".to_vec(),
        (0..32u8).collect::<Vec<u8>>(),
    ]
}
#[test]
fn fresh_append_writer_does_not_touch_sink_before_first_append() {
    let mut sink: Vec<u8> = Vec::new();
    let _w = AppendWriter::new(&mut sink, KNOWN_HASH);
    assert!(
        sink.is_empty(),
        "AppendWriter::new must not write any bytes — header is lazy",
    );
}
#[test]
fn first_append_lazily_writes_header_then_body() {
    let payload = b"alpha".to_vec();
    let mut sink: Vec<u8> = Vec::new();
    let mut w = AppendWriter::new(&mut sink, KNOWN_HASH);
    let _offset = w.append_message(&payload).expect("append_message");
    drop(w);
    assert!(
        sink.len() >= FILE_HEADER_SIZE + payload.len(),
        "first append must write header + body; got {} bytes",
        sink.len(),
    );
    assert_eq!(&sink[..4], &MAGIC, "header magic must be PGNO at byte 0");
    let body_start = messages_offset(0);
    assert_eq!(
        &sink[body_start..body_start + payload.len()],
        payload.as_slice(),
        "body bytes must follow header verbatim",
    );
}
#[test]
fn append_returns_post_append_data_end_offset() {
    let mut sink: Vec<u8> = Vec::new();
    let mut w = AppendWriter::new(&mut sink, KNOWN_HASH);
    let body_start = messages_offset(0) as u64;
    let off1 = w.append_message(b"alpha").expect("a1");
    assert_eq!(
        off1,
        body_start + 5,
        "first append_message must return the byte offset AFTER the body",
    );
    let off2 = w.append_message(b"bravo").expect("a2");
    assert_eq!(
        off2,
        body_start + 5 + 5,
        "second append_message must return cumulative offset",
    );
    assert_eq!(w.data_end_offset(), off2);
    assert_eq!(w.message_count(), 2);
}
#[test]
fn append_writer_with_no_finish_leaves_no_footer() {
    let payloads = fixture_payloads();
    let mut sink: Vec<u8> = Vec::new();
    {
        let mut w = AppendWriter::new(&mut sink, KNOWN_HASH);
        for p in &payloads {
            w.append_message(p).expect("append");
        }
    }
    let expected_data_end = messages_offset(0) + payloads.iter().map(Vec::len).sum::<usize>();
    assert_eq!(
        sink.len(),
        expected_data_end,
        "without finish, sink must hold header + bodies only — no index, no footer",
    );
    let footer_magic_start = sink.len().saturating_sub(FILE_FOOTER_SIZE) + 20;
    assert!(
        !(sink.len() >= FILE_FOOTER_SIZE
            && sink[footer_magic_start..footer_magic_start + 4] == MAGIC),
        "footerless invariant violated: footer magic found at offset {footer_magic_start}"
    );
}
#[test]
fn append_writer_finish_produces_reader_open_compatible_file() {
    let payloads = fixture_payloads();
    let mut append_buf: Vec<u8> = Vec::new();
    {
        let mut w = AppendWriter::new(&mut append_buf, KNOWN_HASH);
        for p in &payloads {
            w.append_message(p).expect("append");
        }
        w.finish().expect("finish");
    }
    let mut r = Reader::open(Cursor::new(&append_buf)).expect("Reader::open");
    assert_eq!(r.message_count(), payloads.len() as u64);
    assert_eq!(r.schema_hash(), KNOWN_HASH);
    for (i, p) in payloads.iter().enumerate() {
        let got = r.read_message(i).expect("read_message");
        assert_eq!(&got, p, "payload {i} round-trip");
        assert_eq!(r.index()[i].checksum, xxh64(p, 0));
    }
}
#[test]
fn append_writer_finish_byte_identical_to_one_shot_writer() {
    let payloads = fixture_payloads();
    let mut append_buf: Vec<u8> = Vec::new();
    {
        let mut w = AppendWriter::new(&mut append_buf, KNOWN_HASH);
        for p in &payloads {
            w.append_message(p).expect("append");
        }
        w.finish().expect("finish");
    }
    let mut one_shot_buf: Vec<u8> = Vec::new();
    {
        let mut w = Writer::new(&mut one_shot_buf, KNOWN_HASH);
        for p in &payloads {
            w.write_message(p).expect("write_message");
        }
        w.finish().expect("finish");
    }
    assert_eq!(
        append_buf, one_shot_buf,
        "AppendWriter::finish must produce byte-identical output to Writer::finish for the same payload sequence",
    );
}
