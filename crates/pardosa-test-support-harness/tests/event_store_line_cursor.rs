//! Sidecar-backed [`pardosa::store::LineCursor`]
//! (ADR-0018 §D3 (c)/§D5, ADR-0011 §D2/§D5/§D8).
//!
//! Pins against [`StoreReader::cursor`]:
//!
//! 1. No sidecar: `acked_offset() == None`; `tail()` yields all
//!    events (Amendment 2 — sidecar-only signature).
//! 2. `commit_consumed` exclusive on resume.
//! 3. ≤1 fsync per commit; stale commits no-op.
//! 4. Reopen with same sidecar resumes after committed event.
//! 5. Missing sidecar = cold start.
//! 6. Stale offset past every id yields nothing without error.
//! 7. Line-global, not per-fiber.
//! 8. Two sidecars do not interfere.
use pardosa::store::{EventId, EventStore, GenomeSafe, HasEventSchemaSource};
use tempfile::TempDir;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Payload {
    v: u64,
}
impl HasEventSchemaSource for Payload {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
fn write_n_events(path: &std::path::Path, n: u64) -> Vec<EventId> {
    let mut store: EventStore<Payload> = EventStore::<Payload>::create(path).expect("create");
    let mut writer = store.writer();
    let mut ids = Vec::with_capacity(usize::try_from(n).expect("event count fits usize"));
    for i in 0..n {
        let r = writer.begin(Payload { v: i }).expect("begin");
        ids.push(r.event_id());
    }
    let _ = writer.sync().expect("sync");
    ids
}
fn collect_events(
    journal: &std::path::Path,
    sidecar: &std::path::Path,
) -> Vec<pardosa::store::Event<Payload>> {
    let store: EventStore<Payload> = EventStore::<Payload>::open(journal).expect("open");
    let reader = store.reader();
    let mut cur = reader.cursor(sidecar).expect("cursor");
    cur.tail().map(|r| r.expect("tail item")).collect()
}
#[test]
fn fresh_cursor_with_no_sidecar_yields_every_event_in_commit_order() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    let ids = write_n_events(&journal, 4);
    let store: EventStore<Payload> = EventStore::<Payload>::open(&journal).expect("open");
    let reader = store.reader();
    let mut cur = reader.cursor(&sidecar).expect("cursor");
    assert_eq!(
        cur.acked_offset(),
        None,
        "fresh cursor with no sidecar must have no acked offset"
    );
    let got: Vec<EventId> = cur
        .tail()
        .map(|r| r.expect("tail item").event_id())
        .collect();
    assert_eq!(
        got, ids,
        "fresh cursor must yield every persisted event in commit order"
    );
}
#[test]
fn commit_consumed_is_exclusive_on_resume_within_one_handle() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    let ids = write_n_events(&journal, 4);
    let events: Vec<_> = {
        let scratch = td.path().join("scratch.ack");
        collect_events(&journal, &scratch)
    };
    let store: EventStore<Payload> = EventStore::<Payload>::open(&journal).expect("open");
    let reader = store.reader();
    let mut cur = reader.cursor(&sidecar).expect("cursor");
    cur.commit_consumed(&events[1]).expect("commit");
    assert_eq!(cur.acked_offset(), Some(ids[1]));
    let got: Vec<EventId> = cur
        .tail()
        .map(|r| r.expect("tail item").event_id())
        .collect();
    assert_eq!(
        got,
        ids[2..].to_vec(),
        "tail after commit_consumed(N) must yield only events with id > N"
    );
}
#[test]
fn reopen_with_same_sidecar_resumes_exclusively_after_committed_event() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    let ids = write_n_events(&journal, 5);
    let events = {
        let scratch = td.path().join("scratch.ack");
        collect_events(&journal, &scratch)
    };
    {
        let store: EventStore<Payload> = EventStore::<Payload>::open(&journal).expect("open 1");
        let reader = store.reader();
        let mut cur = reader.cursor(&sidecar).expect("cursor 1");
        cur.commit_consumed(&events[2]).expect("commit");
    }
    let store: EventStore<Payload> = EventStore::<Payload>::open(&journal).expect("open 2");
    let reader = store.reader();
    let mut cur = reader.cursor(&sidecar).expect("cursor 2");
    assert_eq!(
        cur.acked_offset(),
        Some(ids[2]),
        "reopen must rehydrate the prior committed offset from the sidecar"
    );
    let got: Vec<EventId> = cur
        .tail()
        .map(|r| r.expect("tail item").event_id())
        .collect();
    assert_eq!(
        got,
        ids[3..].to_vec(),
        "reopen with same sidecar must resume exclusively after the committed event"
    );
}
#[test]
fn stale_commit_consumed_is_a_noop_and_does_not_regress_acked() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    let ids = write_n_events(&journal, 4);
    let events = {
        let scratch = td.path().join("scratch.ack");
        collect_events(&journal, &scratch)
    };
    let store: EventStore<Payload> = EventStore::<Payload>::open(&journal).expect("open");
    let reader = store.reader();
    let mut cur = reader.cursor(&sidecar).expect("cursor");
    cur.commit_consumed(&events[2]).expect("commit forward");
    assert_eq!(cur.acked_offset(), Some(ids[2]));
    cur.commit_consumed(&events[1])
        .expect("stale commit must be ok");
    assert_eq!(
        cur.acked_offset(),
        Some(ids[2]),
        "stale commit must not regress acked_offset"
    );
    cur.commit_consumed(&events[2])
        .expect("equal commit must be ok");
    assert_eq!(
        cur.acked_offset(),
        Some(ids[2]),
        "equal commit must not regress or advance acked_offset"
    );
}
#[test]
fn stale_commit_does_not_touch_sidecar_byte_length() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    let _ids = write_n_events(&journal, 4);
    let events = {
        let scratch = td.path().join("scratch.ack");
        collect_events(&journal, &scratch)
    };
    let store: EventStore<Payload> = EventStore::<Payload>::open(&journal).expect("open");
    let reader = store.reader();
    let mut cur = reader.cursor(&sidecar).expect("cursor");
    cur.commit_consumed(&events[2]).expect("commit");
    let meta_before = std::fs::metadata(&sidecar).expect("sidecar exists after commit");
    let mtime_before = meta_before
        .modified()
        .expect("sidecar modified time available");
    std::thread::sleep(std::time::Duration::from_millis(20));
    cur.commit_consumed(&events[0]).expect("stale commit");
    let meta_after = std::fs::metadata(&sidecar).expect("sidecar still exists");
    let mtime_after = meta_after.modified().expect("sidecar mtime");
    assert_eq!(
        meta_before.len(),
        meta_after.len(),
        "stale commit must not rewrite sidecar contents"
    );
    assert_eq!(
        mtime_before, mtime_after,
        "stale commit must not bump sidecar mtime (no fsync) — before={mtime_before:?} after={mtime_after:?}"
    );
}
#[test]
fn stale_sidecar_beyond_every_event_yields_nothing() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    let _ids = write_n_events(&journal, 3);
    let events = {
        let scratch = td.path().join("scratch.ack");
        collect_events(&journal, &scratch)
    };
    let last = events.last().expect("at least one event");
    {
        let store: EventStore<Payload> = EventStore::<Payload>::open(&journal).expect("open 1");
        let reader = store.reader();
        let mut cur = reader.cursor(&sidecar).expect("cursor 1");
        cur.commit_consumed(last).expect("commit terminal event");
    }
    let store: EventStore<Payload> = EventStore::<Payload>::open(&journal).expect("open 2");
    let reader = store.reader();
    let mut cur = reader.cursor(&sidecar).expect("cursor 2");
    let got: Vec<_> = cur.tail().collect::<Result<Vec<_>, _>>().expect("tail ok");
    assert!(
        got.is_empty(),
        "sidecar at or beyond every event-id must yield nothing on reopen; got {} items",
        got.len()
    );
}
#[test]
fn line_cursor_is_global_and_interleaves_fibers_in_commit_order() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    let (expected_ids, expected_payloads): (Vec<EventId>, Vec<u64>) = {
        let mut store: EventStore<Payload> =
            EventStore::<Payload>::create(&journal).expect("create");
        let mut writer = store.writer();
        let ra0 = writer.begin(Payload { v: 10 }).expect("a0");
        let begin_a = ra0.event_id();
        let live_a = ra0.fiber();
        let rb0 = writer.begin(Payload { v: 20 }).expect("b0");
        let begin_b = rb0.event_id();
        let live_b = rb0.fiber();
        let ra1 = writer.append(live_a, Payload { v: 11 }).expect("a1");
        let append_a = ra1.event_id();
        let rb1 = writer.append(live_b, Payload { v: 21 }).expect("b1");
        let append_b = rb1.event_id();
        let _ = writer.sync().expect("sync");
        (
            vec![begin_a, begin_b, append_a, append_b],
            vec![10, 20, 11, 21],
        )
    };
    let store: EventStore<Payload> = EventStore::<Payload>::open(&journal).expect("open");
    let reader = store.reader();
    let mut cur = reader.cursor(&sidecar).expect("cursor");
    let got: Vec<(EventId, u64)> = cur
        .tail()
        .map(|r| {
            let e = r.expect("tail item");
            (e.event_id(), e.domain_event().v)
        })
        .collect();
    let got_ids: Vec<EventId> = got.iter().map(|(id, _)| *id).collect();
    let got_payloads: Vec<u64> = got.iter().map(|(_, v)| *v).collect();
    assert_eq!(
        got_ids, expected_ids,
        "LineCursor::tail must yield events from every fiber in commit order"
    );
    assert_eq!(got_payloads, expected_payloads);
}
#[test]
fn commit_consumed_advances_acked_to_event_id() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    let ids = write_n_events(&journal, 4);
    let store: EventStore<Payload> = EventStore::<Payload>::open(&journal).expect("open");
    let reader = store.reader();
    let mut cur = reader.cursor(&sidecar).expect("cursor");
    let yielded: Vec<_> = cur.tail().map(|r| r.expect("tail item")).collect();
    assert_eq!(yielded.len(), 4);
    let third = &yielded[2];
    cur.commit_consumed(third)
        .expect("commit_consumed must succeed");
    assert_eq!(
        cur.acked_offset(),
        Some(ids[2]),
        "commit_consumed(&event) must advance acked_offset to event.event_id()"
    );
}
#[test]
fn commit_consumed_then_reopen_resumes_exclusively_after_committed_id() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    let ids = write_n_events(&journal, 5);
    {
        let store: EventStore<Payload> = EventStore::<Payload>::open(&journal).expect("open 1");
        let reader = store.reader();
        let mut cur = reader.cursor(&sidecar).expect("cursor 1");
        let evs: Vec<_> = cur.tail().map(|r| r.expect("tail item")).collect();
        cur.commit_consumed(&evs[2])
            .expect("commit_consumed second event");
    }
    let store: EventStore<Payload> = EventStore::<Payload>::open(&journal).expect("open 2");
    let reader = store.reader();
    let mut cur = reader.cursor(&sidecar).expect("cursor 2");
    assert_eq!(
        cur.acked_offset(),
        Some(ids[2]),
        "reopen must rehydrate the commit_consumed watermark from the sidecar"
    );
    let got: Vec<EventId> = cur
        .tail()
        .map(|r| r.expect("tail item").event_id())
        .collect();
    assert_eq!(
        got,
        ids[3..].to_vec(),
        "reopen after commit_consumed must resume exclusively after the committed id"
    );
}
#[test]
fn commit_consumed_preserves_stale_noop_semantics() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    let ids = write_n_events(&journal, 4);
    let store: EventStore<Payload> = EventStore::<Payload>::open(&journal).expect("open");
    let reader = store.reader();
    let mut cur = reader.cursor(&sidecar).expect("cursor");
    let evs: Vec<_> = cur.tail().map(|r| r.expect("tail item")).collect();
    cur.commit_consumed(&evs[2]).expect("commit forward");
    assert_eq!(cur.acked_offset(), Some(ids[2]));
    cur.commit_consumed(&evs[1])
        .expect("stale commit_consumed must be ok");
    assert_eq!(
        cur.acked_offset(),
        Some(ids[2]),
        "stale commit_consumed must not regress acked_offset \
         (ADR-0011 §D2/§D8 preserved through the helper)"
    );
}
#[test]
fn two_cursors_against_same_journal_with_distinct_sidecars_are_independent() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar_a = td.path().join("a.ack");
    let sidecar_b = td.path().join("b.ack");
    let ids = write_n_events(&journal, 4);
    let events_for_ack = {
        let scratch = td.path().join("scratch.ack");
        collect_events(&journal, &scratch)
    };
    let store: EventStore<Payload> = EventStore::<Payload>::open(&journal).expect("open");
    let reader = store.reader();
    let mut cur_a = reader.cursor(&sidecar_a).expect("cursor a");
    cur_a.commit_consumed(&events_for_ack[2]).expect("a commit");
    let mut cur_b = reader.cursor(&sidecar_b).expect("cursor b");
    assert_eq!(
        cur_b.acked_offset(),
        None,
        "cursor B with a distinct sidecar must not observe cursor A's offset"
    );
    let got_b: Vec<EventId> = cur_b
        .tail()
        .map(|r| r.expect("tail b").event_id())
        .collect();
    assert_eq!(
        got_b, ids,
        "cursor B with a fresh sidecar must yield every event regardless of cursor A's commits"
    );
    let got_a: Vec<EventId> = cur_a
        .tail()
        .map(|r| r.expect("tail a").event_id())
        .collect();
    assert_eq!(
        got_a,
        ids[3..].to_vec(),
        "cursor A must still resume exclusively after its own committed offset"
    );
}
#[test]
fn commit_consumed_id_matches_commit_consumed_semantics() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    let ids = write_n_events(&journal, 4);
    let store: EventStore<Payload> = EventStore::<Payload>::open(&journal).expect("open");
    let reader = store.reader();
    let mut cur = reader.cursor(&sidecar).expect("cursor");
    cur.commit_consumed_id(ids[1]).expect("commit by id");
    assert_eq!(
        cur.acked_offset(),
        Some(ids[1]),
        "commit_consumed_id must advance the acked watermark identically to commit_consumed"
    );
    let got: Vec<EventId> = cur
        .tail()
        .map(|r| r.expect("tail item").event_id())
        .collect();
    assert_eq!(
        got,
        ids[2..].to_vec(),
        "tail after commit_consumed_id(N) must yield only events with id > N"
    );
    cur.commit_consumed_id(ids[0]).expect("stale commit by id");
    assert_eq!(
        cur.acked_offset(),
        Some(ids[1]),
        "stale commit_consumed_id must be a no-op (monotonic per ADR-0011 D2)"
    );
}
