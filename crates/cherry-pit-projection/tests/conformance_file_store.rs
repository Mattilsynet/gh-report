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
//! `FileProjectionStore` (this crate's own persistence backend) is
//! **not** exercised here — separate from `Projection::apply`; a
//! commutativity/dedup-under-resume harness for it is future work,
//! out of SM-4 scope.

use cherry_pit_core::testing::conformance::assert_projection_conformance;
use cherry_pit_core::{EventEnvelope, Projection};
use pardosa_cherry_pit_test_support::PgnoEventStore;
use pardosa_cherry_pit_test_support::fixture::RecordedEvent;

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
