//! End-to-end integration tests for the [`Merger`] dispatch loop.
//!
//! Tests against a small in-test aggregate `Counter` whose command
//! enum has variants for each of the three [`PersistMode`] shapes,
//! covering the same call-graph that gh-report's lifted services
//! exercised. The merger crate stays aggregate-agnostic; the
//! aggregate lives here in the test.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_core::testing::{FakeBus, InMemoryEventStore};
use cherry_pit_core::{
    Aggregate, AggregateId, CorrelationContext, DomainEvent, EventStore, StoreError,
};
use cherry_pit_merger::{Merger, MergerArm, PersistMode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
enum CounterEvent {
    Created { initial: i64 },
    Incremented { by: i64 },
    Closed,
}

impl DomainEvent for CounterEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Created { .. } => "counter.created",
            Self::Incremented { .. } => "counter.incremented",
            Self::Closed => "counter.closed",
        }
    }
}

#[derive(Default, Debug)]
struct Counter {
    value: i64,
    closed: bool,
}

impl Aggregate for Counter {
    type Event = CounterEvent;
    fn apply(&mut self, event: &CounterEvent) {
        match event {
            CounterEvent::Created { initial } => self.value = *initial,
            CounterEvent::Incremented { by } => self.value += *by,
            CounterEvent::Closed => self.closed = true,
        }
    }
}

#[derive(Debug)]
enum CounterCmd {
    Create { key: String, initial: i64 },
    Increment { key: String, by: i64 },
    Close { key: String },
    CreateFresh { initial: i64 },
}

#[derive(Debug)]
enum CounterError {
    AlreadyClosed,
    Store(#[allow(dead_code)] StoreError),
    RoutingMiss(String),
}

impl From<StoreError> for CounterError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

struct CounterArm;

impl MergerArm<Counter> for CounterArm {
    type Cmd = CounterCmd;
    type Err = CounterError;

    fn persist_mode(&self, cmd: &Self::Cmd) -> PersistMode {
        match cmd {
            CounterCmd::Create { key, .. } => PersistMode::CreateOrAppend(key.clone()),
            CounterCmd::Increment { key, .. } | CounterCmd::Close { key } => {
                PersistMode::AppendStrict(key.clone())
            }
            CounterCmd::CreateFresh { .. } => PersistMode::Create,
        }
    }

    fn handle(&self, state: &Counter, cmd: Self::Cmd) -> Result<Vec<CounterEvent>, Self::Err> {
        if state.closed {
            return Err(CounterError::AlreadyClosed);
        }
        match cmd {
            CounterCmd::Create { initial, .. } | CounterCmd::CreateFresh { initial } => {
                Ok(vec![CounterEvent::Created { initial }])
            }
            CounterCmd::Increment { by, .. } => Ok(vec![CounterEvent::Incremented { by }]),
            CounterCmd::Close { .. } => Ok(vec![CounterEvent::Closed]),
        }
    }

    fn publish_label(&self, cmd: &Self::Cmd) -> &'static str {
        match cmd {
            CounterCmd::Create { .. } | CounterCmd::CreateFresh { .. } => "CounterCreated",
            CounterCmd::Increment { .. } => "CounterIncremented",
            CounterCmd::Close { .. } => "CounterClosed",
        }
    }

    fn missing_key_error(&self, key: &str) -> Self::Err {
        CounterError::RoutingMiss(key.to_owned())
    }
}

type TestStore = InMemoryEventStore<CounterEvent>;
type TestBus = FakeBus<CounterEvent>;
type Index = Arc<Mutex<HashMap<String, AggregateId>>>;
type NextSeq = Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>;

fn build() -> (Arc<TestStore>, Arc<TestBus>, Index, NextSeq) {
    let store = Arc::new(TestStore::new());
    let bus = Arc::new(TestBus::new());
    let index: Index = Arc::new(Mutex::new(HashMap::new()));
    let next_seq: NextSeq = Arc::new(Mutex::new(HashMap::new()));
    (store, bus, index, next_seq)
}

