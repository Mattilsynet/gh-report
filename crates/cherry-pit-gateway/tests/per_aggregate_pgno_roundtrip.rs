use std::num::NonZeroU64;

use cherry_pit_core::{CorrelationContext, EventStore};
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

#[tokio::test]
async fn per_aggregate_pgno_roundtrip_preserves_sequence_and_payload() {
    let path = temp_pgno_path();

    let (id_a, created_a) = {
        let store = PgnoEventStore::<RecordedEvent>::create_pgno(&path).expect("create store");

        let (id_a, created_a) = store
            .create(vec![recorded(1)], CorrelationContext::none())
            .await
            .expect("create succeeds on fresh pgno store");
        assert_eq!(created_a.len(), 1, "create returns one envelope");

        let appended_1 = store
            .append(
                id_a,
                NonZeroU64::new(1).unwrap(),
                vec![recorded(10)],
                CorrelationContext::none(),
            )
            .await
            .expect("first single-event append succeeds after create");
        assert_eq!(
            appended_1.len(),
            1,
            "single-event append returns one envelope"
        );

        let appended_2 = store
            .append(
                id_a,
                NonZeroU64::new(2).unwrap(),
                vec![recorded(20)],
                CorrelationContext::none(),
            )
            .await
            .expect("second single-event append succeeds");
        assert_eq!(
            appended_2.len(),
            1,
            "single-event append returns one envelope"
        );

        let (id_b, _created_b) = store
            .create(vec![recorded(99)], CorrelationContext::none())
            .await
            .expect("create of a second aggregate on the same store succeeds");
        assert_ne!(
            id_a, id_b,
            "each create call must assign a distinct AggregateId"
        );

        (id_a, created_a)
    };

    let reopened = PgnoEventStore::<RecordedEvent>::open_pgno(&path).expect("reopen pgno store");

    let loaded_a = reopened
        .load(id_a)
        .await
        .expect("load of aggregate A from a fresh store instance succeeds");
    assert_eq!(
        loaded_a.len(),
        3,
        "all three of aggregate A's envelopes survive the persistence boundary"
    );
    assert_eq!(loaded_a[0].sequence().get(), 1);
    assert_eq!(loaded_a[1].sequence().get(), 2);
    assert_eq!(loaded_a[2].sequence().get(), 3);
    assert_eq!(loaded_a[0].aggregate_id(), id_a);
    assert_eq!(loaded_a[1].aggregate_id(), id_a);
    assert_eq!(loaded_a[2].aggregate_id(), id_a);
    assert_eq!(*loaded_a[0].payload(), recorded(1));
    assert_eq!(*loaded_a[1].payload(), recorded(10));
    assert_eq!(*loaded_a[2].payload(), recorded(20));
    assert_eq!(loaded_a[0].event_id(), created_a[0].event_id());
    assert_eq!(loaded_a[0].timestamp(), created_a[0].timestamp());

    let (id_b, _created_b) = reopened
        .create(vec![recorded(99)], CorrelationContext::none())
        .await
        .expect("create after reopen assigns a fresh id");
    let loaded_b = reopened
        .load(id_b)
        .await
        .expect("load of a fresh third aggregate succeeds");
    assert_eq!(
        loaded_b.len(),
        1,
        "aggregate B's own stream isolates from aggregate A: no bleed-through"
    );
    assert!(
        loaded_b.iter().all(|env| env.aggregate_id() == id_b),
        "every envelope loaded for aggregate B must be scoped to id_b, not id_a"
    );
}
