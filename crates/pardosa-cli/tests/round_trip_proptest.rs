use pardosa::store::EventStore;
use pardosa_cli::DomainEvent;
use pardosa_cli::event::limits::{MAX_BATCH_ID, MAX_DOMAIN_KEY, MAX_ORG, MAX_REPO_NAME};
use pardosa_schema::{NonEmptyEventString, Timestamp};
use proptest::prelude::*;
use tempfile::tempdir;
type EventSeed = (u8, String, String, u64, u64, u64);
fn nes<const MAX: usize>(s: &str) -> NonEmptyEventString<MAX> {
    NonEmptyEventString::try_new(s).expect("seed fits MAX, nonempty")
}
fn ts(nanos: u64) -> Timestamp {
    Timestamp::from_nanos(nanos.max(1)).expect("max(1) is nonzero")
}
fn seed_to_event((tag, sa, sb, ua, ub, tn): EventSeed) -> DomainEvent {
    match tag % 3 {
        0 => DomainEvent::SweepStarted {
            org: nes::<MAX_ORG>(&sa),
            repo_count: ua,
            batch_id: nes::<MAX_BATCH_ID>(&sb),
            timestamp: ts(tn),
            snapshot_signature: None,
        },
        1 => DomainEvent::RepoRemoved {
            domain_key: nes::<MAX_DOMAIN_KEY>(&sa),
            repo_name: nes::<MAX_REPO_NAME>(&sb),
            timestamp: ts(tn),
        },
        _ => DomainEvent::SweepCompleted {
            batch_id: nes::<MAX_BATCH_ID>(&sa),
            duration_ms: ua,
            repo_count: ub,
            timestamp: ts(tn),
        },
    }
}
fn arb_seed() -> impl Strategy<Value = EventSeed> {
    (
        any::<u8>(),
        "[a-z]{1,8}",
        "[a-z]{1,8}",
        any::<u64>(),
        any::<u64>(),
        any::<u64>(),
    )
}
proptest! {
    #![proptest_config(ProptestConfig { cases : 32, ..ProptestConfig::default() })]
    #[test] fn round_trip_preserves_event_payloads_via_store(seeds in
    proptest::collection::vec(arb_seed(), 1..= 32),) { let originals : Vec < DomainEvent
    > = seeds.into_iter().map(seed_to_event).collect(); let originals_debug : Vec <
    String > = originals.iter().map(| e | format!("{e:?}")).collect(); let dir =
    tempdir().expect("tempdir"); let store_path = dir.path().join("proptest.pgno"); let
    sidecar = dir.path().join("proptest.sidecar"); { let mut store : EventStore <
    DomainEvent > = EventStore::< DomainEvent >::create(& store_path)
    .expect("create store"); let mut writer = store.writer(); for e in originals { let _
    = writer.begin(e).expect("begin"); } let _lsn = writer.sync().expect("sync"); } let
    store : EventStore < DomainEvent > = EventStore::< DomainEvent >::open(& store_path)
    .expect("open store"); let reader = store.reader(); let mut cursor = reader.cursor(&
    sidecar).expect("cursor open"); let read_back : Vec < String > = cursor.tail().map(|
    r | format!("{:?}", r.expect("decode").into_inner())).collect(); prop_assert_eq!(&
    read_back, & originals_debug); }
}
