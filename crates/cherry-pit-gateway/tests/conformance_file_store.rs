use std::sync::{Arc, Mutex};

use cherry_pit_core::testing::conformance::{
    assert_event_store_conformance, assert_projection_conformance,
};
use cherry_pit_core::{EventEnvelope, Projection};
use pardosa_cherry_pit_test_support::PgnoEventStore;
use pardosa_cherry_pit_test_support::fixture::RecordedEvent;
use tempfile::TempPath;

fn temp_pgno_path() -> TempPath {
    let file = tempfile::NamedTempFile::new().expect("create temp file");
    let path = file.into_temp_path();
    std::fs::remove_file(&path).expect("clear placeholder so create_pgno starts fresh");
    path
}

#[derive(Default, Debug, PartialEq)]
struct SumView {
    total: u64,
    applied: u64,
}

impl Projection for SumView {
    type Event = RecordedEvent;

    fn apply(&mut self, env: &EventEnvelope<Self::Event>) {
        let RecordedEvent::Recorded { value } = env.payload();
        self.total += u64::from(*value);
        self.applied += 1;
    }
}

#[tokio::test]
async fn pgno_event_store_conforms() {
    let paths: Arc<Mutex<Vec<TempPath>>> = Arc::new(Mutex::new(Vec::new()));

    let factory = {
        let paths = Arc::clone(&paths);
        move || {
            let path = temp_pgno_path();
            let store =
                PgnoEventStore::<RecordedEvent>::create_pgno(&path).expect("create store");
            paths.lock().expect("paths mutex").push(path);
            store
        }
    };
    let make_event = |i: u32| RecordedEvent::Recorded { value: i };

    assert_event_store_conformance::<PgnoEventStore<RecordedEvent>, _, _>(factory, make_event)
        .await;
}

#[tokio::test]
async fn sum_view_projection_conforms_over_pgno_event_store() {
    let paths: Arc<Mutex<Vec<TempPath>>> = Arc::new(Mutex::new(Vec::new()));

    let factory = {
        let paths = Arc::clone(&paths);
        move || {
            let path = temp_pgno_path();
            let store =
                PgnoEventStore::<RecordedEvent>::create_pgno(&path).expect("create store");
            paths.lock().expect("paths mutex").push(path);
            store
        }
    };
    let make_event = |i: u32| RecordedEvent::Recorded { value: i };

    assert_projection_conformance::<SumView, PgnoEventStore<RecordedEvent>, _, _, _>(
        factory,
        make_event,
        |a, b| a == b,
    )
    .await;
}
