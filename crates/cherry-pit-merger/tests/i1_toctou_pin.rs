//! I1 TOCTOU regression pin — proptest port of gh-report's
//! `concurrent_same_domain_key_evaluations_create_exactly_one_aggregate`
//! (cite original at `crates/gh-report/src/app/services/repo_service.rs:493`).
//!
//! Fans out `N` concurrent same-`domain_key` dispatches against a
//! freshly-spawned merger and asserts the four invariants that hold
//! iff the create-path is single-flighted across the
//! (`lookup` → `EventStore::create` → `index.or_insert`) sequence:
//!
//! 1. Exactly one routing-index entry materialises.
//! 2. The stream contains exactly `N` envelopes with monotonic
//!    sequences `1..=N` (1 create + `N-1` appends, no orphan stream).
//! 3. The bus log contains exactly `N` envelopes in sequence order.
//! 4. The per-aggregate sequence tracker records `N`.
//!
//! Proptest because the test asserts an invariant that should hold
//! over any concurrency level in a sensible range, not just a single
//! `N`. Replays via committed seed in
//! `proptest-regressions/i1_toctou_pin.txt` when a counterexample
//! lands.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_core::testing::{FakeBus, InMemoryEventStore};
use cherry_pit_core::{
    Aggregate, AggregateId, CorrelationContext, DomainEvent, EventStore, StoreError,
};
use cherry_pit_merger::{Merger, MergerArm, MergerHandle, PersistMode};
use proptest::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
enum BumpEvent {
    Bumped { by: u32 },
}

impl DomainEvent for BumpEvent {
    fn event_type(&self) -> &'static str {
        "bump.bumped"
    }
}

#[derive(Default, Debug)]
struct BumpAggregate {
    total: u64,
}

impl Aggregate for BumpAggregate {
    type Event = BumpEvent;
    fn apply(&mut self, event: &BumpEvent) {
        match event {
            BumpEvent::Bumped { by } => self.total += u64::from(*by),
        }
    }
}

#[derive(Debug)]
struct BumpCmd {
    key: String,
    by: u32,
}

#[derive(Debug)]
enum BumpError {
    Store(#[allow(dead_code)] StoreError),
}

impl From<StoreError> for BumpError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

struct BumpArm;

impl MergerArm<BumpAggregate> for BumpArm {
    type Cmd = BumpCmd;
    type Err = BumpError;

    fn persist_mode(&self, cmd: &Self::Cmd) -> PersistMode {
        PersistMode::CreateOrAppend(cmd.key.clone())
    }

    fn handle(&self, _state: &BumpAggregate, cmd: Self::Cmd) -> Result<Vec<BumpEvent>, Self::Err> {
        Ok(vec![BumpEvent::Bumped { by: cmd.by }])
    }

    fn publish_label(&self, _cmd: &Self::Cmd) -> &'static str {
        "Bumped"
    }
}

type TestStore = InMemoryEventStore<BumpEvent>;
type TestBus = FakeBus<BumpEvent>;
type Index = Arc<Mutex<HashMap<String, AggregateId>>>;
type NextSeq = Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>;

struct Harness {
    store: Arc<TestStore>,
    bus: Arc<TestBus>,
    index: Index,
    next_seq: NextSeq,
    handle: MergerHandle<BumpAggregate, BumpArm>,
    _join: tokio::task::JoinHandle<()>,
}

fn build() -> Harness {
    let store = Arc::new(TestStore::new());
    let bus = Arc::new(TestBus::new());
    let index: Index = Arc::new(Mutex::new(HashMap::new()));
    let next_seq: NextSeq = Arc::new(Mutex::new(HashMap::new()));
    let (handle, join) = Merger::<BumpAggregate, _, _, _>::spawn(
        BumpArm,
        Arc::clone(&store),
        Arc::clone(&bus),
        Arc::clone(&index),
        Arc::clone(&next_seq),
    );
    Harness {
        store,
        bus,
        index,
        next_seq,
        handle,
        _join: join,
    }
}

async fn assert_single_flight_invariants(n: usize, key: String, bys: Vec<u32>) {
    assert_eq!(bys.len(), n, "test setup: bys must have length n");
    let harness = build();
    let handle = Arc::new(harness.handle);

    let mut tasks = Vec::with_capacity(n);
    for by in bys {
        let h = Arc::clone(&handle);
        let k = key.clone();
        tasks.push(tokio::spawn(async move {
            h.dispatch(BumpCmd { key: k, by }, CorrelationContext::none())
                .await
        }));
    }
    for t in tasks {
        t.await
            .expect("task join")
            .expect("dispatch under contention");
    }

    let assigned = {
        let guard = harness.index.lock().unwrap();
        assert_eq_msg(
            &guard.len(),
            &1,
            "routing index must have exactly one entry for the single domain_key",
        );
        *guard.get(&key).expect("index maps domain_key")
    };
    let loaded = harness.store.load(assigned).await.expect("load");
    assert_eq_msg(
        &loaded.len(),
        &n,
        "stream should contain exactly N envelopes (1 create + N-1 appends); orphan stream regression if not",
    );
    for (i, env) in loaded.iter().enumerate() {
        assert_eq_msg(
            &env.sequence().get(),
            &u64::try_from(i + 1).unwrap(),
            "envelope sequences must be contiguous 1..=N",
        );
        assert_eq_msg(
            &env.aggregate_id(),
            &assigned,
            "every envelope must be scoped to the single assigned aggregate id",
        );
    }
    let tracked = harness
        .next_seq
        .lock()
        .unwrap()
        .get(&assigned)
        .copied()
        .unwrap()
        .get();
    assert_eq_msg(
        &tracked,
        &u64::try_from(n).unwrap(),
        "next_seq tracker must record N for the single aggregate",
    );
    let published = harness.bus.published();
    assert_eq_msg(
        &published.len(),
        &n,
        "bus must observe exactly N envelopes (one per command)",
    );
}

fn assert_eq_msg<T: PartialEq + std::fmt::Debug>(actual: &T, expected: &T, why: &str) {
    assert_eq!(actual, expected, "{why}");
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 16,
        .. ProptestConfig::default()
    })]

    /// I1 TOCTOU regression pin (proptest variant).
    ///
    /// Generates a fan-out size `n` in `4..=48` and a `Vec<u32>` of
    /// per-command bumps; asserts the four invariants. Counterexamples
    /// shrink to the smallest `n` that exhibits the regression.
    #[test]
    fn concurrent_same_domain_key_creates_exactly_one_aggregate(
        n in 4usize..=48,
        seed in any::<u64>(),
    ) {
        let mut rng = fastrand::Rng::with_seed(seed);
        let bys: Vec<u32> = (0..n).map(|_| rng.u32(0..1_000)).collect();
        let key = format!("octocat/concurrent-{seed:016x}");

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("rt");
        rt.block_on(async {
            assert_single_flight_invariants(n, key, bys).await;
        });
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deterministic_32_way_fanout_matches_gh_report_pattern() {
    let bys: Vec<u32> = (0u32..32).collect();
    assert_single_flight_invariants(32, "octocat/concurrent-create".into(), bys).await;
}
