//! Registrant: [`PardosaFileEventStore`] against the SM-4 conformance
//! harness (δ.3a — file-backed `PardosaEventStore`).
//!
//! Mirrors [`crates/cherry-pit-pardosa/tests/conformance_pardosa_store.rs`]
//! and [`crates/cherry-pit-gateway/tests/conformance_msgpack_file_store.rs`].
//! Drives the harness from `cherry_pit_core::testing::conformance` against
//! a fresh `PardosaFileEventStore` per scenario.
//!
//! ## `TempDir` retainer pattern
//!
//! The conformance harness signature is `Fn() -> S`: it does *not*
//! own any per-scenario auxiliary state. We therefore stash each
//! `tempfile::TempDir` (one per `make_store()` invocation) in a
//! `Mutex<Vec<TempDir>>` so the directories outlive the harness's
//! borrow of the `S` value. Drop-order matters:
//! `PardosaFileEventStore` holds an advisory flock on `.lock`, and
//! the flock must release (store dropped) **before** `TempDir`
//! removes the directory. Vec push order = drop order (LIFO on
//! `Vec::drop`), so the store is dropped before its tempdir at
//! end-of-test. Within the harness itself, stores from earlier
//! scenarios are not dropped until the test exits — that's safe
//! because each scenario uses its own fresh tempdir.

use std::future::Future;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

use cherry_pit_core::DomainEvent;
use cherry_pit_core::testing::conformance::assert_event_store_conformance;
use cherry_pit_pardosa::PardosaFileEventStore;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

// ── Hand-rolled block_on (no tokio in this test, CHE-0029:R4) ──────

struct NoopWaker;
impl Wake for NoopWaker {
    fn wake(self: Arc<Self>) {}
}

fn block_on<F: Future>(fut: F) -> F::Output {
    let waker: Waker = Arc::new(NoopWaker).into();
    let mut cx = Context::from_waker(&waker);
    let mut fut = pin!(fut);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

// ── Test event (mirrors conformance_pardosa_store::TestEvent) ─────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum TestEvent {
    Incremented(i64),
}

impl DomainEvent for TestEvent {
    fn event_type(&self) -> &'static str {
        "conformance.incremented"
    }
}

impl pardosa_encoding::Encode for TestEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Incremented(n) => {
                out.push(0u8);
                pardosa_encoding::Encode::encode(n, out);
            }
        }
    }
}

// CHE-0064:R2 — symmetric Decode is required by PardosaFileEventStore's
// type bound (`E: pardosa_encoding::Decode`). Hand-rolled (not
// `#[derive]`-able by design).
impl pardosa_encoding::Decode for TestEvent {
    fn decode(
        d: &mut pardosa_encoding::Decoder<'_>,
    ) -> Result<Self, pardosa_encoding::EventError> {
        let tag = <u8 as pardosa_encoding::Decode>::decode(d)?;
        match tag {
            0 => {
                let n = <i64 as pardosa_encoding::Decode>::decode(d)?;
                Ok(Self::Incremented(n))
            }
            _ => Err(pardosa_encoding::EventError::InvalidInput),
        }
    }
}

/// Full `EventStore` conformance against `PardosaFileEventStore`.
///
/// Six harness scenarios (see
/// `cherry_pit_core::testing::conformance::assert_event_store_conformance`).
/// Each scenario gets its own fresh `tempfile::TempDir` so the
/// `.lock` advisory flock does not collide between scenarios.
#[test]
fn pardosa_file_event_store_conforms() {
    let retainer: Arc<Mutex<Vec<TempDir>>> = Arc::new(Mutex::new(Vec::new()));
    let retainer_for_factory = Arc::clone(&retainer);
    let factory = move || {
        let dir = tempfile::tempdir().expect("create tempdir for conformance scenario");
        let store = PardosaFileEventStore::<TestEvent>::open(dir.path())
            .expect("open succeeds on fresh tempdir");
        retainer_for_factory
            .lock()
            .expect("retainer mutex poisoned")
            .push(dir);
        store
    };
    let make_event = |i: u32| TestEvent::Incremented(i64::from(i) + 1);
    block_on(assert_event_store_conformance::<
        PardosaFileEventStore<TestEvent>,
        _,
        _,
    >(factory, make_event));
    // Drop retainer last so stores release flocks before TempDirs reap.
    drop(retainer);
}
