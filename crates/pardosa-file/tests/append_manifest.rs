use pardosa_file::manifest::{
    MANIFEST_FOOTER_SIZE, MANIFEST_HEADER_SIZE, MANIFEST_MAGIC, MANIFEST_RECORD_SIZE,
    MANIFEST_VERSION, ManifestRecord, ManifestSnapshot, parse_manifest, write_complete_manifest,
};
const KNOWN_HASH: u128 = 0x0F0E_0D0C_0B0A_0908_0706_0504_0302_0100;
const FRONTIER: [u8; 32] = [0xA5; 32];
fn rec(offset: u64, size: u32, checksum: u64) -> ManifestRecord {
    ManifestRecord {
        offset,
        size,
        checksum,
    }
}
#[test]
fn manifest_version_bumped_to_two_for_frontier_footer() {
    assert_eq!(MANIFEST_VERSION, 2);
    assert_eq!(MANIFEST_FOOTER_SIZE, 60);
}
#[test]
fn empty_manifest_round_trip() {
    let mut sink: Vec<u8> = Vec::new();
    write_complete_manifest(&mut sink, KNOWN_HASH, 0, 0, &[], 0, FRONTIER).expect("write");
    assert_eq!(
        sink.len(),
        MANIFEST_HEADER_SIZE + MANIFEST_FOOTER_SIZE,
        "empty manifest = header + footer only",
    );
    let snap = parse_manifest(&sink).expect("parse empty");
    assert_eq!(snap.schema_hash, KNOWN_HASH);
    assert_eq!(snap.page_class, 0);
    assert_eq!(snap.schema_size, 0);
    assert!(snap.records.is_empty());
    assert_eq!(snap.data_end, 0);
    assert_eq!(snap.frontier, Some(FRONTIER));
}
#[test]
fn populated_manifest_round_trip() {
    let records = vec![
        rec(40, 5, 0xABCD_1234_5678_9ABC),
        rec(45, 3, 0xDEAD_BEEF_CAFE_F00D),
        rec(48, 10, 0x1111_2222_3333_4444),
    ];
    let data_end = 58;
    let mut sink: Vec<u8> = Vec::new();
    write_complete_manifest(&mut sink, KNOWN_HASH, 7, 0, &records, data_end, FRONTIER)
        .expect("write");
    assert_eq!(
        sink.len(),
        MANIFEST_HEADER_SIZE + records.len() * MANIFEST_RECORD_SIZE + MANIFEST_FOOTER_SIZE,
    );
    let snap = parse_manifest(&sink).expect("parse populated");
    assert_eq!(snap.schema_hash, KNOWN_HASH);
    assert_eq!(snap.page_class, 7);
    assert_eq!(snap.schema_size, 0);
    assert_eq!(snap.records, records);
    assert_eq!(snap.data_end, data_end);
    assert_eq!(snap.frontier, Some(FRONTIER));
}
#[test]
fn v1_manifest_without_frontier_still_parses() {
    let records = vec![rec(40, 5, 0xABCD_1234_5678_9ABC)];
    let data_end = 45;
    let mut sink: Vec<u8> = Vec::new();
    write_legacy_v1_manifest(&mut sink, KNOWN_HASH, 0, 0, &records, data_end).expect("write v1");
    let snap = parse_manifest(&sink).expect("parse v1");
    assert_eq!(snap.records, records);
    assert_eq!(snap.data_end, data_end);
    assert_eq!(snap.frontier, None);
}
#[test]
fn manifest_header_starts_with_distinct_magic() {
    let mut sink: Vec<u8> = Vec::new();
    write_complete_manifest(&mut sink, KNOWN_HASH, 0, 0, &[], 0, FRONTIER).expect("write");
    assert_eq!(
        &sink[..MANIFEST_MAGIC.len()],
        MANIFEST_MAGIC,
        "manifest magic must be distinct from .pgno PGNO magic",
    );
    assert_ne!(
        MANIFEST_MAGIC, b"PGNO",
        "manifest magic must NOT equal .pgno magic",
    );
}
#[test]
fn manifest_version_at_fixed_offset() {
    let mut sink: Vec<u8> = Vec::new();
    write_complete_manifest(&mut sink, KNOWN_HASH, 0, 0, &[], 0, FRONTIER).expect("write");
    let version = u16::from_le_bytes(sink[4..6].try_into().unwrap());
    assert_eq!(version, MANIFEST_VERSION);
}
#[test]
fn parse_rejects_wrong_magic() {
    let mut sink: Vec<u8> = Vec::new();
    write_complete_manifest(&mut sink, KNOWN_HASH, 0, 0, &[], 0, FRONTIER).expect("write");
    sink[0] = b'X';
    parse_manifest(&sink).expect_err("must reject wrong magic");
}
#[test]
fn parse_rejects_corrupted_record_checksum() {
    let records = vec![rec(40, 5, 0xABCD_1234_5678_9ABC)];
    let mut sink: Vec<u8> = Vec::new();
    write_complete_manifest(&mut sink, KNOWN_HASH, 0, 0, &records, 45, FRONTIER).expect("write");
    let target = MANIFEST_HEADER_SIZE + 1;
    sink[target] ^= 0xFF;
    parse_manifest(&sink).expect_err("must reject corrupted record");
}
#[test]
fn parse_rejects_corrupted_frontier_checksum() {
    let records = vec![rec(40, 5, 0xABCD_1234_5678_9ABC)];
    let mut sink: Vec<u8> = Vec::new();
    write_complete_manifest(&mut sink, KNOWN_HASH, 0, 0, &records, 45, FRONTIER).expect("write");
    let frontier_byte = sink.len() - MANIFEST_FOOTER_SIZE + 16;
    sink[frontier_byte] ^= 0xFF;
    parse_manifest(&sink).expect_err("must reject corrupted frontier");
}
#[test]
fn manifest_footer_stores_frontier_before_checksum_and_magic() {
    let records = vec![rec(40, 5, 0xABCD_1234_5678_9ABC)];
    let mut sink: Vec<u8> = Vec::new();
    write_complete_manifest(&mut sink, KNOWN_HASH, 0, 0, &records, 45, FRONTIER).expect("write");
    let footer_start = sink.len() - MANIFEST_FOOTER_SIZE;
    assert_eq!(&sink[footer_start + 16..footer_start + 48], FRONTIER);
    assert_eq!(&sink[footer_start + 56..footer_start + 60], MANIFEST_MAGIC);
}
#[test]
fn parse_rejects_truncated_file() {
    let records = vec![rec(40, 5, 0xABCD_1234_5678_9ABC)];
    let mut sink: Vec<u8> = Vec::new();
    write_complete_manifest(&mut sink, KNOWN_HASH, 0, 0, &records, 45, FRONTIER).expect("write");
    sink.truncate(sink.len() - 4);
    parse_manifest(&sink).expect_err("must reject truncated manifest");
}
#[test]
fn manifest_footer_magic_present_at_end() {
    let records = vec![rec(40, 5, 0xABCD_1234_5678_9ABC)];
    let mut sink: Vec<u8> = Vec::new();
    write_complete_manifest(&mut sink, KNOWN_HASH, 0, 0, &records, 45, FRONTIER).expect("write");
    let n = sink.len();
    assert_eq!(
        &sink[n - 4..],
        MANIFEST_MAGIC,
        "manifest must end with magic for tail-seek validation",
    );
}
#[test]
fn snapshot_pub_fields_constructible() {
    let snap = ManifestSnapshot {
        schema_hash: 0,
        page_class: 0,
        schema_size: 0,
        records: vec![],
        data_end: 0,
        frontier: Some(FRONTIER),
    };
    assert!(snap.records.is_empty());
}
fn write_legacy_v1_manifest<W: std::io::Write>(
    sink: &mut W,
    schema_hash: u128,
    page_class: u8,
    schema_size: u32,
    records: &[ManifestRecord],
    data_end: u64,
) -> std::io::Result<()> {
    let mut header = [0u8; MANIFEST_HEADER_SIZE];
    header[0..4].copy_from_slice(MANIFEST_MAGIC);
    header[4..6].copy_from_slice(&1u16.to_le_bytes());
    header[8..24].copy_from_slice(&schema_hash.to_le_bytes());
    header[24] = page_class;
    header[28..32].copy_from_slice(&schema_size.to_le_bytes());
    sink.write_all(&header)?;
    let mut hasher = xxhash_rust::xxh64::Xxh64::new(0);
    hasher.update(&header);
    for r in records {
        let mut record = [0u8; MANIFEST_RECORD_SIZE];
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
    footer[24..28].copy_from_slice(MANIFEST_MAGIC);
    sink.write_all(&footer)
}
