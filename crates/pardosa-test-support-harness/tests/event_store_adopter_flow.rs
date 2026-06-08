//! End-to-end adopter-flow harness for path-backed
//! `EventStore<T>` (ADR-0018 § Naming/§D1-§D7, ADR-0011 §D2/§D5).
//!
//! Pinning in adopter order against `W = File`:
//!
//! 1. Path-backed `create`/`open` are the only public constructors.
//! 2. Writer payload-only; Pardosa mints identity.
//! 3. `sync()` returns `Lsn`; `acked_lsn` advances.
//! 4. `open` rehydrates; three read views observe every event.
//! 5. The three read views are distinct.
//! 6. `LineCursor` resume is sidecar-durable and exclusive.
//!
//! Per-dimension pinnings live in sibling `event_store_*` files.
use pardosa::store::{EventId, EventStore, FiberId, GenomeSafe, HasEventSchemaSource, Lsn};
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
    fid_a: FiberId,
    fid_b: FiberId,
    head_a: EventId,
    head_b: EventId,
    acked_after_sync: Option<Lsn>,
}
fn create_write_two_fibers_then_sync(journal: &Path) -> WrittenIdentity {
    let mut store: EventStore<Order> =
        EventStore::<Order>::create(journal).expect("path-backed create must succeed");
    let mut writer = store.writer();
    assert_eq!(
        writer.acked_lsn(),
        None,
        "fresh writer has no acked Lsn before sync"
    );
    let begin_a = writer
        .begin(Order {
            id: 1,
            amount_cents: 100,
        })
        .expect("start fiber A");
    let live_a = begin_a.fiber();
    let fid_a = live_a.fiber_id();
    let begin_b = writer
        .begin(Order {
            id: 2,
            amount_cents: 200,
        })
        .expect("start fiber B");
    let live_b = begin_b.fiber();
    let fid_b = live_b.fiber_id();
    assert_ne!(
        fid_a, fid_b,
        "Pardosa-minted FiberId must be unique per begin"
    );
    let live_a = writer
        .append(
            live_a,
            Order {
                id: 1,
                amount_cents: 150,
            },
        )
        .expect("append A1")
        .fiber();
    let r_a2 = writer
        .append(
            live_a,
            Order {
                id: 1,
                amount_cents: 175,
            },
        )
        .expect("append A2");
    let head_a = r_a2.event_id();
    let appended_b1 = writer
        .append(
            live_b,
            Order {
                id: 2,
                amount_cents: 225,
            },
        )
        .expect("append B1");
    let head_b = appended_b1.event_id();
    let lsn = writer.sync().expect("sync returns Lsn, not Result<_, ()>");
    let acked_after_sync = writer.acked_lsn();
    assert_eq!(
        acked_after_sync,
        Some(lsn),
        "acked_lsn must equal the Lsn returned by the most recent sync"
    );
    WrittenIdentity {
        fid_a,
        fid_b,
        head_a,
        head_b,
        acked_after_sync,
    }
}
fn assert_reopen_reads_match_per_fiber_and_causal(journal: &Path, ids: &WrittenIdentity) {
    let store: EventStore<Order> = EventStore::<Order>::open(journal)
        .expect("path-backed open after sync must succeed (no auto-migration in play)");
    let reader = store.reader();
    let history_a: Vec<u64> = reader
        .fiber(ids.fid_a)
        .iter()
        .expect("fiber A history present after rehydrate")
        .map(|ev| ev.domain_event().amount_cents)
        .collect();
    assert_eq!(
        history_a,
        vec![100, 150, 175],
        "fiber A history is per-fiber, in commit order, never interleaves fiber B"
    );
    let history_b: Vec<u64> = reader
        .fiber(ids.fid_b)
        .iter()
        .expect("fiber B history present after rehydrate")
        .map(|ev| ev.domain_event().amount_cents)
        .collect();
    assert_eq!(
        history_b,
        vec![200, 225],
        "fiber B history is per-fiber, in commit order, never interleaves fiber A"
    );
    let causal_walk_a: Vec<u64> = reader
        .causal_chain(ids.head_a)
        .iter()
        .map(|ev| ev.domain_event().amount_cents)
        .collect();
    assert_eq!(
        causal_walk_a,
        vec![175, 150, 100],
        "causal_chain walks head-first along fiber A, terminating at genesis"
    );
    let causal_walk_b: Vec<u64> = reader
        .causal_chain(ids.head_b)
        .iter()
        .map(|ev| ev.domain_event().amount_cents)
        .collect();
    assert_eq!(
        causal_walk_b,
        vec![225, 200],
        "causal_chain walks head-first along fiber B, terminating at genesis"
    );
}
fn open_cursor_and_commit_third_event(journal: &Path, sidecar: &Path) -> Vec<EventId> {
    let store: EventStore<Order> =
        EventStore::<Order>::open(journal).expect("path-backed open for cursor phase");
    let reader = store.reader();
    let mut cursor = reader
        .cursor(sidecar)
        .expect("LineCursor open via path-backed StoreReader");
    assert_eq!(
        cursor.acked_offset(),
        None,
        "fresh sidecar means no prior acked offset (cold-start consumer)"
    );
    let events: Vec<_> = cursor.tail().map(|r| r.expect("tail item")).collect();
    assert_eq!(
        events.len(),
        5,
        "LineCursor is global: 3 events from A + 2 events from B = 5 on the line"
    );
    let line_ids: Vec<EventId> = events
        .iter()
        .map(pardosa::prelude::Event::event_id)
        .collect();
    let ack_target = events[2].event_id();
    cursor
        .commit_consumed(&events[2])
        .expect("commit_consumed advances sidecar with one fsync");
    assert_eq!(
        cursor.acked_offset(),
        Some(ack_target),
        "commit_consumed(&event) leaves acked_offset() == Some(event.event_id())"
    );
    line_ids
}
fn assert_reopen_cursor_resumes_after_committed(
    journal: &Path,
    sidecar: &Path,
    line_ids_in_commit_order: &[EventId],
) {
    let ack_target = line_ids_in_commit_order[2];
    let store: EventStore<Order> =
        EventStore::<Order>::open(journal).expect("second path-backed open of same journal");
    let reader = store.reader();
    let mut cursor = reader
        .cursor(sidecar)
        .expect("reopen LineCursor against same sidecar path");
    assert_eq!(
        cursor.acked_offset(),
        Some(ack_target),
        "reopen rehydrates acked_offset from the sidecar"
    );
    let resumed_ids: Vec<EventId> = cursor
        .tail()
        .map(|r| r.expect("tail item after resume").event_id())
        .collect();
    assert_eq!(
        resumed_ids,
        line_ids_in_commit_order[3..].to_vec(),
        "reopen + tail resumes exclusively after the committed EventId"
    );
}
/// Adopter-flow baseline: path-backed `EventStore<T>::create` →
/// payload-only writer (multi-fiber, append, sync) → `open` →
/// `StoreReader` with all three views (`fiber`, `causal_chain`,
/// `cursor`) → `LineCursor` sidecar `commit_consumed` → reopen
/// resumes exclusively after the committed offset.
///
/// Each assertion sentence is the actual ADR-0018 contract the
/// flow demonstrates. A future visibility/renaming reshape that
/// breaks this single test breaks the adopter narrative; if it
/// passes, the public surface that adopter docs reference is
/// preserved.
#[test]
fn path_backed_adopter_flow_create_write_sync_open_read_cursor() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    let ids = create_write_two_fibers_then_sync(&journal);
    assert_reopen_reads_match_per_fiber_and_causal(&journal, &ids);
    let line_ids_in_commit_order = open_cursor_and_commit_third_event(&journal, &sidecar);
    assert_reopen_cursor_resumes_after_committed(&journal, &sidecar, &line_ids_in_commit_order);
    assert!(
        ids.acked_after_sync.is_some(),
        "writer-side acked_lsn observed before sink was dropped"
    );
}
