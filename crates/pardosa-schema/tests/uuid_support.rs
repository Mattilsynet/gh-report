//! Native raw `uuid::Uuid` support for event/genome
//! payloads.
//!
//! Pins mission `native-uuid-support-20260526`:
//!
//! 1. `uuid::Uuid: EventSafe + GenomeSafe + GenomeOrd` via
//!    in-tree feature-gated impls; no wrapper.
//! 2. Wire layout = 16 raw bytes — no length prefix, no tag
//!    (GEN-0035: fixed-size payloads serialize natural).
//! 3. `#[derive(GenomeSafe)]` accepts a raw `uuid::Uuid`
//!    field directly.
//!
//! Gated `feature = "uuid"`; default features cover it.
#![cfg(feature = "uuid")]
use pardosa_schema::genome_safe::schema_hash_bytes;
use pardosa_schema::{GenomeOrd, GenomeSafe, from_bytes, to_vec};
fn assert_event_safe<T: pardosa_schema::EventSafe>() {}
fn assert_genome_safe<T: GenomeSafe>() {}
fn assert_genome_ord<T: GenomeOrd>() {}
#[test]
fn uuid_is_event_safe_and_genome_safe_and_genome_ord() {
    assert_event_safe::<uuid::Uuid>();
    assert_genome_safe::<uuid::Uuid>();
    assert_genome_ord::<uuid::Uuid>();
}
#[test]
fn uuid_wire_layout_is_16_raw_bytes() {
    let raw: [u8; 16] = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE,
        0xFF,
    ];
    let u = uuid::Uuid::from_bytes(raw);
    let wire = to_vec(&u);
    assert_eq!(wire.len(), 16, "Uuid must serialize to exactly 16 bytes");
    assert_eq!(
        wire.as_slice(),
        &raw[..],
        "Uuid wire bytes must equal as_bytes()"
    );
    let back: uuid::Uuid = from_bytes(&wire).expect("roundtrip");
    assert_eq!(back, u);
}
#[test]
fn uuid_schema_hash_is_stable_and_named() {
    let expected = schema_hash_bytes(b"uuid::Uuid");
    assert_eq!(<uuid::Uuid as GenomeSafe>::SCHEMA_HASH, expected);
    assert_eq!(<uuid::Uuid as GenomeSafe>::SCHEMA_SOURCE, "uuid::Uuid");
}
#[derive(pardosa_schema::GenomeSafe)]
struct Session {
    id: uuid::Uuid,
    seq: u64,
}
#[test]
fn derive_accepts_raw_uuid_field() {
    let s = Session {
        id: uuid::Uuid::from_bytes([7u8; 16]),
        seq: 0x0102_0304_0506_0708,
    };
    let wire = to_vec(&s);
    assert_eq!(wire.len(), 16 + 8, "struct wire = uuid(16) + u64(8)");
    assert_eq!(&wire[..16], &[7u8; 16][..]);
    assert_eq!(&wire[16..], &0x0102_0304_0506_0708u64.to_le_bytes()[..]);
    let back: Session = from_bytes(&wire).expect("roundtrip");
    assert_eq!(back.id, s.id);
    assert_eq!(back.seq, s.seq);
    let _ = <Session as GenomeSafe>::SCHEMA_SOURCE;
}
