//! Tokio integration test for `InProcessEventBus` per WU-5 S3 + CHE-0051:R2.
//!
//! Spawns N concurrent publish tasks against one bus instance and asserts
//! every envelope reaches every registered handler exactly once. Exercises
//! the synchronous-fanout contract (CHE-0024:§7) under a multi-threaded
//! tokio runtime.

use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_agent::InProcessEventBus;
use cherry_pit_core::{AggregateId, DomainEvent, EventBus, EventEnvelope};
use serde::{Deserialize, Serialize};

const N: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum E {
    N(u64),
}

impl DomainEvent for E {
    fn event_type(&self) -> &'static str {
        "test.n"
    }
}

fn env(n: u64) -> EventEnvelope<E> {
    EventEnvelope::new(
        uuid::Uuid::now_v7(),
        AggregateId::new(NonZeroU64::new(1).unwrap()),
        NonZeroU64::new(n).unwrap(),
        jiff::Timestamp::now(),
        None,
        None,
        E::N(n),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fanout_under_concurrent_publishes() {
    let bus: Arc<InProcessEventBus<E>> = Arc::new(InProcessEventBus::new());
    let received: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let received_c = Arc::clone(&received);
    bus.register(move |envelope: &EventEnvelope<E>| {
        let E::N(n) = envelope.payload();
        received_c.lock().unwrap().push(*n);
    });

    let mut handles = Vec::with_capacity(N);
    for i in 1..=N {
        let bus_c = Arc::clone(&bus);
        handles.push(tokio::spawn(async move {
            bus_c.publish(&[env(i as u64)]).await.unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    let mut got = received.lock().unwrap().clone();
    got.sort_unstable();
    let expected: Vec<u64> = (1..=N as u64).collect();
    assert_eq!(got, expected, "every published envelope must reach handler");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multi_handler_fanout_under_concurrent_publishes() {
    let bus: Arc<InProcessEventBus<E>> = Arc::new(InProcessEventBus::new());
    let count_a: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    let count_b: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    let a_c = Arc::clone(&count_a);
    let b_c = Arc::clone(&count_b);
    bus.register(move |_| {
        *a_c.lock().unwrap() += 1;
    });
    bus.register(move |_| {
        *b_c.lock().unwrap() += 1;
    });

    let mut handles = Vec::with_capacity(N);
    for i in 1..=N {
        let bus_c = Arc::clone(&bus);
        handles.push(tokio::spawn(async move {
            bus_c.publish(&[env(i as u64)]).await.unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    assert_eq!(*count_a.lock().unwrap(), N as u64);
    assert_eq!(*count_b.lock().unwrap(), N as u64);
}

/// Regression: a handler that re-enters the bus to register another
/// handler must not deadlock. Pre-fix, `InProcessEventBus::publish`
/// held the handler-vector mutex across handler invocation, so a
/// reentrant `register` parked forever waiting on the same lock.
/// Post-fix, `publish` snapshots the handler list and releases the
/// lock before fan-out, so reentrant registration on the same bus is
/// safe. Bounded by a wall-clock timeout to detect regression as a
/// test failure rather than a hung test process.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reentrant_register_from_handler_does_not_deadlock() {
    let bus: Arc<InProcessEventBus<E>> = Arc::new(InProcessEventBus::new());
    let bus_for_handler = Arc::clone(&bus);
    let nested_calls: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    let nested_c = Arc::clone(&nested_calls);
    bus.register(move |_envelope: &EventEnvelope<E>| {
        let inner = Arc::clone(&nested_c);
        bus_for_handler.register(move |_env: &EventEnvelope<E>| {
            *inner.lock().unwrap() += 1;
        });
    });

    let bus_c = Arc::clone(&bus);
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), async move {
        bus_c.publish(&[env(1)]).await.unwrap();
    })
    .await;
    assert!(
        result.is_ok(),
        "reentrant register from handler deadlocked"
    );

    let count_before = bus.handler_count();
    assert!(
        count_before >= 2,
        "expected at least one newly-registered handler post-publish; got {count_before}"
    );
}

/// Regression: a handler that re-enters the bus to publish another
/// envelope on the same instance must not deadlock. Same rationale as
/// `reentrant_register_from_handler_does_not_deadlock`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reentrant_publish_from_handler_does_not_deadlock() {
    let bus: Arc<InProcessEventBus<E>> = Arc::new(InProcessEventBus::new());
    let bus_for_handler = Arc::clone(&bus);
    let observed: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let observed_c = Arc::clone(&observed);
    let fired: Arc<std::sync::atomic::AtomicBool> =
        Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fired_c = Arc::clone(&fired);
    bus.register(move |envelope: &EventEnvelope<E>| {
        let E::N(n) = envelope.payload();
        observed_c.lock().unwrap().push(*n);
        if !fired_c.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let bus_inner = Arc::clone(&bus_for_handler);
            tokio::task::block_in_place(move || {
                tokio::runtime::Handle::current().block_on(async move {
                    bus_inner.publish(&[env(99)]).await.unwrap();
                });
            });
        }
    });

    let bus_c = Arc::clone(&bus);
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), async move {
        bus_c.publish(&[env(1)]).await.unwrap();
    })
    .await;
    assert!(
        result.is_ok(),
        "reentrant publish from handler deadlocked"
    );

    let got = observed.lock().unwrap().clone();
    assert!(
        got.contains(&1) && got.contains(&99),
        "both outer (1) and reentrant (99) envelopes must reach handler; got {got:?}"
    );
}
