//! Registrant 3: a [`Projection`] impl exercised against a pgno-backed
//! [`EventStore`] via [`assert_projection_conformance`].
//!
//! Third of three SM-4 registrants. The harness probes the
//! [`Projection`] trait contract (CHE-0048:R3 replay-equivalence,
//! fold determinism). The backing [`EventStore`] is the L2a bridge
//! crate's [`PgnoEventStore`], so replay is exercised over envelopes
//! that round-trip through a real on-disk pardosa fiber container
//! rather than an in-process `Vec` (CHE-0100 R3).
//!
//! Pairing with `PgnoEventStore` (rather than `InMemoryEventStore`,
//! which would also satisfy the harness signature) proves the fold is
//! stable across the serde boundary, per SM-4 SC#10 ("registrants must
//! exercise a non-trivial adapter pairing").
//!
//! [`PardosaProjectionStore`] (this crate's PERSISTENT backend,
//! CHE-0048:R1/R10) is exercised separately below: persist/load/delete
//! round-trip and snapshot-then-checkpoint write ordering, against a
//! real `.pgno` file — the commutativity/dedup-under-resume coverage
//! this file's doc comment previously flagged as future work.

use std::num::NonZeroU64;

use cherry_pit_core::testing::conformance::assert_projection_conformance;
use cherry_pit_core::{AggregateId, CorrelationContext, EventEnvelope, EventStore, Projection};
use cherry_pit_projection::PardosaProjectionStore;
use pardosa_cherry_pit_test_support::PgnoEventStore;
use pardosa_cherry_pit_test_support::fixture::RecordedEvent;
use serde::{Deserialize, Serialize};

fn temp_pgno_path() -> tempfile::TempPath {
    let file = tempfile::NamedTempFile::new().expect("create temp file");
    let path = file.into_temp_path();
    std::fs::remove_file(&path).expect("clear placeholder so create_pgno starts fresh");
    path
}

fn recorded(value: u32) -> RecordedEvent {
    RecordedEvent::Recorded { value }
}

/// Tally projection: sums recorded values and tracks how many
/// envelopes have been folded in. Both fields move monotonically
/// away from `Default`, so replay equivalence is observable.
#[derive(Default, Debug, PartialEq)]
struct Tally {
    total: u64,
    applied: u64,
}

impl Projection for Tally {
    type Event = RecordedEvent;
    fn apply(&mut self, env: &EventEnvelope<RecordedEvent>) {
        let RecordedEvent::Recorded { value } = env.payload();
        self.total += u64::from(*value);
        self.applied += 1;
    }
}

#[tokio::test]
async fn tally_projection_conforms_over_pgno_store() {
    let factory = || {
        let path = temp_pgno_path();
        PgnoEventStore::<RecordedEvent>::create_pgno(&path).expect("create pgno store")
    };
    let make_event = |i: u32| recorded(i + 1);

    assert_projection_conformance::<Tally, PgnoEventStore<RecordedEvent>, _, _, _>(
        factory,
        make_event,
        |a, b| a == b,
    )
    .await;
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TallySnapshot {
    total: u64,
}

fn aggregate_id(value: u64) -> AggregateId {
    AggregateId::new(NonZeroU64::new(value).expect("non-zero id"))
}

fn seq(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).expect("non-zero sequence")
}

#[tokio::test]
async fn pardosa_projection_store_persist_load_delete_round_trip() {
    let path = temp_pgno_path();
    let store =
        PardosaProjectionStore::<TallySnapshot>::create_pgno(&path, "tally_view").expect("create");
    let id = aggregate_id(1);

    store
        .persist(id, &TallySnapshot { total: 3 }, seq(3))
        .await
        .expect("persist succeeds");

    assert_eq!(
        store.load_snapshot(id).await.expect("load snapshot"),
        Some(TallySnapshot { total: 3 })
    );
    assert_eq!(
        store
            .load_checkpoint(id)
            .await
            .expect("load checkpoint")
            .expect("checkpoint exists")
            .last_sequence(),
        seq(3)
    );

    store.delete(id).await.expect("delete succeeds");

    assert_eq!(store.load_snapshot(id).await.expect("load"), None);
    assert_eq!(store.load_checkpoint(id).await.expect("load"), None);
}

