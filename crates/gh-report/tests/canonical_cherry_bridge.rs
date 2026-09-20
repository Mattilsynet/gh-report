use cherry_pit_core::{CorrelationContext, EventStore};
use pardosa_cherry_pit_test_support::{PgnoEventStore, fixture::RecordedEvent};

#[tokio::test]
async fn canonical_bridge_preserves_core_identity_across_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("canonical.pgno");
    let (id, event_id) = {
        let store = PgnoEventStore::<RecordedEvent>::create_pgno(&path).unwrap();
        let (id, envelopes) = store
            .create(
                vec![RecordedEvent::Recorded { value: 42 }],
                CorrelationContext::none(),
            )
            .await
            .unwrap();
        (id, envelopes[0].event_id())
    };
    let reopened = PgnoEventStore::<RecordedEvent>::open_pgno(&path).unwrap();
    let events = reopened.load(id).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_id(), event_id);
    assert_eq!(events[0].payload(), &RecordedEvent::Recorded { value: 42 });
}
