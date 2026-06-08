use pardosa_file::manifest::{
    ManifestRecord, RecoveryError, finalize_recovered_prefix, recover_footerless_prefix,
};
use pardosa_file::{AppendWriter, Reader, Writer};
use std::io::Cursor;
const KNOWN_HASH: u128 = 0x0F0E_0D0C_0B0A_0908_0706_0504_0302_0100;
fn payloads() -> Vec<Vec<u8>> {
    vec![
        b"alpha".to_vec(),
        b"bravo".to_vec(),
        (0..32u8).collect::<Vec<u8>>(),
        b"charlie".to_vec(),
    ]
}
fn write_synced_prefix(payloads: &[Vec<u8>]) -> (Vec<u8>, Cursor<Vec<u8>>) {
    let mut data_sink: Vec<u8> = Vec::new();
    let mut manifest_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    {
        let mut w = AppendWriter::new(&mut data_sink, KNOWN_HASH).with_manifest(&mut manifest_sink);
        for p in payloads {
            w.append_message(p).expect("append");
        }
        w.sync_data().expect("sync");
    }
    (data_sink, manifest_sink)
}
#[test]
fn recovery_walks_manifest_records_to_recovered_prefix() {
    let payloads = payloads();
    let (data, manifest) = write_synced_prefix(&payloads);
    let recovered =
        recover_footerless_prefix(&data, manifest.get_ref()).expect("recover_footerless_prefix");
    assert_eq!(recovered.schema_hash, KNOWN_HASH);
    assert_eq!(recovered.records.len(), payloads.len());
    assert_eq!(
        usize::try_from(recovered.data_end).expect("data_end fits usize"),
        data.len()
    );
    for (i, p) in payloads.iter().enumerate() {
        let r = &recovered.records[i];
        assert_eq!(r.size as usize, p.len());
    }
}
#[test]
fn finalize_recovered_prefix_produces_reader_open_compatible_file() {
    let payloads = payloads();
    let (mut data, manifest) = write_synced_prefix(&payloads);
    let recovered =
        recover_footerless_prefix(&data, manifest.get_ref()).expect("recover_footerless_prefix");
    let mut cur = Cursor::new(Vec::new());
    cur.get_mut().extend_from_slice(&data);
    finalize_recovered_prefix(&recovered, &mut cur).expect("finalize");
    data = cur.into_inner();
    let mut r = Reader::open(Cursor::new(&data)).expect("Reader::open after finalize");
    assert_eq!(r.message_count(), payloads.len() as u64);
    let collected: Vec<Vec<u8>> = r.iter_messages().collect::<Result<_, _>>().expect("iter");
    assert_eq!(collected, payloads);
}
#[test]
fn finalized_file_byte_identical_to_writer_finish() {
    let payloads = payloads();
    let (data, manifest) = write_synced_prefix(&payloads);
    let recovered =
        recover_footerless_prefix(&data, manifest.get_ref()).expect("recover_footerless_prefix");
    let mut cur = Cursor::new(data);
    finalize_recovered_prefix(&recovered, &mut cur).expect("finalize");
    let finalized = cur.into_inner();
    let mut one_shot: Vec<u8> = Vec::new();
    {
        let mut w = Writer::new(&mut one_shot, KNOWN_HASH);
        for p in &payloads {
            w.write_message(p).expect("write");
        }
        w.finish().expect("finish");
    }
    assert_eq!(
        finalized, one_shot,
        "finalize-from-prefix must produce byte-identical output to Writer::finish",
    );
}
#[test]
fn recovery_rejects_mismatched_schema_hash() {
    let payloads = payloads();
    let (data, mut manifest) = write_synced_prefix(&payloads);
    let buf = manifest.get_mut();
    let target = 8;
    buf[target] ^= 0xFF;
    let err =
        recover_footerless_prefix(&data, buf).expect_err("manifest tampering must be detected");
    assert!(
        matches!(
            err,
            RecoveryError::Manifest(_) | RecoveryError::SchemaHashMismatch { .. }
        ),
        "expected Manifest or SchemaHashMismatch, got {err:?}",
    );
}
#[test]
fn recovery_rejects_when_data_end_exceeds_pgno_length() {
    let payloads = payloads();
    let (mut data, manifest) = write_synced_prefix(&payloads);
    data.truncate(data.len() - 1);
    let err = recover_footerless_prefix(&data, manifest.get_ref())
        .expect_err("must detect truncated .pgno");
    assert!(
        matches!(err, RecoveryError::DataEndExceedsFile { .. }),
        "expected DataEndExceedsFile, got {err:?}",
    );
}
#[test]
fn recovery_validates_per_message_xxh64() {
    let payloads = payloads();
    let (mut data, manifest) = write_synced_prefix(&payloads);
    let target = pardosa_file::format::messages_offset(0);
    data[target] ^= 0xFF;
    let err = recover_footerless_prefix(&data, manifest.get_ref())
        .expect_err("must detect body corruption");
    assert!(
        matches!(err, RecoveryError::BodyChecksumMismatch { .. }),
        "expected BodyChecksumMismatch, got {err:?}",
    );
}
#[test]
fn finalize_discards_pgno_tail_beyond_manifest_data_end() {
    let payloads = payloads();
    let (mut data, manifest) = write_synced_prefix(&payloads);
    data.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE]);
    let recovered = recover_footerless_prefix(&data, manifest.get_ref())
        .expect("recover (tail bytes are ok, they will be truncated)");
    let mut cur = Cursor::new(data);
    finalize_recovered_prefix(&recovered, &mut cur).expect("finalize");
    let finalized = cur.into_inner();
    let mut r = Reader::open(Cursor::new(&finalized)).expect("Reader::open");
    let collected: Vec<Vec<u8>> = r.iter_messages().collect::<Result<_, _>>().expect("iter");
    assert_eq!(collected, payloads);
}
#[test]
fn manifest_with_no_syncs_recovers_zero_messages() {
    let mut data_sink: Vec<u8> = Vec::new();
    let mut manifest_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    {
        let mut w = AppendWriter::new(&mut data_sink, KNOWN_HASH).with_manifest(&mut manifest_sink);
        w.sync_data().expect("sync zero messages");
    }
    let recovered =
        recover_footerless_prefix(&data_sink, manifest_sink.get_ref()).expect("recover empty");
    assert!(recovered.records.is_empty());
    let mut cur = Cursor::new(data_sink);
    finalize_recovered_prefix(&recovered, &mut cur).expect("finalize empty");
    let finalized = cur.into_inner();
    let mut r = Reader::open(Cursor::new(&finalized)).expect("Reader::open empty");
    assert_eq!(r.message_count(), 0);
    let collected: Vec<Vec<u8>> = r
        .iter_messages()
        .collect::<Result<_, _>>()
        .expect("iter empty");
    assert!(collected.is_empty());
}
#[test]
fn record_shape_is_serializable_to_pgno_index_entry() {
    let r = ManifestRecord {
        offset: 40,
        size: 5,
        checksum: 0xABCD_1234_5678_9ABC,
    };
    assert_eq!(r.offset, 40);
    assert_eq!(r.size, 5);
    assert_eq!(r.checksum, 0xABCD_1234_5678_9ABC);
}
