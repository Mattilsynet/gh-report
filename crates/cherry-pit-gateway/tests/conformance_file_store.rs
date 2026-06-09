//! Registrant 2: `MsgpackFileStore` against the conformance harness.
//!
//! Second of three SM-4 registrants (in-memory landed in
//! `cherry-pit-core/tests/conformance_in_memory.rs`; projection
//! file-store lands in `cherry-pit-projection/tests/`). Future fourth
//! registrant is the pardosa-backed store (Track 2.2) — its shape
//! must match this one verbatim.
//!
//! ## tokio
//!
//! Gateway has `tokio` in dev-deps with `macros, rt-multi-thread`
//! (see `Cargo.toml`). `#[tokio::test]` is the natural runtime driver
//! here. The harness fn is `async fn` and caller-owned — we just
//! `.await` it.
//!
//! ## Per-scenario isolation
//!
//! The harness calls `make_store()` once per scenario. Each call
//! constructs a fresh `MsgpackFileStore` rooted at a fresh `TempDir`.
//! The `TempDir` is owned by an Arc-Mutex'd Vec retained for the
//! duration of the harness call so the directories outlive their
//! stores (drop-order matters — `MsgpackFileStore` holds a `.lock`
//! file handle that must close before the `TempDir` is reaped).
//!
//! ## Q01 (linus round-2 carry-over)
//!
//! Scenario 5 asserts `StoreError::Infrastructure` for append-to-
//! never-created. `msgpack_file.rs:504-508` produces exactly
//! `Infrastructure` (verified by reading the impl pre-test). Keeping
//! the harness narrow; broadening to `Infrastructure |
//! ConcurrencyConflict` would weaken the contract without cause. If
//! the pardosa registrant later diverges, we relax then.

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
