//! `StoreReader::{fiber, causal_chain}` runtime contract
//! (ADR-0018 §D3/§D5/§D6/§D8, ADR-0003 §1/§3).
//!
//! Pins:
//!
//! 1. `FiberHistory` per-fiber, in-memory, no I/O; `FiberId` is
//!    dragline-local.
//! 2. `FiberHistory::state` mirrors `Defined`/`Detached`/
//!    `Undefined`; `FiberState` re-exported.
//! 3. `iter` on unknown id → `FiberNotFound`.
//! 4. `CausalChain` walks head-first within one fiber;
//!    cross-fiber precursors terminate cleanly.
//! 5. Chain ends on `Precursor::Genesis`.
//! 6. Single-event fiber yields exactly that event.
//! 7. Both views read-only.
//! 8. Multiple handles coexist over one borrow.
//! 9. `FiberId` from one dragline is `Undefined` on another.
//!
//! Compile-fail twins under `tests/ui`.
use pardosa::store::{
    Event, EventId, EventStore, FiberState, GenomeSafe, HasEventSchemaSource, PardosaError,
    Precursor,
};
use tempfile::TempDir;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Payload {
    v: u64,
}
impl HasEventSchemaSource for Payload {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
fn new_store() -> (EventStore<Payload>, TempDir) {
    let td = TempDir::new().expect("tempdir");
    let store = EventStore::<Payload>::create(&td.path().join("journal.pgno"))
        .expect("create path-backed EventStore");
    (store, td)
}
#[test]
fn fiber_history_is_per_fiber_filtered_in_commit_order() {
    let (mut store, _td) = new_store();
    let mut writer = store.writer();
    let begin_a = writer.begin(Payload { v: 10 }).expect("start a");
    let live_a = begin_a.fiber();
    let fid_a = live_a.fiber_id();
    let begin_b = writer.begin(Payload { v: 20 }).expect("start b");
    let live_b = begin_b.fiber();
    let fid_b = live_b.fiber_id();
    let live_a = writer
        .append(live_a, Payload { v: 11 })
        .expect("append a1")
        .fiber();
    let _ = writer.append(live_b, Payload { v: 21 }).expect("append b1");
    let _ = writer.append(live_a, Payload { v: 12 }).expect("append a2");
    let reader = writer.reader();
    let hist_a: Vec<u64> = reader
        .fiber(fid_a)
        .iter()
        .expect("history a iter")
        .map(|e| e.domain_event().v)
        .collect();
    assert_eq!(
        hist_a,
        vec![10, 11, 12],
        "FiberHistory(a) must yield only fiber a's events in commit order"
    );
    let hist_b: Vec<u64> = reader
        .fiber(fid_b)
        .iter()
        .expect("history b iter")
        .map(|e| e.domain_event().v)
        .collect();
    assert_eq!(
        hist_b,
        vec![20, 21],
        "FiberHistory(b) must yield only fiber b's events in commit order"
    );
}
#[test]
fn precursor_genesis_and_of_visible_through_fiber_iter() {
    let (mut store, _td) = new_store();
    let mut writer = store.writer();
    let r0 = writer.begin(Payload { v: 10 }).expect("begin");
    let id0 = r0.event_id();
    let live = r0.fiber();
    let fid = live.fiber_id();
    let _r1 = writer.append(live, Payload { v: 20 }).expect("append");
    let reader = writer.reader();
    let events: Vec<&Event<Payload>> = reader.fiber(fid).iter().expect("fiber iter").collect();
    assert_eq!(events.len(), 2, "fiber must carry exactly two events");
    assert!(
        matches!(events[0].precursor(), Precursor::Genesis),
        "first event on a fresh fiber must be Precursor::Genesis; got {:?}",
        events[0].precursor()
    );
    match events[1].precursor() {
        Precursor::Of(idx) => {
            assert_eq!(
                idx.value(),
                id0.value(),
                "second event's Precursor::Of must point at the first event's id"
            );
        }
        other => panic!("expected Precursor::Of, got {other:?}"),
    }
}
#[test]
fn fiber_history_state_reflects_defined_and_detached() {
    let (mut store, _td) = new_store();
    let mut writer = store.writer();
    let r0 = writer.begin(Payload { v: 1 }).expect("begin");
    let live = r0.fiber();
    let fid = live.fiber_id();
    assert_eq!(
        writer.reader().fiber(fid).state(),
        FiberState::Defined,
        "freshly created fiber must be Defined"
    );
    let dr = writer.detach(live, Payload { v: 2 }).expect("detach");
    let detached_fid = dr.fiber().fiber_id();
    assert_eq!(detached_fid, fid, "detach preserves FiberId");
    assert_eq!(
        writer.reader().fiber(fid).state(),
        FiberState::Detached,
        "post-detach fiber must be Detached"
    );
}
#[test]
fn fiber_id_from_other_dragline_is_undefined_locally() {
    let (mut other, _td_other) = new_store();
    let foreign_fid = other
        .writer()
        .begin(Payload { v: 0 })
        .expect("start other")
        .fiber()
        .fiber_id();
    let (store, _td) = new_store();
    let reader = store.reader();
    assert_eq!(
        reader.fiber(foreign_fid).state(),
        FiberState::Undefined,
        "FiberId minted on another dragline must be Undefined locally (ADR-0003 §1)"
    );
}
#[test]
fn fiber_history_iter_on_unknown_fiber_errors_fiber_not_found() {
    let (mut other, _td_other) = new_store();
    let foreign_fid = other
        .writer()
        .begin(Payload { v: 0 })
        .expect("start other")
        .fiber()
        .fiber_id();
    let (store, _td) = new_store();
    let reader = store.reader();
    let err = reader
        .fiber(foreign_fid)
        .iter()
        .err()
        .expect("iter on unknown fiber must error, not be silently empty");
    let msg = format!("{err}");
    assert!(
        msg.to_lowercase().contains("fiber"),
        "expected FiberNotFound-shaped error; got: {msg}"
    );
}
#[test]
fn causal_chain_walks_head_first_same_fiber_to_genesis() {
    let (mut store, _td) = new_store();
    let mut writer = store.writer();
    let r0 = writer.begin(Payload { v: 100 }).expect("start");
    let id0 = r0.event_id();
    let live = r0.fiber();
    let r1 = writer.append(live, Payload { v: 101 }).expect("append 1");
    let id1 = r1.event_id();
    let live = r1.fiber();
    let r2 = writer.append(live, Payload { v: 102 }).expect("append 2");
    let id2 = r2.event_id();
    let reader = writer.reader();
    let chain: Vec<(u64, EventId)> = reader
        .causal_chain(id2)
        .iter()
        .map(|e| (e.domain_event().v, e.event_id()))
        .collect();
    let ids: Vec<EventId> = chain.iter().map(|(_, id)| *id).collect();
    let payloads: Vec<u64> = chain.iter().map(|(v, _)| *v).collect();
    assert_eq!(
        ids,
        vec![id2, id1, id0],
        "CausalChain must walk head-first, oldest last"
    );
    assert_eq!(payloads, vec![102, 101, 100]);
}
#[test]
fn causal_chain_rooted_on_fiber_a_does_not_leak_fiber_b_events() {
    let (mut store, _td) = new_store();
    let mut writer = store.writer();
    let r_a0 = writer.begin(Payload { v: 1 }).expect("start a");
    let live_a = r_a0.fiber();
    let r_a1 = writer.append(live_a, Payload { v: 2 }).expect("append a");
    let head_a = r_a1.event_id();
    let _r_b0 = writer.begin(Payload { v: 99 }).expect("start b");
    let reader = writer.reader();
    let chain: Vec<u64> = reader
        .causal_chain(head_a)
        .iter()
        .map(|e| e.domain_event().v)
        .collect();
    assert_eq!(
        chain,
        vec![2, 1],
        "CausalChain must stay on the head's fiber and terminate on Genesis without crossing into fiber b"
    );
}
#[test]
fn causal_chain_on_single_event_fiber_yields_only_that_event() {
    let (mut store, _td) = new_store();
    let mut writer = store.writer();
    let r0 = writer.begin(Payload { v: 42 }).expect("start");
    let id0 = r0.event_id();
    let reader = writer.reader();
    let chain: Vec<&Event<Payload>> = reader.causal_chain(id0).iter().collect();
    assert_eq!(
        chain.len(),
        1,
        "single-event fiber: chain is exactly [head] then terminates"
    );
    assert_eq!(chain[0].domain_event().v, 42);
    assert_eq!(chain[0].event_id(), id0);
}
#[test]
fn multiple_history_and_chain_handles_coexist_over_shared_reader() {
    let (mut store, _td) = new_store();
    let mut writer = store.writer();
    let begin_a = writer.begin(Payload { v: 1 }).expect("start a");
    let live_a = begin_a.fiber();
    let fid_a = live_a.fiber_id();
    let _ = writer.append(live_a, Payload { v: 2 }).expect("append a1");
    let begin_b = writer.begin(Payload { v: 10 }).expect("start b");
    let head_b = begin_b.event_id();
    let fid_b = begin_b.fiber().fiber_id();
    let reader = writer.reader();
    let h_a = reader.fiber(fid_a);
    let h_b = reader.fiber(fid_b);
    let c_b = reader.causal_chain(head_b);
    let a_payloads: Vec<u64> = h_a.iter().expect("a").map(|e| e.domain_event().v).collect();
    let b_payloads: Vec<u64> = h_b.iter().expect("b").map(|e| e.domain_event().v).collect();
    let c_payloads: Vec<u64> = c_b.iter().map(|e| e.domain_event().v).collect();
    assert_eq!(a_payloads, vec![1, 2]);
    assert_eq!(b_payloads, vec![10]);
    assert_eq!(c_payloads, vec![10]);
}
#[test]
fn fiber_history_size_hint_matches_commit_order_length() {
    let (mut store, _td) = new_store();
    let mut writer = store.writer();
    let r0 = writer.begin(Payload { v: 1 }).expect("start");
    let live = r0.fiber();
    let fid = live.fiber_id();
    let live = writer
        .append(live, Payload { v: 2 })
        .expect("append")
        .fiber();
    let _ = writer.append(live, Payload { v: 3 }).expect("append");
    let reader = writer.reader();
    let iter = reader.fiber(fid).iter().expect("iter");
    let (lo, hi) = iter.size_hint();
    assert_eq!(
        (lo, hi),
        (3, Some(3)),
        "size_hint must exactly match the per-fiber history length"
    );
}
fn build_fiber_with_ids(
    n: usize,
) -> (
    EventStore<Payload>,
    TempDir,
    pardosa::store::FiberId,
    Vec<EventId>,
) {
    assert!(n >= 1, "need at least one event");
    let (mut store, td) = new_store();
    let mut writer = store.writer();
    let r0 = writer.begin(Payload { v: 0 }).expect("start");
    let id0 = r0.event_id();
    let mut live = r0.fiber();
    let fid = live.fiber_id();
    let mut ids = vec![id0];
    for i in 1..n {
        let r = writer
            .append(live, Payload { v: i as u64 })
            .expect("append");
        ids.push(r.event_id());
        live = r.fiber();
    }
    (store, td, fid, ids)
}
#[test]
fn fiber_history_full_via_iter_unchanged_by_partial_read_additions() {
    let (store, _td, fid, ids) = build_fiber_with_ids(5);
    let reader = store.reader();
    let collected: Vec<EventId> = reader
        .fiber(fid)
        .iter()
        .expect("iter")
        .map(Event::event_id)
        .collect();
    assert_eq!(
        collected, ids,
        "iter() must still yield the full commit order"
    );
}
#[test]
fn fiber_history_range_yields_half_open_window() {
    let (store, _td, fid, ids) = build_fiber_with_ids(5);
    let reader = store.reader();
    let window: Vec<EventId> = reader
        .fiber(fid)
        .range(ids[1], ids[4])
        .expect("range")
        .map(Event::event_id)
        .collect();
    assert_eq!(
        window,
        vec![ids[1], ids[2], ids[3]],
        "range must be half-open [from, to)"
    );
}
#[test]
fn fiber_history_from_event_id_yields_suffix() {
    let (store, _td, fid, ids) = build_fiber_with_ids(5);
    let reader = store.reader();
    let suffix: Vec<EventId> = reader
        .fiber(fid)
        .from_event_id(ids[2])
        .expect("from_event_id")
        .map(Event::event_id)
        .collect();
    assert_eq!(
        suffix,
        vec![ids[2], ids[3], ids[4]],
        "from_event_id must yield [from, end] inclusive of `from`"
    );
}
#[test]
fn fiber_history_take_n_yields_prefix() {
    let (store, _td, fid, ids) = build_fiber_with_ids(5);
    let reader = store.reader();
    let prefix: Vec<EventId> = reader
        .fiber(fid)
        .take(3)
        .expect("take")
        .map(Event::event_id)
        .collect();
    assert_eq!(
        prefix,
        vec![ids[0], ids[1], ids[2]],
        "take(n) must yield the first n events in commit order"
    );
}
#[test]
fn fiber_history_take_n_saturates_when_n_exceeds_length() {
    let (store, _td, fid, ids) = build_fiber_with_ids(3);
    let reader = store.reader();
    let all: Vec<EventId> = reader
        .fiber(fid)
        .take(10)
        .expect("take saturating")
        .map(Event::event_id)
        .collect();
    assert_eq!(
        all, ids,
        "take(n) with n > len yields the whole history without error"
    );
}
#[test]
fn fiber_history_range_empty_when_from_equals_to() {
    let (store, _td, fid, ids) = build_fiber_with_ids(5);
    let reader = store.reader();
    let empty: Vec<EventId> = reader
        .fiber(fid)
        .range(ids[2], ids[2])
        .expect("range empty")
        .map(Event::event_id)
        .collect();
    assert!(
        empty.is_empty(),
        "range with from == to is empty (half-open)"
    );
}
#[test]
fn fiber_history_range_empty_when_from_greater_than_to() {
    let (store, _td, fid, ids) = build_fiber_with_ids(5);
    let reader = store.reader();
    let empty: Vec<EventId> = reader
        .fiber(fid)
        .range(ids[3], ids[1])
        .expect("range inverted")
        .map(Event::event_id)
        .collect();
    assert!(
        empty.is_empty(),
        "range with from > to is empty rather than error (half-open semantics)"
    );
}
#[test]
fn fiber_history_from_event_id_strictly_before_first_yields_whole_history() {
    let (mut store, _td) = new_store();
    let mut writer = store.writer();
    let _ = writer
        .begin(Payload { v: 0 })
        .expect("start sentinel fiber so target fiber's first id is > EventId::ZERO");
    let r0 = writer.begin(Payload { v: 1 }).expect("start target");
    let id0 = r0.event_id();
    let mut live = r0.fiber();
    let fid = live.fiber_id();
    let mut ids = vec![id0];
    for i in 1..3u64 {
        let r = writer.append(live, Payload { v: i + 1 }).expect("append");
        ids.push(r.event_id());
        live = r.fiber();
    }
    assert!(
        id0 > EventId::ZERO,
        "precondition: target fiber's first id must be strictly greater than EventId::ZERO; got {id0:?}"
    );
    let reader = writer.reader();
    let all: Vec<EventId> = reader
        .fiber(fid)
        .from_event_id(EventId::ZERO)
        .expect("from_event_id strictly before first")
        .map(Event::event_id)
        .collect();
    assert_eq!(
        all, ids,
        "from_event_id with `from` strictly less than the fiber's first event must yield the whole history"
    );
}
#[test]
fn fiber_history_take_zero_yields_empty() {
    let (store, _td, fid, _ids) = build_fiber_with_ids(3);
    let reader = store.reader();
    let empty: Vec<EventId> = reader
        .fiber(fid)
        .take(0)
        .expect("take(0)")
        .map(Event::event_id)
        .collect();
    assert!(
        empty.is_empty(),
        "take(0) must yield an empty iterator per rustdoc contract"
    );
}
#[test]
fn fiber_history_range_on_unknown_fiber_errors_fiber_not_found() {
    let (mut other, _td_other) = new_store();
    let foreign_fid = other
        .writer()
        .begin(Payload { v: 0 })
        .expect("start other")
        .fiber()
        .fiber_id();
    let (store, _td) = new_store();
    let reader = store.reader();
    let err = reader
        .fiber(foreign_fid)
        .range(EventId::ZERO, EventId::ZERO)
        .err()
        .expect("range on unknown fiber must error");
    assert!(
        matches!(err, PardosaError::FiberNotFound(_)),
        "expected PardosaError::FiberNotFound, got: {err:?}"
    );
}
#[test]
fn fiber_history_from_event_id_on_unknown_fiber_errors_fiber_not_found() {
    let (mut other, _td_other) = new_store();
    let foreign_fid = other
        .writer()
        .begin(Payload { v: 0 })
        .expect("start other")
        .fiber()
        .fiber_id();
    let (store, _td) = new_store();
    let reader = store.reader();
    let err = reader
        .fiber(foreign_fid)
        .from_event_id(EventId::ZERO)
        .err()
        .expect("from_event_id on unknown fiber must error");
    assert!(
        matches!(err, PardosaError::FiberNotFound(_)),
        "expected PardosaError::FiberNotFound, got: {err:?}"
    );
}
#[test]
fn fiber_history_take_on_unknown_fiber_errors_fiber_not_found() {
    let (mut other, _td_other) = new_store();
    let foreign_fid = other
        .writer()
        .begin(Payload { v: 0 })
        .expect("start other")
        .fiber()
        .fiber_id();
    let (store, _td) = new_store();
    let reader = store.reader();
    let err = reader
        .fiber(foreign_fid)
        .take(3)
        .err()
        .expect("take on unknown fiber must error");
    assert!(
        matches!(err, PardosaError::FiberNotFound(_)),
        "expected PardosaError::FiberNotFound, got: {err:?}"
    );
}
#[test]
fn fiber_history_spans_detach_and_resume_without_yielding_other_fiber_events() {
    let (mut store, _td) = new_store();
    let mut writer = store.writer();
    let begin_a = writer.begin(Payload { v: 1 }).expect("start a");
    let live_a = begin_a.fiber();
    let fid_a = live_a.fiber_id();
    let dr = writer.detach(live_a, Payload { v: 2 }).expect("detach a");
    let detached_a = dr.fiber();
    let begin_b = writer.begin(Payload { v: 99 }).expect("start b");
    let fid_b = begin_b.fiber().fiber_id();
    assert_ne!(fid_b, fid_a);
    let r_a2 = writer
        .resume(detached_a, Payload { v: 3 })
        .expect("resume a");
    let live_a = r_a2.fiber();
    let _ = writer
        .append(live_a, Payload { v: 4 })
        .expect("append a post-resume");
    let reader = writer.reader();
    let hist_a: Vec<u64> = reader
        .fiber(fid_a)
        .iter()
        .expect("hist a")
        .map(|e| e.domain_event().v)
        .collect();
    assert!(
        !hist_a.contains(&99),
        "FiberHistory(a) must never surface fiber b's event 99; got {hist_a:?}"
    );
    assert!(
        hist_a.contains(&4) && hist_a.contains(&1),
        "FiberHistory(a) must include both pre-detach and post-resume events on fiber a; got {hist_a:?}"
    );
}
#[test]
fn fiber_history_iter_rev_yields_newest_first_matching_iter_rev() {
    let (mut store, _td) = new_store();
    let mut writer = store.writer();
    let r0 = writer.begin(Payload { v: 1 }).expect("begin");
    let mut live = r0.fiber();
    let fid = live.fiber_id();
    for v in [2u64, 3, 4, 5] {
        live = writer.append(live, Payload { v }).expect("append").fiber();
    }
    let reader = writer.reader();
    let chronological: Vec<u64> = reader
        .fiber(fid)
        .iter()
        .expect("iter")
        .map(|e| e.domain_event().v)
        .collect();
    let stream_order: Vec<u64> = reader
        .fiber(fid)
        .iter_rev()
        .expect("iter_rev")
        .map(|e| e.domain_event().v)
        .collect();
    let expected_rev: Vec<u64> = chronological.iter().rev().copied().collect();
    assert_eq!(
        stream_order, expected_rev,
        "iter_rev must yield events newest-first, matching iter().rev() semantically; \
         chronological={chronological:?} stream={stream_order:?}"
    );
    assert_eq!(
        stream_order,
        vec![5, 4, 3, 2, 1],
        "iter_rev must yield 5 4 3 2 1 for the appended sequence"
    );
}
#[test]
fn fiber_history_iter_rev_on_unknown_fiber_errors_fiber_not_found() {
    let (mut other, _td_other) = new_store();
    let foreign_fid = other
        .writer()
        .begin(Payload { v: 0 })
        .expect("start other")
        .fiber()
        .fiber_id();
    let (store, _td) = new_store();
    let reader = store.reader();
    let err = reader
        .fiber(foreign_fid)
        .iter_rev()
        .err()
        .expect("iter_rev on unknown fiber must error, not be silently empty");
    assert!(
        matches!(err, PardosaError::FiberNotFound(_)),
        "expected PardosaError::FiberNotFound, got: {err:?}"
    );
}
#[test]
fn fiber_history_iter_rev_single_event_yields_once_then_terminates() {
    let (mut store, _td) = new_store();
    let mut writer = store.writer();
    let r = writer.begin(Payload { v: 42 }).expect("begin");
    let fid = r.fiber().fiber_id();
    let reader = writer.reader();
    let mut stream = reader.fiber(fid).iter_rev().expect("iter_rev");
    assert_eq!(stream.size_hint(), (1, Some(1)), "size_hint must be exact");
    let first = stream.next().expect("single-event fiber must yield once");
    assert_eq!(first.domain_event().v, 42);
    assert!(
        stream.next().is_none(),
        "stream must terminate after the genesis event"
    );
    assert!(
        stream.next().is_none(),
        "FusedIterator must keep returning None"
    );
    assert_eq!(
        stream.size_hint(),
        (0, Some(0)),
        "size_hint must reach (0,0)"
    );
}
#[test]
fn fiber_history_iter_rev_walks_through_detach_and_rescue() {
    let (mut store, _td) = new_store();
    let mut writer = store.writer();
    let r0 = writer.begin(Payload { v: 1 }).expect("begin");
    let live = r0.fiber();
    let fid = live.fiber_id();
    let live = writer
        .append(live, Payload { v: 2 })
        .expect("append")
        .fiber();
    let dr = writer.detach(live, Payload { v: 3 }).expect("detach");
    let detached = dr.fiber();
    let resumed = writer
        .resume(detached, Payload { v: 4 })
        .expect("resume")
        .fiber();
    let _ = writer
        .append(resumed, Payload { v: 5 })
        .expect("append post-resume");
    let reader = writer.reader();
    let chronological: Vec<u64> = reader
        .fiber(fid)
        .iter()
        .expect("iter")
        .map(|e| e.domain_event().v)
        .collect();
    let reverse: Vec<u64> = reader
        .fiber(fid)
        .iter_rev()
        .expect("iter_rev")
        .map(|e| e.domain_event().v)
        .collect();
    let expected_rev: Vec<u64> = chronological.iter().rev().copied().collect();
    assert_eq!(
        reverse, expected_rev,
        "iter_rev must walk the precursor chain across detach + rescue; \
         chronological={chronological:?} reverse={reverse:?}"
    );
    assert_eq!(
        reverse,
        vec![5, 4, 3, 2, 1],
        "iter_rev must surface every event on fiber a including the detach and rescue events"
    );
}
#[test]
fn fiber_history_iter_rev_size_hint_decrements_monotonically() {
    let (mut store, _td) = new_store();
    let mut writer = store.writer();
    let r0 = writer.begin(Payload { v: 1 }).expect("begin");
    let mut live = r0.fiber();
    let fid = live.fiber_id();
    for v in [2u64, 3] {
        live = writer.append(live, Payload { v }).expect("append").fiber();
    }
    let reader = writer.reader();
    let mut stream = reader.fiber(fid).iter_rev().expect("iter_rev");
    assert_eq!(
        stream.size_hint(),
        (3, Some(3)),
        "initial size_hint must equal fiber length"
    );
    let _ = stream.next();
    assert_eq!(stream.size_hint(), (2, Some(2)));
    let _ = stream.next();
    assert_eq!(stream.size_hint(), (1, Some(1)));
    let _ = stream.next();
    assert_eq!(stream.size_hint(), (0, Some(0)));
    assert!(stream.next().is_none());
}
