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
use cherry_pit_core::{AggregateId, EventEnvelope, Projection};
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

