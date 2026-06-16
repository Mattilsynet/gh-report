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
fn stamped_frontier(payloads: &[Vec<u8>]) -> [u8; 32] {
    payloads.iter().fold([0u8; 32], |frontier, body| {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&frontier);
        hasher.update(body);
        hasher.finalize().into()
    })
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
fn recovery_recomputes_frontier_over_raw_pgno_bodies() {
    let payloads = payloads();
    let (data, manifest) = write_synced_prefix(&payloads);
    let recovered =
        recover_footerless_prefix(&data, manifest.get_ref()).expect("recover_footerless_prefix");
    assert_eq!(recovered.frontier, Some(stamped_frontier(&payloads)));
}
#[test]
fn recovery_declines_frontier_mismatch_even_when_body_checksums_match() {
    let payloads = payloads();
    let (data, mut manifest) = write_synced_prefix(&payloads);
    let mut wrong = [0x5Au8; 32];
    wrong[0] ^= 0xA5;
    rewrite_manifest_frontier(manifest.get_mut(), wrong);
    let err = recover_footerless_prefix(&data, manifest.get_ref())
        .expect_err("wrong frontier must decline recovery");
    assert!(
        matches!(err, RecoveryError::FrontierMismatch { .. }),
        "expected FrontierMismatch, got {err:?}",
    );
}
#[test]
fn v1_manifest_recovers_with_checksum_only_fallback() {
    let payloads = payloads();
    let (data, manifest) = write_synced_prefix(&payloads);
    let v2 = pardosa_file::manifest::parse_manifest(manifest.get_ref()).expect("parse v2");
    let mut legacy_manifest = Vec::new();
    write_legacy_v1_manifest(
        &mut legacy_manifest,
        v2.schema_hash,
        v2.page_class,
        v2.schema_size,
        &v2.records,
        v2.data_end,
    )
    .expect("write v1");
    let recovered = recover_footerless_prefix(&data, &legacy_manifest).expect("recover v1");
    assert_eq!(recovered.records.len(), payloads.len());
    assert_eq!(recovered.frontier, None);
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
fn rewrite_manifest_frontier(manifest: &mut [u8], frontier: [u8; 32]) {
    let footer_start = manifest.len() - pardosa_file::manifest::MANIFEST_FOOTER_SIZE;
    let frontier_start = footer_start + 16;
    manifest[frontier_start..frontier_start + 32].copy_from_slice(&frontier);
    let checksum_start = footer_start + 48;
    let mut hasher = xxhash_rust::xxh64::Xxh64::new(0);
    hasher.update(&manifest[..footer_start]);
    hasher.update(&frontier);
    manifest[checksum_start..checksum_start + 8].copy_from_slice(&hasher.digest().to_le_bytes());
}
fn write_legacy_v1_manifest<W: std::io::Write>(
    sink: &mut W,
    schema_hash: u128,
    page_class: u8,
    schema_size: u32,
    records: &[ManifestRecord],
    data_end: u64,
) -> std::io::Result<()> {
    let mut header = [0u8; pardosa_file::manifest::MANIFEST_HEADER_SIZE];
    header[0..4].copy_from_slice(pardosa_file::manifest::MANIFEST_MAGIC);
    header[4..6].copy_from_slice(&1u16.to_le_bytes());
    header[8..24].copy_from_slice(&schema_hash.to_le_bytes());
    header[24] = page_class;
    header[28..32].copy_from_slice(&schema_size.to_le_bytes());
    sink.write_all(&header)?;
    let mut hasher = xxhash_rust::xxh64::Xxh64::new(0);
    hasher.update(&header);
    for r in records {
        let mut record = [0u8; pardosa_file::manifest::MANIFEST_RECORD_SIZE];
        record[0..8].copy_from_slice(&r.offset.to_le_bytes());
        record[8..12].copy_from_slice(&r.size.to_le_bytes());
        record[16..24].copy_from_slice(&r.checksum.to_le_bytes());
        sink.write_all(&record)?;
        hasher.update(&record);
    }
    let mut footer = [0u8; 28];
    footer[0..8].copy_from_slice(
        &u64::try_from(records.len())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?
            .to_le_bytes(),
    );
    footer[8..16].copy_from_slice(&data_end.to_le_bytes());
    footer[16..24].copy_from_slice(&hasher.digest().to_le_bytes());
    footer[24..28].copy_from_slice(pardosa_file::manifest::MANIFEST_MAGIC);
    sink.write_all(&footer)
}
