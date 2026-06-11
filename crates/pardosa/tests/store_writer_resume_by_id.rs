use pardosa::store::{EventStore, FiberState, GenomeSafe, HasEventSchemaSource, PardosaError};
use tempfile::TempDir;

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Payload {
    v: u64,
}

impl HasEventSchemaSource for Payload {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}

impl pardosa::store::Validate for Payload {
    type Error = core::convert::Infallible;

    fn validate(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn journal_path(td: &TempDir) -> std::path::PathBuf {
    td.path().join("resume-by-id.pgno")
}

#[test]
fn resume_defined_round_trips_after_reopen_and_append() {
    let td = TempDir::new().expect("tempdir");
    let path = journal_path(&td);
    let fiber_id = {
        let mut store = EventStore::<Payload>::create(&path).expect("create");
        let receipt = store.writer().begin(Payload { v: 1 }).expect("begin");
        let fiber_id = receipt.fiber().fiber_id();
        let _ = store.writer().sync().expect("sync initial");
        fiber_id
    };
    {
        let mut store = EventStore::<Payload>::open_validated(&path).expect("reopen");
        let receipt = store
            .writer()
            .resume_defined(fiber_id, Payload { v: 2 })
            .expect("resume defined");
        let live = receipt.fiber();
        let _ = store
            .writer()
            .append(live, Payload { v: 3 })
            .expect("append after resume_defined");
        let _ = store.writer().sync().expect("sync resumed");
    }
    let store = EventStore::<Payload>::open_validated(&path).expect("reopen after append");
    let payloads: Vec<u64> = store
        .reader()
        .fiber(fiber_id)
        .iter()
        .expect("fiber history")
        .map(|event| event.domain_event().v)
        .collect();
    assert_eq!(payloads, vec![1, 2, 3]);
}

#[test]
fn resume_defined_rejects_detached_fiber_by_id() {
    let td = TempDir::new().expect("tempdir");
    let path = journal_path(&td);
    let fiber_id = {
        let mut store = EventStore::<Payload>::create(&path).expect("create");
        let receipt = store.writer().begin(Payload { v: 1 }).expect("begin");
        let live = receipt.fiber();
        let fiber_id = live.fiber_id();
        let _ = store
            .writer()
            .detach(live, Payload { v: 2 })
            .expect("detach");
        let _ = store.writer().sync().expect("sync detached");
        fiber_id
    };
    let mut store = EventStore::<Payload>::open_validated(&path).expect("reopen");
    let err = store
        .writer()
        .resume_defined(fiber_id, Payload { v: 3 })
        .expect_err("detached fiber must not resume_defined");
    assert!(matches!(err, PardosaError::InvalidTransition { .. }));
}

#[test]
fn rescue_detached_by_id_round_trips_after_reopen() {
    let td = TempDir::new().expect("tempdir");
    let path = journal_path(&td);
    let fiber_id = {
        let mut store = EventStore::<Payload>::create(&path).expect("create");
        let receipt = store.writer().begin(Payload { v: 1 }).expect("begin");
        let live = receipt.fiber();
        let fiber_id = live.fiber_id();
        let _ = store
            .writer()
            .detach(live, Payload { v: 2 })
            .expect("detach");
        let _ = store.writer().sync().expect("sync detached");
        fiber_id
    };
    {
        let mut store = EventStore::<Payload>::open_validated(&path).expect("reopen");
        let receipt = store
            .writer()
            .rescue_detached(fiber_id, Payload { v: 3 })
            .expect("rescue detached");
        assert_eq!(receipt.fiber().fiber_id(), fiber_id);
        assert_eq!(store.reader().fiber(fiber_id).state(), FiberState::Defined);
        let _ = store.writer().sync().expect("sync rescued");
    }
    let store = EventStore::<Payload>::open_validated(&path).expect("reopen after rescue");
    let payloads: Vec<u64> = store
        .reader()
        .fiber(fiber_id)
        .iter()
        .expect("fiber history")
        .map(|event| event.domain_event().v)
        .collect();
    assert_eq!(payloads, vec![1, 2, 3]);
}

#[test]
fn rescue_detached_rejects_defined_fiber_by_id() {
    let td = TempDir::new().expect("tempdir");
    let path = journal_path(&td);
    let fiber_id = {
        let mut store = EventStore::<Payload>::create(&path).expect("create");
        let receipt = store.writer().begin(Payload { v: 1 }).expect("begin");
        let fiber_id = receipt.fiber().fiber_id();
        let _ = store.writer().sync().expect("sync defined");
        fiber_id
    };
    let mut store = EventStore::<Payload>::open_validated(&path).expect("reopen");
    let err = store
        .writer()
        .rescue_detached(fiber_id, Payload { v: 2 })
        .expect_err("defined fiber must not rescue_detached");
    assert!(matches!(err, PardosaError::InvalidTransition { .. }));
}

#[test]
fn resume_defined_rejects_fiber_id_from_other_dragline() {
    let td = TempDir::new().expect("tempdir");
    let local_path = td.path().join("local.pgno");
    let foreign_path = td.path().join("foreign.pgno");
    let foreign_id = {
        let mut foreign = EventStore::<Payload>::create(&foreign_path).expect("create foreign");
        let _ = foreign
            .writer()
            .begin(Payload { v: 0 })
            .expect("align id allocator");
        let receipt = foreign
            .writer()
            .begin(Payload { v: 9 })
            .expect("begin foreign");
        let fiber_id = receipt.fiber().fiber_id();
        let _ = foreign.writer().sync().expect("sync foreign");
        fiber_id
    };
    let mut local = EventStore::<Payload>::create(&local_path).expect("create local");
    let _ = local.writer().begin(Payload { v: 1 }).expect("begin local");
    let _ = local.writer().sync().expect("sync local");
    let err = local
        .writer()
        .resume_defined(foreign_id, Payload { v: 2 })
        .expect_err("unminted local fiber id must be rejected");
    assert!(matches!(err, PardosaError::FiberNotFound(_)));
}
