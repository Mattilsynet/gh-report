//! Registrant: `PardosaEventStore` against the SM-4 conformance harness.
//!
//! This is the load-bearing exit gate for the cherry-pit-pardosa
//! package (SM-6, mission bead adr-fmt-gh1x). The harness lives in
//! `cherry_pit_core::testing::conformance` and is adapter-agnostic;
//! this file exercises only its public surface.
//!
//! ## 6 `EventStore` scenarios (all driven by `assert_event_store_conformance`)
//!
//! Per `cherry_pit_core::testing::conformance`, the 6 invariant
//! scenarios are run on isolated fresh stores from `make_store`:
//!
//! 1. create → load round-trips with contiguous sequences 1..N.
//! 2. stale `expected_sequence` on `append` yields `ConcurrencyError`.
//! 3. `load` of unknown `AggregateId` returns `Ok(empty)` (CHE-0019:R1).
//! 4. empty-events `create` returns Infrastructure error.
//! 5. `append` to a phantom (never-created) aggregate returns
//!    Infrastructure error.
//! 6. create then append produces a monotone, contiguous stream.
//!
//! Substrate notes (PardosaEventStore-specific):
//!
//! - Each scenario receives a fresh `PardosaEventStore::new()` via the
//!   factory closure, so isolation is trivial: no shared Dragline.
//! - Pardosa's underlying `Dragline` stores cherry-pit `EventEnvelope`s
//!   as the pardosa payload; sequence/event-id are preserved at the
//!   envelope level (PAR-0001 + CHE-0042:R4 honoured per-incarnation).
//! - Concurrency-violation detection in `append` rides on pardosa's
//!   stream-tip check inside `state.lock()` — single-writer per
//!   CHE-0061 (zero-method marker trait).
//!
//! ## Why no `#[tokio::test]`
//!
//! Mirrors `crates/cherry-pit-core/tests/conformance_in_memory.rs`:
//! cherry-pit-core has zero async-runtime dep (CHE-0029:R4 +
//! CHE-0018:R3) and the conformance fns return RPITIT futures.
//! We drive them with the same hand-rolled `block_on`. tokio is a
//! dev-dep for unrelated future smoke work; this registrant doesn't
//! reach for it.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use cherry_pit_core::testing::conformance::{
    assert_event_store_conformance, assert_projection_conformance,
};
use cherry_pit_core::{DomainEvent, EventEnvelope, Projection};
use cherry_pit_pardosa::PardosaEventStore;
use serde::{Deserialize, Serialize};

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

// ── Test event ─────────────────────────────────────────────────────
//
// Mirrors `conformance_in_memory.rs::TestEvent` — single Incremented
// variant is sufficient to drive all 6 EventStore scenarios.

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

// ── Projection (drives projection-over-pardosa-store registrant) ───

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

// ── Registrant tests ───────────────────────────────────────────────

/// Load-bearing exit gate for SM-6: all 6 `EventStore` conformance
/// scenarios run against `PardosaEventStore`. Failure here = the
/// pardosa adapter has a substrate-binding gap. Don't relax the
/// harness; surface to moltke as SURPRISE.
#[test]
fn pardosa_event_store_conforms() {
    let factory = PardosaEventStore::<TestEvent>::new;
    let make_event = |i: u32| TestEvent::Incremented(i64::from(i) + 1);
    block_on(assert_event_store_conformance::<
        PardosaEventStore<TestEvent>,
        _,
        _,
    >(factory, make_event));
}

/// Projection-over-pardosa-store: verifies the harness's projection
/// scenarios run cleanly when the underlying store is pardosa-backed
/// rather than in-memory. The harness drives create + load + apply,
/// asserting projection state equality. Aggregate-conformance is
/// store-independent and already covered in `conformance_in_memory.rs`;
/// no duplication needed here.
#[test]
fn counter_view_projection_conforms_over_pardosa_store() {
    let factory = PardosaEventStore::<TestEvent>::new;
    let make_event = |i: u32| TestEvent::Incremented(i64::from(i) + 1);
    block_on(assert_projection_conformance::<
        CounterView,
        PardosaEventStore<TestEvent>,
        _,
        _,
        _,
    >(factory, make_event, |a, b| a == b));
}
