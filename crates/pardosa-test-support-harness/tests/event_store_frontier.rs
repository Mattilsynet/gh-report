//! BLAKE3 chain-frontier observability on the public reader surface
//! (mission `invariant-port-20260601` Port 03).
//!
//! `StoreReader::frontier()` exposes the rolling BLAKE3 commitment
//! over the full event line as a public observable:
//!
//! - GENESIS on a freshly created empty store;
//! - advances on every append;
//! - survives `sync` + `EventStore::open` (Port 01 re-fold path).
//!
//! Adopters can diff frontiers cross-replica or feed them to a
//! downstream attestor without crossing a `pub(crate)` boundary.
use pardosa::store::{EventStore, Frontier, GenomeSafe, HasEventSchemaSource};
use tempfile::TempDir;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Payload {
    v: u64,
}
impl HasEventSchemaSource for Payload {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
#[test]
fn fresh_store_frontier_is_genesis() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let store = EventStore::<Payload>::create(&journal).expect("create");
    assert_eq!(store.reader().frontier(), Frontier::GENESIS);
}
#[test]
fn frontier_advances_after_each_append() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let mut store = EventStore::<Payload>::create(&journal).expect("create");
    let f0 = store.reader().frontier();
    assert_eq!(f0, Frontier::GENESIS);
    let _ = store.writer().begin(Payload { v: 1 }).expect("begin 1");
    let f1 = store.reader().frontier();
    assert_ne!(f1, f0, "frontier must change on first append");
    let _ = store.writer().begin(Payload { v: 2 }).expect("begin 2");
    let f2 = store.reader().frontier();
    assert_ne!(f2, f1, "frontier must change on second append");
}
#[test]
fn frontier_is_deterministic_across_two_stores() {
    let td_a = TempDir::new().expect("tempdir a");
    let td_b = TempDir::new().expect("tempdir b");
    let journal_a = td_a.path().join("a.pgno");
    let journal_b = td_b.path().join("b.pgno");
    let mut sa = EventStore::<Payload>::create(&journal_a).expect("create a");
    let mut sb = EventStore::<Payload>::create(&journal_b).expect("create b");
    for v in 0..5 {
        let _ = sa.writer().begin(Payload { v }).expect("a begin");
        let _ = sb.writer().begin(Payload { v }).expect("b begin");
    }
    assert_eq!(
        sa.reader().frontier(),
        sb.reader().frontier(),
        "same event sequence → same frontier"
    );
}
#[test]
fn frontier_survives_sync_and_reopen() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let before = {
        let mut store = EventStore::<Payload>::create(&journal).expect("create");
        let _ = store.writer().begin(Payload { v: 10 }).expect("begin 10");
        let _ = store.writer().begin(Payload { v: 20 }).expect("begin 20");
        let _ = store.writer().begin(Payload { v: 30 }).expect("begin 30");
        let _ = store.writer().sync().expect("sync");
        store.reader().frontier()
    };
    let reopened = EventStore::<Payload>::open(&journal).expect("reopen");
    assert_eq!(
        reopened.reader().frontier(),
        before,
        "reopened frontier must match the pre-sync rolling value (Port 01 re-fold path)"
    );
}
