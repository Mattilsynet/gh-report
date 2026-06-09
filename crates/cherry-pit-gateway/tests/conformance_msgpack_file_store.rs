//! Registrant: `MsgpackFileStore` against the SM-4 conformance harness.
//!
//! This is the load-bearing exit gate for Track 7.1: proof that the
//! conformance harness is sound via a second known-good candidate. The
//! harness lives in `cherry_pit_core::testing::conformance` and is
//! adapter-agnostic; this file exercises only its public surface.
//!
//! ## 6 `EventStore` scenarios (all driven by `assert_event_store_conformance`)
//!
//! Per `cherry_pit_core::testing::conformance`, the 6 invariant
//! scenarios are run on isolated fresh stores from `make_store`:
//!
//! 1. create → load round-trips with contiguous sequences 1..N.
//! 2. stale `expected_sequence` on `append` yields `ConcurrencyConflict`.
//! 3. `load` of unknown `AggregateId` returns `Ok(empty)` (CHE-0019:R1).
//! 4. empty-events `create` returns Infrastructure error.
//! 5. `append` to a phantom (never-created) aggregate returns
//!    Infrastructure error.
//! 6. create then append produces a monotone, contiguous stream.
//!
//! Substrate notes (MsgpackFileStore-specific):
//!
//! - Each scenario receives a fresh `MsgpackFileStore` rooted at a fresh
//!   `TempDir` via the factory closure; `TempDir`s are retained in an
//!   `Arc<Mutex<Vec<TempDir>>>` so they outlive their stores (drop-order
//!   matters: `MsgpackFileStore` holds a `.lock` file handle that must
//!   close before the `TempDir` is reaped).
//! - File persistence is one `.msgpack` file per aggregate; create is
//!   atomic via temp-and-rename (CHE-0032, CHE-0043, CHE-0047:R1).
//! - Concurrency-violation detection in `append` uses per-aggregate
//!   `tokio::sync::Mutex` via `scc::HashMap` (CHE-0035) + a full-file
//!   rewrite under the per-aggregate lock (CHE-0036:R1).
//!
//! ## Why `#[tokio::test]`
//!
//! Unlike `PardosaEventStore` (which resolves to ready futures and
//! supports a hand-rolled `block_on`), `MsgpackFileStore` performs real
//! async I/O via `tokio::fs`. The gateway crate already carries tokio
//! with `macros, rt-multi-thread` in dev-deps (Cargo.toml:37), so
//! `#[tokio::test]` is the natural and correct driver here.
//!
//! ## CHE-0057:R4 compliance
//!
//! The harness is generic over `S: EventStore` — no extension-trait
//! bound (`PurgeableEventStore`, `HashChainedEventStore`,
//! `SingleWriterEventStore`). `MsgpackFileStore` implements only core
//! `EventStore` + `Default`; no extension traits are implemented or
//! required. The registration is trait-clean.

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
