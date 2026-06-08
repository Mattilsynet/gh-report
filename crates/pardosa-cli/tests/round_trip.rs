use pardosa::store::EventStore;
use pardosa_cli::DomainEvent;
use pardosa_cli::event::limits::{
    MAX_BATCH_ID, MAX_DOMAIN_KEY, MAX_EVIDENCE, MAX_ORG, MAX_REPO_NAME, MAX_SNAPSHOT_SIG,
    MAX_SOURCE,
};
use pardosa_schema::{EventString, NonEmptyEventString, Timestamp};
use tempfile::tempdir;
const EXPECTED_SCHEMA_SOURCE: &str = "pardosa-cli/DomainEvent";
fn nes<const MAX: usize>(s: &str) -> NonEmptyEventString<MAX> {
    NonEmptyEventString::try_new(s).expect("fits MAX, nonempty")
}
fn es<const MAX: usize>(s: &str) -> EventString<MAX> {
    EventString::try_from(s.to_string()).expect("fits MAX")
}
fn ts(nanos: u64) -> Timestamp {
    Timestamp::from_nanos(nanos).expect("nonzero")
}
fn sample_events() -> Vec<DomainEvent> {
    vec![
        DomainEvent::SweepStarted {
            org: nes::<MAX_ORG>("acme"),
            repo_count: 3,
            batch_id: nes::<MAX_BATCH_ID>("b-001"),
            timestamp: ts(1),
            snapshot_signature: Some(es::<MAX_SNAPSHOT_SIG>("sig-abc")),
        },
        DomainEvent::RepoEvaluated {
            domain_key: nes::<MAX_DOMAIN_KEY>("rust"),
            repo_name: nes::<MAX_REPO_NAME>("acme/widget"),
            success: true,
            source: nes::<MAX_SOURCE>("github"),
            duration_ms: 1234,
            timestamp: ts(2),
            evidence: Some(es::<MAX_EVIDENCE>("blurb")),
        },
        DomainEvent::SweepCompleted {
            batch_id: nes::<MAX_BATCH_ID>("b-001"),
            duration_ms: 9999,
            repo_count: 3,
            timestamp: ts(3),
        },
    ]
}
#[test]
fn round_trip_three_events_via_store() {
    let originals = sample_events();
    let originals_debug: Vec<String> = originals.iter().map(|e| format!("{e:?}")).collect();
    let dir = tempdir().expect("tempdir");
    let store_path = dir.path().join("round-trip.pgno");
    let sidecar = dir.path().join("round-trip.sidecar");
    {
        let mut store: EventStore<DomainEvent> =
            EventStore::<DomainEvent>::create(&store_path).expect("create store");
        let mut writer = store.writer();
        for e in originals {
            let _ = writer.begin(e).expect("begin");
        }
        let _lsn = writer.sync().expect("sync");
    }
    let store: EventStore<DomainEvent> =
        EventStore::<DomainEvent>::open(&store_path).expect("open store");
    let reader = store.reader();
    let mut cursor = reader.cursor(&sidecar).expect("cursor open");
    let read_back: Vec<String> = cursor
        .tail()
        .map(|r| format!("{:?}", r.expect("decode").into_inner()))
        .collect();
    assert_eq!(read_back.len(), originals_debug.len(), "read-back length");
    for (i, (a, b)) in originals_debug.iter().zip(read_back.iter()).enumerate() {
        assert_eq!(a, b, "event {i} differs post-round-trip");
    }
    let meta = EventStore::<DomainEvent>::metadata(&store_path).expect("metadata");
    assert_eq!(
        meta.schema_source(),
        Some(EXPECTED_SCHEMA_SOURCE),
        "schema source round-trips via EventStore::create"
    );
    assert_eq!(
        meta.len(),
        u64::try_from(originals_debug.len()).expect("fits u64"),
        "container message_count matches written event count"
    );
}