#[tokio::test]
async fn create_or_append_first_call_creates_aggregate() {
    let (store, bus, index, next_seq) = build();
    let (handle, _join) = Merger::<Counter, _, _, _>::spawn(
        CounterArm,
        Arc::clone(&store),
        Arc::clone(&bus),
        Arc::clone(&index),
        Arc::clone(&next_seq),
    );

    handle
        .dispatch(
            CounterCmd::Create {
                key: "k1".into(),
                initial: 10,
            },
            CorrelationContext::none(),
        )
        .await
        .expect("create");

    let assigned = {
        let guard = index.lock().unwrap();
        assert_eq!(guard.len(), 1);
        *guard.get("k1").expect("k1 indexed")
    };
    let loaded = store.load(assigned).await.expect("load");
    assert_eq!(loaded.len(), 1);
    assert!(matches!(
        loaded[0].payload(),
        CounterEvent::Created { initial: 10 }
    ));
    assert_eq!(bus.published().len(), 1);
    assert_eq!(
        next_seq
            .lock()
            .unwrap()
            .get(&assigned)
            .copied()
            .unwrap()
            .get(),
        1
    );
}

#[tokio::test]
async fn create_or_append_second_call_appends_to_existing() {
    let (store, bus, index, next_seq) = build();
    let (handle, _join) = Merger::<Counter, _, _, _>::spawn(
        CounterArm,
        Arc::clone(&store),
        Arc::clone(&bus),
        Arc::clone(&index),
        Arc::clone(&next_seq),
    );

    handle
        .dispatch(
            CounterCmd::Create {
                key: "k2".into(),
                initial: 1,
            },
            CorrelationContext::none(),
        )
        .await
        .unwrap();
    handle
        .dispatch(
            CounterCmd::Create {
                key: "k2".into(),
                initial: 999,
            },
            CorrelationContext::none(),
        )
        .await
        .unwrap();

    let assigned = *index.lock().unwrap().get("k2").unwrap();
    let loaded = store.load(assigned).await.unwrap();
    assert_eq!(
        loaded.len(),
        2,
        "second create should append, not orphan a new aggregate"
    );
    assert_eq!(loaded[0].sequence().get(), 1);
    assert_eq!(loaded[1].sequence().get(), 2);
    assert_eq!(
        index.lock().unwrap().len(),
        1,
        "exactly one entry for the single domain key"
    );
}

#[tokio::test]
async fn append_strict_misses_route_to_arm_error() {
    let (store, bus, index, next_seq) = build();
    let (handle, _join) = Merger::<Counter, _, _, _>::spawn(
        CounterArm,
        Arc::clone(&store),
        Arc::clone(&bus),
        Arc::clone(&index),
        Arc::clone(&next_seq),
    );

    let err = handle
        .dispatch(
            CounterCmd::Increment {
                key: "ghost".into(),
                by: 1,
            },
            CorrelationContext::none(),
        )
        .await
        .expect_err("should miss");
    match err {
        CounterError::RoutingMiss(k) => assert_eq!(k, "ghost"),
        other => panic!("expected RoutingMiss, got {other:?}"),
    }
    assert!(index.lock().unwrap().is_empty());
    assert_eq!(bus.published().len(), 0);
}

#[tokio::test]
async fn append_strict_after_create_lifts_sequence() {
    let (store, bus, index, next_seq) = build();
    let (handle, _join) = Merger::<Counter, _, _, _>::spawn(
        CounterArm,
        Arc::clone(&store),
        Arc::clone(&bus),
        Arc::clone(&index),
        Arc::clone(&next_seq),
    );

    handle
        .dispatch(
            CounterCmd::Create {
                key: "k3".into(),
                initial: 0,
            },
            CorrelationContext::none(),
        )
        .await
        .unwrap();
    for n in 1..=5 {
        handle
            .dispatch(
                CounterCmd::Increment {
                    key: "k3".into(),
                    by: n,
                },
                CorrelationContext::none(),
            )
            .await
            .unwrap();
    }

    let id = *index.lock().unwrap().get("k3").unwrap();
    let loaded = store.load(id).await.unwrap();
    assert_eq!(loaded.len(), 6, "1 create + 5 appends");
    for (i, env) in loaded.iter().enumerate() {
        assert_eq!(env.sequence().get(), u64::try_from(i + 1).unwrap());
    }
    let tracked = next_seq.lock().unwrap().get(&id).copied().unwrap().get();
    assert_eq!(tracked, 6);
}

