//! Registrant: `MsgpackFileStore` against the SM-4 conformance harness.
//!
//! Load-bearing exit gate for Track 7.1: proves the harness
//! (`cherry_pit_core::testing::conformance`) is sound via a second
//! known-good candidate.
//!
//! The 6 `EventStore` invariant scenarios run on fresh stores from
//! `make_store`: create→load round-trips contiguous sequences; stale
//! `expected_sequence` yields `ConcurrencyConflict`; unknown
//! `AggregateId` load returns `Ok(empty)` (CHE-0019:R1); empty-events
//! create and append-to-phantom both error Infrastructure;
//! create-then-append is monotone.
//!
//! Each scenario gets a fresh store on a fresh `TempDir` (drop order
//! matters: `.lock` must close first). Create is atomic via
//! temp-and-rename (CHE-0032, CHE-0043); concurrency detection uses
//! per-aggregate locks via `scc::HashMap` (CHE-0035) plus full-file
//! rewrite under lock (CHE-0036:R1).
//!
//! `#[tokio::test]` drives it — real async I/O via `tokio::fs`,
//! unlike `PardosaEventStore`'s `block_on`.
//! CHE-0057:R4: no extension-trait bound; only `EventStore` +
//! `Default` — trait-clean.

use std::sync::{Arc, Mutex};

use cherry_pit_core::testing::conformance::{
    assert_event_store_conformance, assert_projection_conformance,
};
use cherry_pit_core::{DomainEvent, EventEnvelope, Projection};
use cherry_pit_gateway::MsgpackFileStore;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum TestEvent {
    Incremented(i64),
}

impl DomainEvent for TestEvent {
    fn event_type(&self) -> &'static str {
        "conformance.incremented"
    }
}

#[derive(Default, Debug, PartialEq)]
struct CounterView {
    total: i64,
    applied: u64,
}

impl Projection for CounterView {
    type Event = TestEvent;
    fn apply(&mut self, env: &EventEnvelope<TestEvent>) {
        match env.payload() {
            TestEvent::Incremented(n) => self.total += n,
        }
        self.applied += 1;
    }
}

/// Load-bearing gate for Track 7.1: all 6 `EventStore` conformance
/// scenarios run against `MsgpackFileStore`. Failure here = the
/// filesystem adapter has a behavioural gap relative to the trait
/// contract. Don't relax the harness; surface to moltke as SURPRISE.
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
    let make_event = |i: u32| TestEvent::Incremented(i64::from(i) + 1);

    assert_event_store_conformance::<MsgpackFileStore<TestEvent>, _, _>(factory, make_event).await;
}

/// Projection-over-msgpack-store: verifies the harness's projection
/// scenarios run cleanly when the underlying store is file-backed.
/// The harness drives create + load + apply, asserting projection
/// state equality (CHE-0048:R3 replay-equivalence).
#[tokio::test]
async fn counter_view_projection_conforms_over_msgpack_file_store() {
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
    let make_event = |i: u32| TestEvent::Incremented(i64::from(i) + 1);

    assert_projection_conformance::<CounterView, MsgpackFileStore<TestEvent>, _, _, _>(
        factory,
        make_event,
        |a, b| a == b,
    )
    .await;
}
