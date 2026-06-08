//! Round-trip equivalence for `EventStore::<T>::open_with_backend`
//! (ADR-0022 §D1, §D12).
//!
//! Pins that the typed-handle constructor is observationally
//! interchangeable with [`EventStore::open`] when the supplied
//! [`PgnoBackend`] targets the same `.pgno` path.
//!
//! Also pins that [`PgnoBackend`] and [`AuthoritativeBackend`]
//! are reachable from `pardosa::store` (ADR-0018 §D1) and that
//! `PgnoBackend::open` accepts `impl Into<PathBuf>`.
use pardosa::store::{
    AuthoritativeBackend, EventId, EventStore, FiberId, GenomeSafe, HasEventSchemaSource,
    PgnoBackend,
};
use std::path::Path;
use tempfile::TempDir;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Order {
    id: u64,
    amount_cents: u64,
}
impl HasEventSchemaSource for Order {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
struct WrittenIdentity {
    fid: FiberId,
    head: EventId,
}
fn create_one_fiber_then_sync(journal: &Path) -> WrittenIdentity {
    let mut store: EventStore<Order> = EventStore::<Order>::create(journal).expect("create");
    let mut writer = store.writer();
    let begin = writer
        .begin(Order {
            id: 1,
            amount_cents: 100,
        })
        .expect("begin");
    let begin_id = begin.event_id();
    let live = begin.fiber();
    let fid = live.fiber_id();
    let append = writer
        .append(
            live,
            Order {
                id: 1,
                amount_cents: 200,
            },
        )
        .expect("append");
    let head = append.event_id();
    let _live = append.fiber();
    let _lsn = writer.sync().expect("sync");
    assert_ne!(
        begin_id, head,
        "begin and append produce distinct event ids"
    );
    WrittenIdentity { fid, head }
}
fn reread_history_ids(store: &EventStore<Order>, fid: FiberId) -> Vec<EventId> {
    let reader = store.reader();
    reader
        .fiber(fid)
        .iter()
        .expect("fiber history present after rehydrate")
        .map(pardosa::prelude::Event::event_id)
        .collect()
}
#[test]
fn open_with_backend_rehydrates_same_state_as_open() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let written = create_one_fiber_then_sync(&journal);
    let store_via_path: EventStore<Order> = EventStore::<Order>::open(&journal).expect("path open");
    let ids_via_path = reread_history_ids(&store_via_path, written.fid);
    drop(store_via_path);
    let backend = PgnoBackend::open(&journal);
    let store_via_backend: EventStore<Order> =
        EventStore::<Order>::open_with_backend(backend).expect("open_with_backend");
    let ids_via_backend = reread_history_ids(&store_via_backend, written.fid);
    assert_eq!(
        ids_via_path, ids_via_backend,
        "rehydrated fiber-history ids equal across constructors"
    );
    let head_via_backend = *ids_via_backend
        .last()
        .expect("rehydrated history has at least one event");
    assert_eq!(
        head_via_backend, written.head,
        "rehydrated head equal to originally written head"
    );
}
#[test]
fn pgno_backend_open_accepts_path_and_pathbuf() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let _ = create_one_fiber_then_sync(&journal);
    let _by_ref: PgnoBackend = PgnoBackend::open(&journal);
    let _by_buf: PgnoBackend = PgnoBackend::open(journal.clone());
}
#[test]
fn pgno_backend_satisfies_authoritative_backend_marker() {
    fn requires_authoritative_backend<B: AuthoritativeBackend>(_: &B) {}
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let backend = PgnoBackend::open(&journal);
    requires_authoritative_backend(&backend);
}
