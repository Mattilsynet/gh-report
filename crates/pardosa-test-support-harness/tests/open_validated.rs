//! Public adopter contract for
//! [`pardosa::store::EventStore::open_validated`]
//! (ADR-0018 §D7; Fiber-semantics goal 6).
//!
//! A persisted journal whose payloads all pass [`Validate`] reopens
//! identically to [`EventStore::open`]; a payload that decodes but
//! whose `validate()` rejects surfaces as
//! [`ValidatedReplayError::Payload`] from `open_validated`, while
//! [`EventStore::open`] remains the unchecked fast path.
use pardosa::store::{
    EventStore, GenomeSafe, HasEventSchemaSource, Validate, ValidatedReplayError,
};
use tempfile::TempDir;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Marker {
    v: u64,
}
impl HasEventSchemaSource for Marker {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
#[derive(Debug, PartialEq, Eq)]
struct MarkerInvalid;
impl Validate for Marker {
    type Error = MarkerInvalid;
    fn validate(&self) -> Result<(), Self::Error> {
        if self.v == 0 {
            Err(MarkerInvalid)
        } else {
            Ok(())
        }
    }
}
#[test]
fn valid_persisted_store_reopens_via_open_validated() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let fid;
    {
        let mut store: EventStore<Marker> =
            EventStore::<Marker>::create(&journal).expect("path-backed create must succeed");
        let mut writer = store.writer();
        let r0 = writer
            .begin(Marker { v: 1 })
            .expect("start fiber with valid payload");
        let live = r0.fiber();
        fid = live.fiber_id();
        let live = writer
            .append(live, Marker { v: 2 })
            .expect("append valid payload")
            .fiber();
        let _ = writer
            .append(live, Marker { v: 3 })
            .expect("append valid payload again");
        let _ = writer.sync().expect("sync");
    }
    let store: EventStore<Marker> = EventStore::<Marker>::open_validated(&journal)
        .expect("open_validated must succeed when every payload satisfies Validate");
    let reader = store.reader();
    let history: Vec<u64> = reader
        .fiber(fid)
        .iter()
        .expect("fiber history present after validated rehydrate")
        .map(|ev| ev.domain_event().v)
        .collect();
    assert_eq!(
        history,
        vec![1, 2, 3],
        "validated rehydrate must observe every persisted event in commit order"
    );
}
#[test]
fn invalid_payload_rejected_by_open_validated_while_open_remains_unchecked() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let fid;
    {
        let mut store: EventStore<Marker> =
            EventStore::<Marker>::create(&journal).expect("path-backed create must succeed");
        let mut writer = store.writer();
        let r0 = writer
            .begin(Marker { v: 1 })
            .expect("start fiber with valid payload");
        let live = r0.fiber();
        fid = live.fiber_id();
        let _ = writer
            .append(live, Marker { v: 0 })
            .expect("EventStore writer does not call Validate; v=0 must be appendable");
        let _ = writer.sync().expect("sync");
    }
    let result = EventStore::<Marker>::open_validated(&journal);
    match result {
        Err(ValidatedReplayError::Payload(MarkerInvalid)) => {}
        Err(other) => {
            panic!("expected ValidatedReplayError::Payload(MarkerInvalid), got {other:?}")
        }
        Ok(_) => panic!("open_validated must reject the v=0 payload"),
    }
    let store: EventStore<Marker> = EventStore::<Marker>::open(&journal)
        .expect("EventStore::open is the unchecked path and must still succeed");
    let reader = store.reader();
    let history: Vec<u64> = reader
        .fiber(fid)
        .iter()
        .expect("fiber history present after unchecked rehydrate")
        .map(|ev| ev.domain_event().v)
        .collect();
    assert_eq!(
        history,
        vec![1, 0],
        "unchecked open exposes both events including the invalid v=0 payload"
    );
}
