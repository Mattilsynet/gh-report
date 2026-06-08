//! Publish-watermark observability on the path-backed `EventStore<T>`
//! reader surface (ADR-0018 §D3 observability; ADR-0016 §§D5–D7).
//!
//! Exercises `StoreReader::publish_watermark()` across the three
//! publisher modes reachable through the public API:
//!
//! * `EventStore::open` (no publisher attached) — always `None`.
//! * `EventStore::open_with_publisher` — `Some(last_event_id)` after a
//!   successful drain (anchors flushed and sidecar fsync-ed,
//!   ADR-0016 §D5).
//! * Reopen with `EventStore::open_with_publisher` against the same
//!   sidecar — the watermark recovers from the on-disk sidecar
//!   without any new publish activity (ADR-0016 §D6 / §D7).
use pardosa::store::{
    EventId, EventStore, FrontierPublisher, GenomeSafe, HasEventSchemaSource, PublishError,
};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
type PublishLog = Arc<Mutex<Vec<(String, Vec<u8>)>>>;
#[derive(Clone, Debug)]
struct LocalPublisher {
    log: PublishLog,
}
impl LocalPublisher {
    fn new() -> Self {
        Self {
            log: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn published(&self) -> Vec<(String, Vec<u8>)> {
        self.log.lock().expect("mutex").clone()
    }
}
impl FrontierPublisher for LocalPublisher {
    fn publish(&mut self, subject: &str, payload: &[u8]) -> Result<(), PublishError> {
        self.log
            .lock()
            .expect("mutex")
            .push((subject.to_owned(), payload.to_vec()));
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Order {
    id: u64,
}
impl HasEventSchemaSource for Order {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
fn seed_events(path: &std::path::Path, n: u64) -> EventId {
    let mut seed = EventStore::<Order>::create(path).expect("create");
    let mut last = None;
    for id in 0..n {
        let receipt = seed.writer().begin(Order { id }).expect("begin");
        last = Some(receipt.event_id());
    }
    let _ = seed.writer().sync().expect("seed sync");
    last.expect("at least one event seeded")
}
#[test]
fn publish_watermark_is_none_without_publisher() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let _last = seed_events(&journal, 3);
    let store = EventStore::<Order>::open(&journal).expect("open");
    assert_eq!(
        store.reader().publish_watermark(),
        None,
        "EventStore::open attaches no publisher; watermark must be None \
         (ADR-0016 §§D5–D7)"
    );
}
#[test]
fn publish_watermark_advances_after_drain() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    let last = seed_events(&journal, 3);
    let publisher = LocalPublisher::new();
    let probe = publisher.clone();
    let mut store = EventStore::<Order>::open_with_publisher(
        &journal,
        sidecar.clone(),
        "drain".to_owned(),
        1,
        Box::new(publisher),
    )
    .expect("open_with_publisher");
    assert_eq!(
        store.reader().publish_watermark(),
        None,
        "pre-sync, no anchors have been drained or recorded in the sidecar; \
         watermark must still be None (ADR-0016 §D5)"
    );
    let _ = store.writer().sync().expect("first sync drains anchors");
    assert_eq!(
        probe.published().len(),
        3,
        "harness precondition: drain must have published every reconstructed anchor"
    );
    assert_eq!(
        store.reader().publish_watermark(),
        Some(last),
        "after successful drain the watermark must point at the last \
         durably-published event (ADR-0016 §D5)"
    );
}
#[test]
fn publish_watermark_recovers_after_reopen() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    let last = seed_events(&journal, 2);
    {
        let publisher = LocalPublisher::new();
        let mut store = EventStore::<Order>::open_with_publisher(
            &journal,
            sidecar.clone(),
            "reopen".to_owned(),
            1,
            Box::new(publisher),
        )
        .expect("open_with_publisher");
        let _ = store.writer().sync().expect("first sync drains anchors");
        assert_eq!(
            store.reader().publish_watermark(),
            Some(last),
            "harness precondition: watermark covers the full line after first drain"
        );
        drop(store);
    }
    assert!(
        sidecar.exists(),
        "publish-watermark sidecar must have been fsync-ed before drop (ADR-0016 §D5)"
    );
    let republisher = LocalPublisher::new();
    let republish_probe = republisher.clone();
    let reopened = EventStore::<Order>::open_with_publisher(
        &journal,
        sidecar.clone(),
        "reopen".to_owned(),
        1,
        Box::new(republisher),
    )
    .expect("reopen_with_publisher");
    assert_eq!(
        reopened.reader().publish_watermark(),
        Some(last),
        "watermark must recover from the durably-fenced sidecar without any \
         publish activity (ADR-0016 §D6)"
    );
    assert!(
        republish_probe.published().is_empty(),
        "watermark observation must not trigger a republish (ADR-0016 §D7); \
         got {} anchors",
        republish_probe.published().len(),
    );
}
