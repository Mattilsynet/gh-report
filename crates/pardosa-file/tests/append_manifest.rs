use pardosa_file::manifest::{
    MANIFEST_FOOTER_SIZE, MANIFEST_HEADER_SIZE, MANIFEST_MAGIC, MANIFEST_RECORD_SIZE,
    MANIFEST_VERSION, ManifestRecord, ManifestSnapshot, parse_manifest, write_complete_manifest,
};
const KNOWN_HASH: u128 = 0x0F0E_0D0C_0B0A_0908_0706_0504_0302_0100;
fn rec(offset: u64, size: u32, checksum: u64) -> ManifestRecord {
    ManifestRecord {
        offset,
        size,
        checksum,
    }
}
#[test]
fn empty_manifest_round_trip() {
    let mut sink: Vec<u8> = Vec::new();
    write_complete_manifest(&mut sink, KNOWN_HASH, 0, 0, &[], 0).expect("write");
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
    write_complete_manifest(&mut sink, KNOWN_HASH, 7, 0, &records, data_end).expect("write");
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
}
#[test]
fn manifest_header_starts_with_distinct_magic() {
    let mut sink: Vec<u8> = Vec::new();
    write_complete_manifest(&mut sink, KNOWN_HASH, 0, 0, &[], 0).expect("write");
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
    write_complete_manifest(&mut sink, KNOWN_HASH, 0, 0, &[], 0).expect("write");
    let version = u16::from_le_bytes(sink[4..6].try_into().unwrap());
    assert_eq!(version, MANIFEST_VERSION);
}
#[test]
fn parse_rejects_wrong_magic() {
    let mut sink: Vec<u8> = Vec::new();
    write_complete_manifest(&mut sink, KNOWN_HASH, 0, 0, &[], 0).expect("write");
    sink[0] = b'X';
    parse_manifest(&sink).expect_err("must reject wrong magic");
}
#[test]
fn parse_rejects_corrupted_record_checksum() {
    let records = vec![rec(40, 5, 0xABCD_1234_5678_9ABC)];
    let mut sink: Vec<u8> = Vec::new();
    write_complete_manifest(&mut sink, KNOWN_HASH, 0, 0, &records, 45).expect("write");
    let target = MANIFEST_HEADER_SIZE + 1;
    sink[target] ^= 0xFF;
    parse_manifest(&sink).expect_err("must reject corrupted record");
}
#[test]
fn parse_rejects_truncated_file() {
    let records = vec![rec(40, 5, 0xABCD_1234_5678_9ABC)];
    let mut sink: Vec<u8> = Vec::new();
    write_complete_manifest(&mut sink, KNOWN_HASH, 0, 0, &records, 45).expect("write");
    sink.truncate(sink.len() - 4);
    parse_manifest(&sink).expect_err("must reject truncated manifest");
}
#[test]
fn manifest_footer_magic_present_at_end() {
    let records = vec![rec(40, 5, 0xABCD_1234_5678_9ABC)];
    let mut sink: Vec<u8> = Vec::new();
    write_complete_manifest(&mut sink, KNOWN_HASH, 0, 0, &records, 45).expect("write");
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
    };
    assert!(snap.records.is_empty());
}
