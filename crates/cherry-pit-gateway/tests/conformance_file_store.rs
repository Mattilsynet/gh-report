//! Registrant 2: `MsgpackFileStore` against the conformance harness.
//!
//! Second of three SM-4 registrants (in-memory in
//! `cherry-pit-core/tests/conformance_in_memory.rs`; projection
//! file-store in `cherry-pit-projection/tests/`); the pardosa-backed
//! store (Track 2.2) is a future fourth registrant with matching shape.
//!
//! `#[tokio::test]` drives the `async fn` harness directly (gateway
//! carries `tokio` dev-dep with `macros, rt-multi-thread`). Each
//! scenario gets a fresh `MsgpackFileStore` rooted at a fresh
//! `TempDir`, retained in an `Arc<Mutex<Vec<TempDir>>>` for the
//! harness call's duration — drop order matters, since
//! `MsgpackFileStore` holds a `.lock` file handle that must close
//! before the `TempDir` is reaped.
//!
//! Q01 (linus round-2): scenario 5 asserts `StoreError::Infrastructure`
//! for append-to-never-created; `msgpack_file.rs:504-508` produces
//! exactly that variant. Kept narrow deliberately — broadening to
//! `Infrastructure | ConcurrencyConflict` weakens the contract without
//! cause; relax only if the pardosa registrant diverges.

use std::sync::{Arc, Mutex};

use cherry_pit_core::DomainEvent;
use cherry_pit_core::testing::conformance::assert_event_store_conformance;
use cherry_pit_gateway::MsgpackFileStore;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum TestEvent {
    Stamped(u32),
}

impl DomainEvent for TestEvent {
    fn event_type(&self) -> &'static str {
        "conformance.stamped"
    }
}

#[tokio::test]
async fn msgpack_file_store_conforms() {
    let dirs: Arc<Mutex<Vec<TempDir>>> = Arc::new(Mutex::new(Vec::new()));

    let factory = {
        let dirs = Arc::clone(&dirs);
        move || {
            let dir = tempfile::tempdir().expect("tempdir");
            let store = MsgpackFileStore::<TestEvent>::new(dir.path());
            dirs.lock().expect("dirs mutex").push(dir);
            store
        }
    };
    let make_event = |i: u32| TestEvent::Stamped(i);

    assert_event_store_conformance::<MsgpackFileStore<TestEvent>, _, _>(factory, make_event).await;
}
