use pardosa_file::manifest::{ManifestRecord, parse_manifest};
use pardosa_file::{AppendWriter, Reader};
use std::io::Cursor;
use xxhash_rust::xxh64::xxh64;
const KNOWN_HASH: u128 = 0x0F0E_0D0C_0B0A_0908_0706_0504_0302_0100;
fn payloads() -> Vec<Vec<u8>> {
    vec![
        b"alpha".to_vec(),
        b"bravo".to_vec(),
        (0..16u8).collect::<Vec<u8>>(),
    ]
}
#[test]
fn manifest_sink_untouched_until_first_sync() {
    let payloads = payloads();
    let mut data_sink: Vec<u8> = Vec::new();
    let mut manifest_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    {
        let mut w = AppendWriter::new(&mut data_sink, KNOWN_HASH).with_manifest(&mut manifest_sink);
        for p in &payloads {
            w.append_message(p).expect("append");
        }
    }
    assert!(
        manifest_sink.get_ref().is_empty(),
        "manifest sink must remain empty until sync_data fires",
    );
}
#[test]
fn first_sync_writes_full_manifest() {
    let payloads = payloads();
    let mut data_sink: Vec<u8> = Vec::new();
    let mut manifest_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    let mut w = AppendWriter::new(&mut data_sink, KNOWN_HASH).with_manifest(&mut manifest_sink);
    for p in &payloads {
        w.append_message(p).expect("append");
    }
    let data_end = w.data_end_offset();
    w.sync_data().expect("sync_data");
    drop(w);
    let snap = parse_manifest(manifest_sink.get_ref()).expect("parse manifest");
    assert_eq!(snap.schema_hash, KNOWN_HASH);
    assert_eq!(snap.page_class, 0);
    assert_eq!(snap.schema_size, 0);
    assert_eq!(snap.records.len(), payloads.len());
    assert_eq!(snap.data_end, data_end);
    for (i, p) in payloads.iter().enumerate() {
        assert_eq!(snap.records[i].size as usize, p.len());
        assert_eq!(snap.records[i].checksum, xxh64(p, 0));
    }
}
#[test]
fn incremental_sync_only_writes_delta_plus_footer() {
    let mut data_sink: Vec<u8> = Vec::new();
    let mut manifest_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    let (len_after_sync1, len_after_sync2) = {
        let mut w = AppendWriter::new(&mut data_sink, KNOWN_HASH).with_manifest(&mut manifest_sink);
        w.append_message(b"alpha").expect("a1");
        w.sync_data().expect("sync 1");
        let pos1 = w.manifest_persisted_len().expect("len after sync 1");
        w.append_message(b"bravo").expect("a2");
        w.sync_data().expect("sync 2");
        let pos2 = w.manifest_persisted_len().expect("len after sync 2");
        (pos1, pos2)
    };
    let snap1_len = pardosa_file::manifest::MANIFEST_HEADER_SIZE
        + pardosa_file::manifest::MANIFEST_RECORD_SIZE
        + pardosa_file::manifest::MANIFEST_FOOTER_SIZE;
    assert_eq!(
        usize::try_from(len_after_sync1).expect("len fits usize"),
        snap1_len
    );
    let snap2_len = pardosa_file::manifest::MANIFEST_HEADER_SIZE
        + 2 * pardosa_file::manifest::MANIFEST_RECORD_SIZE
        + pardosa_file::manifest::MANIFEST_FOOTER_SIZE;
    assert_eq!(
        usize::try_from(len_after_sync2).expect("len fits usize"),
        snap2_len
    );
    let snap = parse_manifest(manifest_sink.get_ref()).expect("parse after 2 syncs");
    assert_eq!(snap.records.len(), 2);
}
#[test]
fn finish_does_not_alter_manifest() {
    let payloads = payloads();
    let mut data_sink: Vec<u8> = Vec::new();
    let mut manifest_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    let manifest_len_after_sync;
    {
        let mut w = AppendWriter::new(&mut data_sink, KNOWN_HASH).with_manifest(&mut manifest_sink);
        for p in &payloads {
            w.append_message(p).expect("append");
        }
        w.sync_data().expect("sync");
        manifest_len_after_sync = w
            .manifest_persisted_len()
            .expect("manifest_persisted_len after sync");
        w.finish().expect("finish");
    }
    assert_eq!(
        manifest_sink.get_ref().len() as u64,
        manifest_len_after_sync,
        "finish must not write any further bytes to the manifest sink",
    );
    let mut r = Reader::open(Cursor::new(&data_sink)).expect("Reader::open");
    let collected: Vec<Vec<u8>> = r.iter_messages().collect::<Result<_, _>>().expect("iter");
    assert_eq!(collected, payloads);
}
#[test]
fn page_class_and_schema_source_propagate_to_manifest_header() {
    let mut data_sink: Vec<u8> = Vec::new();
    let mut manifest_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    let source = "examples/foo";
    let mut w = AppendWriter::new(&mut data_sink, KNOWN_HASH)
        .with_page_class(3)
        .with_schema_source(source)
        .with_manifest(&mut manifest_sink);
    w.append_message(b"alpha").expect("a1");
    w.sync_data().expect("sync");
    drop(w);
    let snap = parse_manifest(manifest_sink.get_ref()).expect("parse");
    assert_eq!(snap.schema_hash, KNOWN_HASH);
    assert_eq!(snap.page_class, 3);
    assert_eq!(
        snap.schema_size,
        u32::try_from(source.len()).expect("source.len fits u32")
    );
}
#[test]
fn manifest_record_offsets_match_data_offsets() {
    let payloads = payloads();
    let mut data_sink: Vec<u8> = Vec::new();
    let mut manifest_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    let mut w = AppendWriter::new(&mut data_sink, KNOWN_HASH).with_manifest(&mut manifest_sink);
    for p in &payloads {
        w.append_message(p).expect("append");
    }
    w.sync_data().expect("sync");
    w.finish().expect("finish");
    let snap = parse_manifest(manifest_sink.get_ref()).expect("parse");
    let r = Reader::open(Cursor::new(&data_sink)).expect("Reader::open");
    let pgno_index = r.index();
    assert_eq!(snap.records.len(), pgno_index.len());
    for (mrec, prec) in snap.records.iter().zip(pgno_index.iter()) {
        assert_eq!(
            *mrec,
            ManifestRecord {
                offset: prec.offset,
                size: prec.size,
                checksum: prec.checksum,
            }
        );
    }
}
