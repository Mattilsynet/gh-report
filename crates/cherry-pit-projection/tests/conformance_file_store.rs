//! Registrant 3: a [`Projection`] impl exercised against a file-backed
//! [`EventStore`] via [`assert_projection_conformance`].
//!
//! Third of three SM-4 registrants. The harness probes the
//! [`Projection`] trait contract (CHE-0048:R3 replay-equivalence,
//! fold determinism). The backing [`EventStore`] is the gateway's
//! [`MsgpackFileStore`], so replay is exercised over envelopes that
//! round-trip through real on-disk msgpack frames rather than an
//! in-process `Vec`.
//!
//! Pairing with `MsgpackFileStore` (rather than `InMemoryEventStore`,
//! which would also satisfy the harness signature) proves the fold is
//! stable across the serde boundary, per SM-4 SC#10 ("registrants must
//! exercise a non-trivial adapter pairing").
//!
//! `FileProjectionStore` (this crate's own persistence backend) is
//! **not** exercised here — separate from `Projection::apply`; a
//! commutativity/dedup-under-resume harness for it is future work,
//! out of SM-4 scope.

use std::sync::{Arc, Mutex};

use cherry_pit_core::testing::conformance::assert_projection_conformance;
use cherry_pit_core::{DomainEvent, EventEnvelope, Projection};
use cherry_pit_gateway::MsgpackFileStore;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum TestEvent {
    Tallied(i64),
}

impl DomainEvent for TestEvent {
    fn event_type(&self) -> &'static str {
        "conformance.tallied"
    }
}

/// Tally projection: sums signed integers from `Tallied` events and
/// tracks how many envelopes have been folded in. Both fields move
/// monotonically away from `Default`, so replay equivalence is
/// observable.
#[derive(Default, Debug, PartialEq)]
struct Tally {
    total: i64,
    applied: u64,
}

impl Projection for Tally {
    type Event = TestEvent;
    fn apply(&mut self, env: &EventEnvelope<TestEvent>) {
        match env.payload() {
            TestEvent::Tallied(n) => self.total += n,
        }
        self.applied += 1;
    }
}

#[tokio::test]
async fn tally_projection_conforms_over_file_store() {
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
    let make_event = |i: u32| TestEvent::Tallied(i64::from(i) + 1);

    assert_projection_conformance::<Tally, MsgpackFileStore<TestEvent>, _, _, _>(
        factory,
        make_event,
        |a, b| a == b,
    )
    .await;
}