#[tokio::test]
async fn pardosa_projection_store_persist_is_snapshot_then_checkpoint_ordered() {
    let path = temp_pgno_path();
    let store =
        PardosaProjectionStore::<TallySnapshot>::create_pgno(&path, "tally_view").expect("create");
    let id = aggregate_id(1);

    store
        .persist(id, &TallySnapshot { total: 5 }, seq(5))
        .await
        .expect("first persist succeeds");
    store
        .persist(id, &TallySnapshot { total: 8 }, seq(8))
        .await
        .expect("second persist succeeds");

    assert_eq!(
        store.load_snapshot(id).await.expect("load"),
        Some(TallySnapshot { total: 8 }),
        "latest snapshot wins on repeated persist"
    );
    assert_eq!(
        store
            .load_checkpoint(id)
            .await
            .expect("load")
            .expect("checkpoint exists")
            .last_sequence(),
        seq(8),
        "latest checkpoint tracks the latest persisted sequence"
    );

    let regression = store.persist(id, &TallySnapshot { total: 1 }, seq(2)).await;
    assert!(
        regression.is_err(),
        "CHE-0097:R1 monotonicity: persist below the existing checkpoint must be rejected"
    );
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Trace {
    seen: Vec<u32>,
}

impl Projection for Trace {
    type Event = RecordedEvent;
    fn apply(&mut self, env: &EventEnvelope<RecordedEvent>) {
        let RecordedEvent::Recorded { value } = env.payload();
        self.seen.push(*value);
    }
}

#[derive(Default, Debug, PartialEq)]
struct NoOpProjection;

impl Projection for NoOpProjection {
    type Event = RecordedEvent;
    fn apply(&mut self, _env: &EventEnvelope<RecordedEvent>) {}
}

async fn recorded_envelopes(values: &[u32]) -> Vec<EventEnvelope<RecordedEvent>> {
    let path = temp_pgno_path();
    let store = PgnoEventStore::<RecordedEvent>::create_pgno(&path).expect("create pgno store");
    let (first, rest) = values.split_first().expect("at least one event");
    let (id, created) = store
        .create(vec![recorded(*first)], CorrelationContext::none())
        .await
        .expect("create must succeed");
    let mut last_seq = created.last().expect("at least one envelope").sequence();
    for value in rest {
        let appended = store
            .append(
                id,
                last_seq,
                vec![recorded(*value)],
                CorrelationContext::none(),
            )
            .await
            .expect("append with correct expected_sequence must succeed");
        last_seq = appended.last().expect("at least one envelope").sequence();
    }
    store.load(id).await.expect("load must succeed for replay")
}

#[tokio::test]
async fn equality_only_conformance_is_satisfied_by_a_no_op_fold() {
    let factory = || {
        let path = temp_pgno_path();
        PgnoEventStore::<RecordedEvent>::create_pgno(&path).expect("create pgno store")
    };

    assert_projection_conformance::<NoOpProjection, PgnoEventStore<RecordedEvent>, _, _, _>(
        factory,
        |i: u32| recorded(i + 1),
        |a, b| a == b,
    )
    .await;
}

#[tokio::test]
async fn tally_fold_matches_independently_computed_expected_state() {
    let values = [1u32, 2, 3];
    let envs = recorded_envelopes(&values).await;
    assert_eq!(envs.len(), values.len(), "loaded stream must be complete");

    let mut tally = Tally::default();
    for env in &envs {
        tally.apply(env);
    }

    assert_eq!(
        tally,
        Tally {
            total: 6,
            applied: 3
        },
        "fold must reach the state expected from the fixture itself, not merely \
         agree with a second fold of the same events"
    );
}

#[tokio::test]
async fn envelope_order_is_load_ordered_and_observable_by_an_order_sensitive_fold() {
    let envs = recorded_envelopes(&[1, 2, 3]).await;

    let sequences: Vec<NonZeroU64> = envs.iter().map(EventEnvelope::sequence).collect();
    let mut ascending = sequences.clone();
    ascending.sort_unstable();
    assert_eq!(
        sequences, ascending,
        "the pgno-backed store must load envelopes in ascending sequence order"
    );

    let mut forward = Trace::default();
    for env in &envs {
        forward.apply(env);
    }
    let mut reversed = Trace::default();
    for env in envs.iter().rev() {
        reversed.apply(env);
    }

    assert_eq!(
        forward,
        Trace {
            seen: vec![1, 2, 3]
        },
        "forward fold must observe the recorded order"
    );
    assert_ne!(
        forward, reversed,
        "ordering obligation: an order-sensitive projection must distinguish a \
         permuted stream, which the order-insensitive Tally sum cannot"
    );
}

#[tokio::test]
async fn checkpoint_resume_restores_persisted_state_and_equals_a_full_fold() {
    let envs = recorded_envelopes(&[1, 2, 3]).await;
    let path = temp_pgno_path();
    let id = aggregate_id(1);

    let mut full = Trace::default();
    for env in &envs {
        full.apply(env);
    }

    let mut partial = Trace::default();
    let mut checkpoint_seq = None;
    {
        let writer =
            PardosaProjectionStore::<Trace>::create_pgno(&path, "trace_view").expect("create");
        for env in envs.iter().take(2) {
            partial.apply(env);
            checkpoint_seq = Some(env.sequence());
        }
        writer
            .persist(
                id,
                &partial,
                checkpoint_seq.expect("two envelopes folded before the crash point"),
            )
            .await
            .expect("persist snapshot and checkpoint");
    }
    let checkpoint_seq = checkpoint_seq.expect("two envelopes folded before the crash point");
    drop(partial);

    let reopened = PardosaProjectionStore::<Trace>::open_pgno(&path, "trace_view").expect("reopen");
    let mut resumed = reopened
        .load_snapshot(id)
        .await
        .expect("load snapshot")
        .expect("snapshot exists");
    let resume_from = reopened
        .load_checkpoint(id)
        .await
        .expect("load checkpoint")
        .expect("checkpoint exists")
        .last_sequence();
    assert_eq!(
        resume_from, checkpoint_seq,
        "resume must restart from the persisted checkpoint, not from zero"
    );
    assert_eq!(
        resumed,
        Trace { seen: vec![1, 2] },
        "the restored state must be the complete persisted prefix, independently of \
         what the pre-crash writer held in memory"
    );

    let mut replayed = 0usize;
    for env in envs.iter().filter(|env| env.sequence() > resume_from) {
        resumed.apply(env);
        replayed += 1;
    }
    assert_eq!(
        replayed, 1,
        "exactly the post-checkpoint tail must be replayed"
    );
    assert_eq!(
        resumed,
        Trace {
            seen: vec![1, 2, 3]
        },
        "checkpoint/resume obligation: a process that crashed after the checkpoint and \
         reopened the store must reach the independently expected final state"
    );
    assert_eq!(
        resumed, full,
        "restored-then-resumed state must equal an uninterrupted fold"
    );

    let mut naive = Trace::default();
    for env in envs.iter().take(2) {
        naive.apply(env);
    }
    for env in &envs {
        naive.apply(env);
    }
    assert_ne!(
        naive, full,
        "dedup-under-resume fault must be detectable: ignoring the checkpoint and \
         replaying the whole stream double-folds the checkpointed prefix"
    );
}
