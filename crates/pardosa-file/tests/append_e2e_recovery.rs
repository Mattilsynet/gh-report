use pardosa_file::manifest::{finalize_recovered_prefix, recover_footerless_prefix};
use pardosa_file::{AppendWriter, FileError, Reader};
use std::io::Cursor;
const KNOWN_HASH: u128 = 0x0F0E_0D0C_0B0A_0908_0706_0504_0302_0100;
#[test]
fn many_appends_then_crash_then_recover_then_read_round_trip() {
    let mut payloads: Vec<Vec<u8>> = Vec::new();
    for i in 0..20u8 {
        let p: Vec<u8> = (0..(8 + i)).map(|j| i.wrapping_mul(j) ^ 0x5A).collect();
        payloads.push(p);
    }
    let mut data_sink: Vec<u8> = Vec::new();
    let mut manifest_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    {
        let mut w = AppendWriter::new(&mut data_sink, KNOWN_HASH).with_manifest(&mut manifest_sink);
        for (i, p) in payloads.iter().enumerate() {
            w.append_message(p).expect("append");
            if i % 3 == 2 {
                w.sync_data().expect("sync");
            }
        }
        w.sync_data().expect("final sync before simulated crash");
    }
    let recovered = recover_footerless_prefix(&data_sink, manifest_sink.get_ref())
        .expect("recover_footerless_prefix");
    assert_eq!(recovered.records.len(), payloads.len());
    let mut cur = Cursor::new(data_sink);
    finalize_recovered_prefix(&recovered, &mut cur).expect("finalize");
    let finalized = cur.into_inner();
    let mut r = Reader::open(Cursor::new(&finalized)).expect("Reader::open after recovery");
    assert_eq!(r.message_count(), payloads.len() as u64);
    let collected: Vec<Vec<u8>> = r.iter_messages().collect::<Result<_, _>>().expect("iter");
    assert_eq!(collected, payloads);
}
#[test]
fn append_after_sync_then_crash_drops_unsynced_tail() {
    let mut payloads: Vec<Vec<u8>> = Vec::new();
    for i in 0..6u8 {
        payloads.push(vec![i; 4 + i as usize]);
    }
    let synced_count = 3;
    let mut data_sink: Vec<u8> = Vec::new();
    let mut manifest_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    {
        let mut w = AppendWriter::new(&mut data_sink, KNOWN_HASH).with_manifest(&mut manifest_sink);
        for p in &payloads[..synced_count] {
            w.append_message(p).expect("append");
        }
        w.sync_data().expect("sync");
        for p in &payloads[synced_count..] {
            w.append_message(p).expect("append-unsynced");
        }
    }
    let recovered = recover_footerless_prefix(&data_sink, manifest_sink.get_ref())
        .expect("recover_footerless_prefix");
    assert_eq!(
        recovered.records.len(),
        synced_count,
        "only synced messages must appear in the recovered index",
    );
    let mut cur = Cursor::new(data_sink);
    finalize_recovered_prefix(&recovered, &mut cur).expect("finalize");
    let finalized = cur.into_inner();
    let mut r = Reader::open(Cursor::new(&finalized)).expect("Reader::open");
    assert_eq!(r.message_count(), synced_count as u64);
    let collected: Vec<Vec<u8>> = r.iter_messages().collect::<Result<_, _>>().expect("iter");
    assert_eq!(collected, payloads[..synced_count]);
}
#[test]
fn original_writer_and_append_writer_produce_identical_finalized_files() {
    let payloads: Vec<Vec<u8>> = (0..8u8).map(|i| vec![i; 16]).collect();
    let mut append_buf: Vec<u8> = Vec::new();
    let mut manifest_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    {
        let mut w =
            AppendWriter::new(&mut append_buf, KNOWN_HASH).with_manifest(&mut manifest_sink);
        for p in &payloads {
            w.append_message(p).expect("append");
            w.sync_data().expect("sync per append");
        }
        w.finish().expect("finish");
    }
    let mut one_shot: Vec<u8> = Vec::new();
    {
        let mut w = pardosa_file::Writer::new(&mut one_shot, KNOWN_HASH);
        for p in &payloads {
            w.write_message(p).expect("write");
        }
        w.finish().expect("finish");
    }
    assert_eq!(
        append_buf, one_shot,
        "AppendWriter::finish must produce byte-identical output to Writer::finish",
    );
}
#[test]
fn recovery_does_not_accept_pgno_without_manifest() {
    let mut data_sink: Vec<u8> = Vec::new();
    let manifest_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    {
        let mut w = AppendWriter::new(&mut data_sink, KNOWN_HASH);
        w.append_message(b"alpha").expect("append");
        w.sync_data().expect("sync no manifest");
    }
    let err = recover_footerless_prefix(&data_sink, manifest_sink.get_ref())
        .expect_err("empty manifest must be rejected");
    let _ = err;
    let direct_err = Reader::open(Cursor::new(&data_sink))
        .expect_err("footerless .pgno must NOT open via Reader::open");
    assert!(
        matches!(
            direct_err,
            FileError::InvalidIndex | FileError::InvalidMagic
        ),
        "Reader::open must reject footerless prefix; got {direct_err:?}",
    );
}
