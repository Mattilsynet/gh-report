//! Typestate enforcement on the ADR-0018 public migration path.
//!
//! Asserts that every fiber state reachable through the ADR-0018 public
//! lifecycle (`Defined` via begin/append, `Detached` via detach,
//! `Defined`-after-rescue via resume) round-trips under
//! [`migrate_keep`] with state preserved.
use pardosa::store::migrate::migrate_keep;
use pardosa::store::{Event, EventStore, FiberId, GenomeSafe, HasEventSchemaSource};
use tempfile::TempDir;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct OldV1 {
    v: u32,
}
impl HasEventSchemaSource for OldV1 {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct NewV2 {
    v: u64,
}
impl HasEventSchemaSource for NewV2 {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
#[derive(Debug, PartialEq, Eq)]
struct UpcastFailed;
#[allow(
    clippy::needless_pass_by_value,
    clippy::unnecessary_wraps,
    reason = "signature dictated by migrate_keep's FnMut(Event<Old>) -> Result<New, E> contract"
)]
fn upcast(e: Event<OldV1>) -> Result<NewV2, UpcastFailed> {
    Ok(NewV2 {
        v: u64::from(e.domain_event().v),
    })
}
fn migrate_to(new_path: &std::path::Path, old_path: &std::path::Path) -> (u64, u64) {
    let report = migrate_keep::<OldV1, NewV2, UpcastFailed, _>(old_path, new_path, upcast)
        .expect("migrate_keep ok");
    let counts = (report.old_event_count(), report.new_event_count());
    drop(report);
    counts
}
/// Defined-state lifecycle (begin → append → append) round-trips
/// through `migrate_keep` with fiber state preserved. The per-fiber
/// `Migrate(MigrationPolicy::Keep)` action is *never* invoked — the
/// public lifecycle uses `Create`/`Update` only.
#[test]
fn migrate_keep_round_trips_defined_state_lifecycle() {
    let td = TempDir::new().expect("tempdir");
    let old = td.path().join("old.pgno");
    let new = td.path().join("new.pgno");
    let sidecar = td.path().join("ack.sidecar");
    {
        let mut store: EventStore<OldV1> = EventStore::<OldV1>::create(&old).expect("create old");
        let mut writer = store.writer();
        let r0 = writer.begin(OldV1 { v: 1 }).expect("begin");
        let live = r0.fiber();
        let r1 = writer.append(live, OldV1 { v: 2 }).expect("append");
        let live = r1.fiber();
        let _r2 = writer.append(live, OldV1 { v: 3 }).expect("append");
        let _ = writer.sync().expect("sync");
    }
    let (old_n, new_n) = migrate_to(&new, &old);
    assert_eq!((old_n, new_n), (3, 3));
    let store: EventStore<NewV2> = EventStore::<NewV2>::open(&new).expect("open new");
    let reader = store.reader();
    let events: Vec<(FiberId, bool, u64)> = {
        let mut cur = reader.cursor(&sidecar).expect("cursor");
        cur.tail()
            .map(|r| {
                let e = r.expect("tail item");
                (e.fiber_id(), e.detached(), e.domain_event().v)
            })
            .collect()
    };
    assert_eq!(events.len(), 3);
    let fid = events[0].0;
    assert!(
        events.iter().all(|(f, d, _)| *f == fid && !*d),
        "all three events stay on one Defined fiber after migrate_keep; got {events:?}"
    );
    assert_eq!(
        events.iter().map(|(_, _, v)| *v).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "Defined-state event order preserved"
    );
}
/// Detached-state lifecycle (begin → detach) round-trips: the
/// post-migration fiber is still `Detached`. The per-fiber
/// `Migrate(MigrationPolicy::Keep)` action that `transition()`
/// admits for `Detached → Detached` is *not* the mechanism — the
/// public path uses `Detach` only.
#[test]
fn migrate_keep_round_trips_detached_state_lifecycle() {
    let td = TempDir::new().expect("tempdir");
    let old = td.path().join("old.pgno");
    let new = td.path().join("new.pgno");
    let sidecar = td.path().join("ack.sidecar");
    {
        let mut store: EventStore<OldV1> = EventStore::<OldV1>::create(&old).expect("create old");
        let mut writer = store.writer();
        let r0 = writer.begin(OldV1 { v: 10 }).expect("begin");
        let live = r0.fiber();
        let _dr = writer.detach(live, OldV1 { v: 11 }).expect("detach");
        let _ = writer.sync().expect("sync");
    }
    let (old_n, new_n) = migrate_to(&new, &old);
    assert_eq!((old_n, new_n), (2, 2));
    let store: EventStore<NewV2> = EventStore::<NewV2>::open(&new).expect("open new");
    let reader = store.reader();
    let events: Vec<(FiberId, bool, u64)> = {
        let mut cur = reader.cursor(&sidecar).expect("cursor");
        cur.tail()
            .map(|r| {
                let e = r.expect("tail item");
                (e.fiber_id(), e.detached(), e.domain_event().v)
            })
            .collect()
    };
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].0, events[1].0, "same fiber across detach");
    assert!(!events[0].1, "begin event is not detached-flagged");
    assert!(events[1].1, "detach event is detached-flagged");
    assert_eq!(events[0].2, 10);
    assert_eq!(events[1].2, 11);
}
/// Defined-after-rescue lifecycle (begin → detach → resume → append)
/// round-trips. Rescue (`Detached → Defined`) is the substrate
/// counterpart to `Migrate(MigrationPolicy::Keep)` for retention
/// semantics but uses `FiberAction::Rescue`, not
/// `FiberAction::Migrate(_)`. `migrate_keep` replays it via
/// `commit_rescue`, never `migrate_fiber`.
#[test]
fn migrate_keep_round_trips_rescued_then_defined_lifecycle() {
    let td = TempDir::new().expect("tempdir");
    let old = td.path().join("old.pgno");
    let new = td.path().join("new.pgno");
    let sidecar = td.path().join("ack.sidecar");
    {
        let mut store: EventStore<OldV1> = EventStore::<OldV1>::create(&old).expect("create old");
        let mut writer = store.writer();
        let r0 = writer.begin(OldV1 { v: 100 }).expect("begin");
        let live = r0.fiber();
        let dr = writer.detach(live, OldV1 { v: 101 }).expect("detach");
        let detached = dr.fiber();
        let rr = writer.resume(detached, OldV1 { v: 102 }).expect("resume");
        let live = rr.fiber();
        let _ = writer.append(live, OldV1 { v: 103 }).expect("append");
        let _ = writer.sync().expect("sync");
    }
    let (old_n, new_n) = migrate_to(&new, &old);
    assert_eq!((old_n, new_n), (4, 4));
    let store: EventStore<NewV2> = EventStore::<NewV2>::open(&new).expect("open new");
    let reader = store.reader();
    let events: Vec<(FiberId, bool, u64)> = {
        let mut cur = reader.cursor(&sidecar).expect("cursor");
        cur.tail()
            .map(|r| {
                let e = r.expect("tail item");
                (e.fiber_id(), e.detached(), e.domain_event().v)
            })
            .collect()
    };
    assert_eq!(events.len(), 4);
    let fid = events[0].0;
    assert!(events.iter().all(|(f, _, _)| *f == fid), "single fiber");
    assert_eq!(
        events.iter().map(|(_, d, _)| *d).collect::<Vec<_>>(),
        vec![false, true, false, false],
        "detached flag pattern preserved (begin, detach, resume, append)"
    );
    assert_eq!(
        events.iter().map(|(_, _, v)| *v).collect::<Vec<_>>(),
        vec![100, 101, 102, 103]
    );
}
/// Multi-fiber lifecycle interleaving Defined and Detached fibers
/// round-trips under `migrate_keep`. Each fiber's terminal state
/// matches its source-stream terminal state; no per-fiber
/// `Migrate(policy)` is invoked at any point along the path.
#[test]
fn migrate_keep_round_trips_multi_fiber_interleaved_states() {
    let td = TempDir::new().expect("tempdir");
    let old = td.path().join("old.pgno");
    let new = td.path().join("new.pgno");
    let sidecar = td.path().join("ack.sidecar");
    {
        let mut store: EventStore<OldV1> = EventStore::<OldV1>::create(&old).expect("create old");
        let mut writer = store.writer();
        let ra = writer.begin(OldV1 { v: 1 }).expect("a begin");
        let live_a = ra.fiber();
        let rb = writer.begin(OldV1 { v: 2 }).expect("b begin");
        let live_b = rb.fiber();
        let _ = writer.append(live_a, OldV1 { v: 3 }).expect("a append");
        let dr_b = writer.detach(live_b, OldV1 { v: 4 }).expect("b detach");
        let detached_b = dr_b.fiber();
        let _ = writer.resume(detached_b, OldV1 { v: 5 }).expect("b resume");
        let _ = writer.sync().expect("sync");
    }
    let (old_n, new_n) = migrate_to(&new, &old);
    assert_eq!((old_n, new_n), (5, 5));
    let store: EventStore<NewV2> = EventStore::<NewV2>::open(&new).expect("open new");
    let reader = store.reader();
    let events: Vec<(FiberId, bool, u64)> = {
        let mut cur = reader.cursor(&sidecar).expect("cursor");
        cur.tail()
            .map(|r| {
                let e = r.expect("tail item");
                (e.fiber_id(), e.detached(), e.domain_event().v)
            })
            .collect()
    };
    assert_eq!(events.len(), 5);
    let fa = events[0].0;
    let fb = events[1].0;
    assert_ne!(fa, fb, "two distinct fibers");
    assert_eq!(events[2].0, fa, "event 2 is on fiber A");
    assert_eq!(events[3].0, fb, "event 3 (detach) is on fiber B");
    assert_eq!(events[4].0, fb, "event 4 (resume) is on fiber B");
    assert_eq!(
        events.iter().map(|(_, d, _)| *d).collect::<Vec<_>>(),
        vec![false, false, false, true, false]
    );
    assert_eq!(
        events.iter().map(|(_, _, v)| *v).collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5]
    );
}
