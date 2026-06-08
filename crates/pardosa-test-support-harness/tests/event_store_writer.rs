//! Runtime contract for the `pardosa::store` writer and
//! `EventStore::open` (ADR-0018 §D1/§D2/§D4/§D7).
//!
//! Pins payload-only authoring, `LiveFiber`/`DetachedFiber`
//! typestate, `sync`→`Lsn` shape with monotonic `acked_lsn`,
//! `create`→`sync`→`open` round-trip rehydration, the no-auto-
//! migrate hard error on schema-hash mismatch, and
//! `StoreWriter::reader` capability re-export (ADR-0016 §D1).
//! Compile-fail twins live in `tests/ui` and the module-reach
//! compile-pass gate in `event_store_negative_gates`.
use pardosa::store::{
    AppendReceipt, Decode, DetachReceipt, DetachedFiber, Encode, EventStore, GenomeSafe,
    HasEventSchemaSource, LiveFiber, Lsn,
};
use tempfile::TempDir;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct PayloadA {
    v: u64,
}
impl HasEventSchemaSource for PayloadA {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct PayloadB {
    tag: u8,
    v: u32,
}
impl HasEventSchemaSource for PayloadB {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
fn new_store_tmp<T>() -> (EventStore<T>, TempDir)
where
    T: Encode + Decode + GenomeSafe + HasEventSchemaSource,
{
    let td = TempDir::new().expect("tempdir");
    let store = EventStore::<T>::create(&td.path().join("journal.pgno"))
        .expect("create path-backed EventStore");
    (store, td)
}
#[test]
fn begin_mints_envelope_identity_from_payload_only() {
    let (mut store, _td) = new_store_tmp::<PayloadA>();
    let mut writer = store.writer();
    let r0 = writer.begin(PayloadA { v: 1 }).expect("begin");
    let r1 = writer.begin(PayloadA { v: 2 }).expect("begin");
    let id0 = r0.event_id();
    let id1 = r1.event_id();
    let f0 = r0.fiber().fiber_id();
    let f1 = r1.fiber().fiber_id();
    assert_ne!(id0, id1, "distinct events must mint distinct EventIds");
    assert_eq!(
        id0.value() + 1,
        id1.value(),
        "EventIds are minted in commit order"
    );
    assert_ne!(f0, f1, "distinct begin calls must mint distinct FiberIds");
}
#[test]
fn append_continues_same_fiber_with_fresh_event_id() {
    let (mut store, _td) = new_store_tmp::<PayloadA>();
    let mut writer = store.writer();
    let r0 = writer.begin(PayloadA { v: 10 }).expect("begin");
    let id0 = r0.event_id();
    let live: LiveFiber = r0.fiber();
    let fid = live.fiber_id();
    let r1: AppendReceipt = writer.append(live, PayloadA { v: 11 }).expect("append");
    let id1 = r1.event_id();
    assert_eq!(
        r1.fiber().fiber_id(),
        fid,
        "append must continue the same fiber"
    );
    assert_ne!(id1, id0, "each append mints a fresh EventId");
}
#[test]
fn detach_then_resume_preserves_fiber_identity_through_facade() {
    let (mut store, _td) = new_store_tmp::<PayloadA>();
    let mut writer = store.writer();
    let live = writer.begin(PayloadA { v: 100 }).expect("begin").fiber();
    let original_fid = live.fiber_id();
    let dr: DetachReceipt = writer.detach(live, PayloadA { v: 101 }).expect("detach");
    let detached: DetachedFiber = dr.fiber();
    assert_eq!(
        detached.fiber_id(),
        original_fid,
        "detach must preserve FiberId at the facade layer"
    );
    let r: AppendReceipt = writer
        .resume(detached, PayloadA { v: 102 })
        .expect("resume");
    let live_again: LiveFiber = r.fiber();
    assert_eq!(
        live_again.fiber_id(),
        original_fid,
        "resume must preserve FiberId at the facade layer"
    );
    let r_next = writer
        .append(live_again, PayloadA { v: 103 })
        .expect("append post-resume");
    assert_eq!(r_next.fiber().fiber_id(), original_fid);
}
#[test]
fn sync_returns_lsn_and_acked_lsn_is_monotonic() {
    let (mut store, _td) = new_store_tmp::<PayloadA>();
    let mut writer = store.writer();
    assert_eq!(
        writer.acked_lsn(),
        None,
        "acked_lsn must be None before any sync"
    );
    let _ = writer.begin(PayloadA { v: 1 }).expect("begin");
    let lsn1 = writer.sync().expect("sync 1");
    assert_eq!(writer.acked_lsn(), Some(lsn1));
    let _ = writer.begin(PayloadA { v: 2 }).expect("begin");
    let lsn2 = writer.sync().expect("sync 2");
    assert_eq!(writer.acked_lsn(), Some(lsn2));
    assert!(
        lsn2 >= lsn1,
        "Lsn must be monotonic non-decreasing across syncs: {lsn1:?} -> {lsn2:?}"
    );
}
#[test]
fn sync_signature_is_payload_only_no_durability_param() {
    let (mut store, _td) = new_store_tmp::<PayloadA>();
    let mut writer = store.writer();
    let _ = writer.begin(PayloadA { v: 0 }).expect("begin");
    let lsn: Lsn = writer.sync().expect("sync returns Lsn on Ok");
    let _ = lsn;
}
#[test]
fn writer_reader_observes_writer_appends_within_same_borrow_scope() {
    let (mut store, _td) = new_store_tmp::<PayloadA>();
    let mut writer = store.writer();
    let r0 = writer.begin(PayloadA { v: 7 }).expect("begin");
    let live = r0.fiber();
    let fid = live.fiber_id();
    let _r1 = writer.append(live, PayloadA { v: 8 }).expect("append");
    let reader = writer.reader();
    let hist = reader.fiber(fid);
    let payloads: Vec<u64> = hist
        .iter()
        .expect("history iter")
        .map(|e| e.domain_event().v)
        .collect();
    assert_eq!(payloads, vec![7, 8]);
}
#[test]
fn file_backed_create_sync_open_round_trips_events_and_continues_writer() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar1 = td.path().join("ack1.sidecar");
    let sidecar2 = td.path().join("ack2.sidecar");
    {
        let mut store: EventStore<PayloadA> =
            EventStore::<PayloadA>::create(&journal).expect("create");
        let mut writer = store.writer();
        let r0 = writer.begin(PayloadA { v: 1 }).expect("begin");
        let live = r0.fiber();
        let _ = writer.append(live, PayloadA { v: 2 }).expect("append");
        let _ = writer.sync().expect("sync");
    }
    let mut store: EventStore<PayloadA> = EventStore::<PayloadA>::open(&journal).expect("open");
    let fids: Vec<_> = {
        let reader = store.reader();
        let mut cur = reader.cursor(&sidecar1).expect("cursor 1");
        cur.tail()
            .map(|r| r.expect("tail item").fiber_id())
            .collect()
    };
    assert_eq!(
        fids.len(),
        2,
        "rehydrate must surface the two persisted events"
    );
    let mut writer = store.writer();
    let r2 = writer.begin(PayloadA { v: 3 }).expect("begin");
    assert_ne!(
        r2.fiber().fiber_id(),
        fids[0],
        "post-open writer must mint a fresh FiberId distinct from rehydrated ones"
    );
    let _ = writer.sync().expect("sync after open");
    let reader = store.reader();
    let mut cur = reader.cursor(&sidecar2).expect("cursor 2");
    let payloads: Vec<u64> = cur
        .tail()
        .map(|r| r.expect("tail item").domain_event().v)
        .collect();
    assert_eq!(
        payloads,
        vec![1, 2, 3],
        "post-open writer must extend the persisted line in commit order"
    );
}
#[test]
fn open_does_not_auto_migrate_schema_hash_mismatch_is_hard_error() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    {
        let mut store: EventStore<PayloadA> =
            EventStore::<PayloadA>::create(&journal).expect("create");
        let mut writer = store.writer();
        let _ = writer.begin(PayloadA { v: 1 }).expect("begin");
        let _ = writer.sync().expect("sync");
    }
    let err = EventStore::<PayloadB>::open(&journal)
        .err()
        .expect("schema-hash mismatch must surface as PardosaError");
    let msg = format!("{err}");
    assert!(
        msg.contains("cursor read") || msg.contains("schema") || msg.contains("hash"),
        "expected schema-hash-mismatch wrapping; got: {msg}"
    );
}
#[test]
fn open_on_missing_file_errors() {
    let td = TempDir::new().expect("tempdir");
    let missing = td.path().join("missing.pgno");
    let err = EventStore::<PayloadA>::open(&missing)
        .err()
        .expect("opening a non-existent file must error");
    let _ = format!("{err}");
}
#[test]
fn create_overwrites_existing_file() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    {
        let mut store: EventStore<PayloadA> =
            EventStore::<PayloadA>::create(&journal).expect("create 1");
        let mut writer = store.writer();
        let _ = writer.begin(PayloadA { v: 999 }).expect("begin");
        let _ = writer.sync().expect("sync");
    }
    {
        let mut store: EventStore<PayloadA> =
            EventStore::<PayloadA>::create(&journal).expect("create 2 (overwrite)");
        let mut writer = store.writer();
        let _ = writer.begin(PayloadA { v: 1 }).expect("begin");
        let _ = writer.sync().expect("sync");
    }
    let store: EventStore<PayloadA> =
        EventStore::<PayloadA>::open(&journal).expect("open after overwrite");
    let reader = store.reader();
    let mut cur = reader.cursor(&sidecar).expect("cursor");
    let payloads: Vec<u64> = cur
        .tail()
        .map(|r| r.expect("tail item").domain_event().v)
        .collect();
    assert_eq!(
        payloads,
        vec![1],
        "second create must truncate the prior file content"
    );
}
#[test]
fn unsynced_appends_are_in_memory_only_and_do_not_survive_drop_before_sync() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    let (synced_id, minted_unsynced) = {
        let mut store: EventStore<PayloadA> =
            EventStore::<PayloadA>::create(&journal).expect("create");
        let mut writer = store.writer();
        let r_synced = writer.begin(PayloadA { v: 40 }).expect("begin synced");
        let synced_id = r_synced.event_id();
        let live = r_synced.fiber();
        let _ = writer
            .sync()
            .expect("sync establishes header + synced prefix");
        let r0 = writer.begin(PayloadA { v: 41 }).expect("begin unsynced 0");
        let id0 = r0.event_id();
        let r1 = writer
            .append(r0.fiber(), PayloadA { v: 42 })
            .expect("append unsynced 1");
        let id1 = r1.event_id();
        let dr = writer
            .detach(r1.fiber(), PayloadA { v: 43 })
            .expect("detach unsynced 2");
        let id2 = dr.event_id();
        let acked = writer.acked_lsn();
        assert!(
            acked.is_some(),
            "acked_lsn must reflect the initial sync; got None"
        );
        let _ = live;
        (synced_id, vec![id0, id1, id2])
    };
    let reopened: EventStore<PayloadA> =
        EventStore::<PayloadA>::open(&journal).expect("reopen after drop without 2nd sync");
    let reader = reopened.reader();
    let mut cur = reader.cursor(&sidecar).expect("cursor");
    let recovered: Vec<_> = cur
        .tail()
        .map(|r| r.expect("tail item").event_id())
        .collect();
    assert_eq!(
        recovered,
        vec![synced_id],
        "only the synced-prefix event must survive drop-without-sync; \
         unsynced receipts {minted_unsynced:?} must not be recovered"
    );
    for id in &minted_unsynced {
        assert!(
            !recovered.contains(id),
            "unsynced receipt {id:?} must not appear in recovered tail"
        );
    }
}
#[test]
fn only_synced_prefix_is_recovered_after_reopen() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    let (synced_ids, unsynced_id) = {
        let mut store: EventStore<PayloadA> =
            EventStore::<PayloadA>::create(&journal).expect("create");
        let mut writer = store.writer();
        let r0 = writer.begin(PayloadA { v: 1 }).expect("begin");
        let id0 = r0.event_id();
        let live = r0.fiber();
        let r1 = writer.append(live, PayloadA { v: 2 }).expect("append");
        let id1 = r1.event_id();
        let _ = writer.sync().expect("sync after two events");
        let r2 = writer.begin(PayloadA { v: 3 }).expect("begin unsynced");
        (vec![id0, id1], r2.event_id())
    };
    let reopened: EventStore<PayloadA> = EventStore::<PayloadA>::open(&journal).expect("reopen");
    let reader = reopened.reader();
    let mut cur = reader.cursor(&sidecar).expect("cursor");
    let recovered: Vec<_> = cur
        .tail()
        .map(|r| r.expect("tail item").event_id())
        .collect();
    assert_eq!(
        recovered, synced_ids,
        "reopen must surface exactly the synced prefix; \
         post-sync unsynced event id {unsynced_id:?} must not be present"
    );
    assert!(
        !recovered.contains(&unsynced_id),
        "unsynced receipt id {unsynced_id:?} must not survive drop-without-sync"
    );
}
#[test]
fn second_sync_persists_events_minted_after_first_sync() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    let all_ids = {
        let mut store: EventStore<PayloadA> =
            EventStore::<PayloadA>::create(&journal).expect("create");
        let mut writer = store.writer();
        let r0 = writer.begin(PayloadA { v: 1 }).expect("begin");
        let id0 = r0.event_id();
        let _ = writer.sync().expect("sync 1");
        let r1 = writer.begin(PayloadA { v: 2 }).expect("begin 2");
        let id1 = r1.event_id();
        let _ = writer.sync().expect("sync 2");
        vec![id0, id1]
    };
    let reopened: EventStore<PayloadA> = EventStore::<PayloadA>::open(&journal).expect("reopen");
    let reader = reopened.reader();
    let mut cur = reader.cursor(&sidecar).expect("cursor");
    let recovered: Vec<_> = cur
        .tail()
        .map(|r| r.expect("tail item").event_id())
        .collect();
    assert_eq!(
        recovered, all_ids,
        "after two syncs both event-id values must be recovered in commit order"
    );
}
#[test]
fn fiber_verbs_begin_append_resume_compose_full_lifecycle() {
    let (mut store, _td) = new_store_tmp::<PayloadA>();
    let mut writer = store.writer();
    let r_begin = writer.begin(PayloadA { v: 1 }).expect("begin");
    let live = r_begin.fiber();
    let begin_fid = live.fiber_id();
    let r_append = writer.append(live, PayloadA { v: 2 }).expect("append");
    let live = r_append.fiber();
    assert_eq!(live.fiber_id(), begin_fid, "append preserves fiber id");
    let r_detach = writer.detach(live, PayloadA { v: 3 }).expect("detach");
    let detached = r_detach.fiber();
    assert_eq!(detached.fiber_id(), begin_fid, "detach preserves fiber id");
    let r_resume = writer.resume(detached, PayloadA { v: 4 }).expect("resume");
    let live_again = r_resume.fiber();
    assert_eq!(
        live_again.fiber_id(),
        begin_fid,
        "resume preserves fiber id"
    );
}