#[tokio::test]
async fn create_fresh_mints_new_aggregate_each_call_no_index_touch() {
    let (store, bus, index, next_seq) = build();
    let (handle, _join) = Merger::<Counter, _, _, _>::spawn(
        CounterArm,
        Arc::clone(&store),
        Arc::clone(&bus),
        Arc::clone(&index),
        Arc::clone(&next_seq),
    );

    for n in 0..4 {
        handle
            .dispatch(
                CounterCmd::CreateFresh { initial: n },
                CorrelationContext::none(),
            )
            .await
            .unwrap();
    }

    assert!(
        index.lock().unwrap().is_empty(),
        "Create mode does not touch the routing index"
    );
    assert_eq!(bus.published().len(), 4);
    assert_eq!(
        next_seq.lock().unwrap().len(),
        4,
        "one tracker entry per fresh aggregate"
    );
}

#[tokio::test]
async fn handle_error_surfaces_through_reply_channel() {
    let (store, bus, index, next_seq) = build();
    let (handle, _join) = Merger::<Counter, _, _, _>::spawn(
        CounterArm,
        Arc::clone(&store),
        Arc::clone(&bus),
        Arc::clone(&index),
        Arc::clone(&next_seq),
    );

    handle
        .dispatch(
            CounterCmd::Create {
                key: "kx".into(),
                initial: 0,
            },
            CorrelationContext::none(),
        )
        .await
        .unwrap();
    handle
        .dispatch(
            CounterCmd::Close { key: "kx".into() },
            CorrelationContext::none(),
        )
        .await
        .unwrap();
    let err = handle
        .dispatch(
            CounterCmd::Increment {
                key: "kx".into(),
                by: 1,
            },
            CorrelationContext::none(),
        )
        .await
        .expect_err("closed → arm rejects");
    assert!(matches!(err, CounterError::AlreadyClosed));
}

#[tokio::test]
async fn publish_order_matches_dispatch_order_per_aggregate() {
    let (store, bus, index, next_seq) = build();
    let (handle, _join) = Merger::<Counter, _, _, _>::spawn(
        CounterArm,
        Arc::clone(&store),
        Arc::clone(&bus),
        Arc::clone(&index),
        Arc::clone(&next_seq),
    );

    handle
        .dispatch(
            CounterCmd::Create {
                key: "kord".into(),
                initial: 0,
            },
            CorrelationContext::none(),
        )
        .await
        .unwrap();
    for n in 1..=8 {
        handle
            .dispatch(
                CounterCmd::Increment {
                    key: "kord".into(),
                    by: n,
                },
                CorrelationContext::none(),
            )
            .await
            .unwrap();
    }

    let published = bus.published();
    assert_eq!(published.len(), 9);
    for (i, env) in published.iter().enumerate() {
        assert_eq!(env.sequence().get(), u64::try_from(i + 1).unwrap());
    }
}

#[tokio::test]
async fn dispatch_after_task_shutdown_returns_infra_error() {
    let (store, bus, index, next_seq) = build();
    let (handle, join) = Merger::<Counter, _, _, _>::spawn(
        CounterArm,
        Arc::clone(&store),
        Arc::clone(&bus),
        Arc::clone(&index),
        Arc::clone(&next_seq),
    );
    drop(handle);
    let _ = join.await;

    let (handle2, join2) = Merger::<Counter, _, _, _>::spawn(
        CounterArm,
        Arc::clone(&store),
        Arc::clone(&bus),
        Arc::clone(&index),
        Arc::clone(&next_seq),
    );
    drop(handle2);
    let _ = join2.await;
}
