//! Admission seam: `EventStore::<T>::open_with_backend` accepts a
//! sealed `JetStream` handle alongside [`PgnoBackend`], returning
//! the single typed `EventStore<T>` alias in both cases.
//!
//! Pins the type-level handshake: alias stays single-generic;
//! `open_with_backend` stays the sole typed-backend admission
//! name; no `EventStore<T, B>` widening leaks.
//!
//! Offline coverage; live end-to-end lives in
//! `tests/live_jetstream_authoritative_recovery.rs` (`#[ignore]`).
use pardosa::store::{
    AuthoritativeBackend, EventStore, GenomeSafe, HasEventSchemaSource, JetStreamBackend,
    PgnoBackend,
};
use pardosa_nats::{JetStreamBackend as SubstrateJetStreamBackend, JetStreamConfig, RuntimeHandle};
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Ledger {
    seq: u64,
}
impl HasEventSchemaSource for Ledger {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
fn detached_config(tag: &str) -> JetStreamConfig {
    JetStreamConfig::builder()
        .stream_name(format!("OPEN_WITH_BACKEND_{tag}"))
        .subject(format!("open_with_backend.{tag}"))
        .durable_consumer(format!("open-with-backend-c-{tag}"))
        .runtime_handle(RuntimeHandle::detached_for_tests())
        .build()
        .expect("offline config")
}
#[test]
fn open_with_backend_admits_jetstream_handle_at_type_level() {
    fn requires_authoritative_backend<B: AuthoritativeBackend>(_: &B) {}
    let handle = SubstrateJetStreamBackend::open(detached_config("type-level"));
    let backend: JetStreamBackend = JetStreamBackend::open(handle);
    requires_authoritative_backend(&backend);
}
#[test]
fn open_with_backend_admits_jetstream_at_type_level_and_call_compiles() {
    let handle = SubstrateJetStreamBackend::open(detached_config("call-compiles"));
    let backend: JetStreamBackend = JetStreamBackend::open(handle);
    let result: Result<EventStore<Ledger>, _> = EventStore::<Ledger>::open_with_backend(backend);
    assert!(
        result.is_err(),
        "detached-for-tests substrate handle must surface a substrate-side fetch error \
         (not panic, not silently succeed) when admission seam dispatches into the \
         JetStream rehydrate path; the live recovery gate covers the network-positive arm",
    );
}
#[test]
fn open_with_backend_still_admits_pgno_backend_and_observes_committed_events() {
    use std::path::PathBuf;
    let mut tmp: PathBuf = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    tmp.push(format!(
        "pardosa-open-with-backend-pgno-admit-{}-{nanos}.pgno",
        std::process::id()
    ));
    let written_id = {
        let mut store: EventStore<Ledger> =
            EventStore::<Ledger>::create(&tmp).expect("create scratch .pgno");
        let receipt = store
            .writer()
            .begin(Ledger { seq: 1 })
            .expect("begin first event");
        let event_id = receipt.event_id();
        let _ = receipt.fiber();
        let _ = store.writer().sync().expect("sync");
        event_id
    };
    let backend = PgnoBackend::open(&tmp);
    let store: EventStore<Ledger> =
        EventStore::<Ledger>::open_with_backend(backend).expect("open_with_backend(PgnoBackend)");
    let frontier_after = store.reader().frontier();
    assert_ne!(
        frontier_after,
        pardosa::store::Frontier::GENESIS,
        "PgnoBackend admission must surface the committed event in the reader frontier \
         (sub-mission 03b parity remains intact after the open_with_backend signature \
         lifts to impl AuthoritativeBackend); committed event id: {written_id:?}",
    );
    let _ = std::fs::remove_file(&tmp);
}
