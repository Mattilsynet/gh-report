//! Registrant 1: `InMemoryEventStore` against the conformance harness.
//!
//! First registrant of three for SM-4 (gateway file-store and projection
//! file-store land alongside). Pardosa-backed store is the eventual
//! fourth registrant (Track 2.2). The harness shape must remain
//! adapter-agnostic — this test exercises only the public
//! `cherry_pit_core::testing` surface.
//!
//! ## Why no `#[tokio::test]`
//!
//! `cherry-pit-core` has zero async-runtime dependency (CHE-0029:R4 +
//! CHE-0018:R3). The conformance fns return RPITIT futures; we drive
//! them with the same hand-rolled `block_on` pattern SM-3 introduced
//! for the in-module smoke tests (`testing.rs` inner-tests block).
//! ~30 LOC of `std::task::Waker` boilerplate, zero deps. Oracle §1.3
//! option (b) explicitly permits this approach.
//!
//! ## Aggregate / Projection registrants
//!
//! `assert_aggregate_conformance` and `assert_projection_conformance`
//! are exercised here against a minimal in-test `Counter` aggregate
//! and `CounterView` projection. The harness shape mirrors the file-
//! store and (eventual) pardosa registrants — only the impl types
//! differ.

use std::future::Future;
use std::num::NonZeroU64;
use std::task::{Context, Poll, Waker};

use cherry_pit_core::testing::InMemoryEventStore;
use cherry_pit_core::testing::conformance::{
    assert_aggregate_conformance, assert_event_store_conformance, assert_projection_conformance,
};
use cherry_pit_core::{Aggregate, DomainEvent, EventEnvelope, Projection};
use serde::{Deserialize, Serialize};

fn block_on<F: Future>(fut: F) -> F::Output {
    let mut cx = Context::from_waker(Waker::noop());
    let mut fut = std::pin::pin!(fut);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

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
struct Counter {
    value: i64,
}

impl Aggregate for Counter {
    type Event = TestEvent;
    fn apply(&mut self, event: &TestEvent) {
        match event {
            TestEvent::Incremented(n) => self.value += n,
        }
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

#[test]
fn in_memory_event_store_conforms() {
    let factory = || InMemoryEventStore::<TestEvent>::new();
    let make_event = |i: u32| TestEvent::Incremented(i64::from(i) + 1);
    block_on(assert_event_store_conformance::<
        InMemoryEventStore<TestEvent>,
        _,
        _,
    >(factory, make_event));
}

#[test]
fn counter_aggregate_conforms() {
    let events = vec![
        TestEvent::Incremented(1),
        TestEvent::Incremented(2),
        TestEvent::Incremented(3),
    ];
    assert_aggregate_conformance::<Counter>(&events, |c| c.value == 6);
}

#[test]
fn counter_view_projection_conforms_over_in_memory_store() {
    let factory = || InMemoryEventStore::<TestEvent>::new();
    let make_event = |i: u32| TestEvent::Incremented(i64::from(i) + 1);
    block_on(assert_projection_conformance::<
        CounterView,
        InMemoryEventStore<TestEvent>,
        _,
        _,
        _,
    >(factory, make_event, |a, b| a == b));
}

#[test]
#[should_panic(expected = "A::default() must NOT satisfy")]
fn aggregate_harness_rejects_trivial_probe() {
    assert_aggregate_conformance::<Counter>(&[TestEvent::Incremented(1)], |_| true);
}

#[test]
#[should_panic(expected = "requires ≥1 event")]
fn aggregate_harness_rejects_empty_events() {
    assert_aggregate_conformance::<Counter>(&[], |_| true);
}

#[test]
fn stale_sequence_constant_is_sane() {
    let real_last = NonZeroU64::new(1).unwrap();
    let stale = NonZeroU64::new(real_last.get().saturating_add(99)).unwrap();
    assert_eq!(stale.get(), 100);
}
