use cherry_pit_core::testing::conformance::assert_event_store_conformance;
use cherry_pit_core::DomainEvent;
use cherry_pit_pardosa::PardosaEventStore;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum TestEvent {
    Incremented(i64),
}

impl DomainEvent for TestEvent {
    fn event_type(&self) -> &'static str {
        "conformance.incremented"
    }
}

#[tokio::test]
async fn pardosa_event_store_conforms() {
    let dirs: Arc<Mutex<Vec<TempDir>>> = Arc::new(Mutex::new(Vec::new()));
    let factory = {
        let dirs = Arc::clone(&dirs);
        move || {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("events.pgno");
            let store = PardosaEventStore::<TestEvent>::create_pgno(&path).expect("create store");
            dirs.lock().expect("dirs mutex").push(dir);
            store
        }
    };
    let make_event = |i: u32| TestEvent::Incremented(i64::from(i) + 1);
    assert_event_store_conformance::<PardosaEventStore<TestEvent>, _, _>(factory, make_event)
        .await;
}
