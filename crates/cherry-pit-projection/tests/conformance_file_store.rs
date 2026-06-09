//! Registrant 3: a [`Projection`] impl exercised against a file-backed
//! [`EventStore`] via [`assert_projection_conformance`].
//!
//! Third of three SM-4 registrants. The harness probes the
//! [`Projection`] trait contract (CHE-0048:R3 replay-equivalence,
//! fold determinism). The backing [`EventStore`] is the gateway's
//! [`MsgpackFileStore`] — exercising projection replay over
//! envelopes that round-trip through real on-disk msgpack frames
//! rather than just an in-process Vec.
//!
//! ## Why pair with `MsgpackFileStore` rather than `InMemoryEventStore`
//!
//! The harness signature takes any `S: EventStore<Event = P::Event>`
//! — the in-memory store would compile and pass. Pairing with the
//! file store is *meaningful*: it proves the projection fold is
//! stable across the serde boundary, not just across an in-process
//! shuffle. The gateway dev-dep is already justified by SM-4 SC#10
//! ("registrants must exercise a non-trivial adapter pairing"), so
//! adding it here costs nothing structurally.
//!
//! ## Q02 (linus round-2 carry-over)
//!
//! `FileProjectionStore` (this crate's own primary type) is **not**
//! exercised by `assert_projection_conformance` — that harness probes
//! the `Projection` *trait*, while `FileProjectionStore` is a
//! snapshot/checkpoint persistence backend separate from
//! `Projection::apply`. Commutativity / dedup-under-resume would
//! require a *new* harness that probes `FileProjectionStore::persist`
//! / `load_snapshot` semantics — that's a different abstraction and
//! out of SM-4 scope. Logged as future work; do not add to
//! `assert_projection_conformance`.
//!
//! ## tokio
//!
//! cherry-pit-projection has `tokio` in dev-deps with
//! `macros, rt-multi-thread`. `#[tokio::test]` is the natural driver
//! for the async harness fn.

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
