use cherry_pit_core::{AggregateId, DomainEvent, EventEnvelope};
use cherry_pit_pardosa::PardosaEventStore;
use cherry_pit_pardosa::payload::EnvelopePayload;
use pardosa::store::EventStore as PardosaStore;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum TestEvent {
    Created { domain_key: String, value: u64 },
}

impl DomainEvent for TestEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Created { .. } => "test.created",
        }
    }
}

fn envelope(aggregate_id: u64, sequence: u64, value: u64) -> EventEnvelope<TestEvent> {
    EventEnvelope::new(
        uuid::Uuid::now_v7(),
        AggregateId::new(NonZeroU64::new(aggregate_id).expect("non-zero aggregate id")),
        NonZeroU64::new(sequence).expect("non-zero sequence"),
        jiff::Timestamp::now(),
        None,
        None,
        TestEvent::Created {
            domain_key: format!("repo-{aggregate_id}"),
            value,
        },
    )
    .expect("valid envelope")
}

fn payload(envelope: &EventEnvelope<TestEvent>) -> EnvelopePayload {
    EnvelopePayload::new(
        rmp_serde::to_vec_named(envelope).expect("encode envelope"),
        envelope.aggregate_id().get(),
        format!("repo-{}", envelope.aggregate_id().get()),
    )
    .expect("payload fits bounds")
}

#[test]
fn read_seam_lists_and_loads_logical_streams_after_pgno_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("events.pgno");
    let repo_one_second = envelope(1, 2, 20);
    let repo_two_first = envelope(2, 1, 30);
    let repo_one_first = envelope(1, 1, 10);
    {
        let mut store = PardosaStore::<EnvelopePayload>::create(&path).expect("create pardosa store");
        let _ = store
            .writer()
            .begin(payload(&repo_one_second))
            .expect("begin aggregate 1 sequence 2");
        let _ = store
            .writer()
            .begin(payload(&repo_two_first))
            .expect("begin aggregate 2 sequence 1");
        let _ = store
            .writer()
            .begin(payload(&repo_one_first))
            .expect("begin aggregate 1 sequence 1");
        let _ = store.writer().sync().expect("sync pardosa store");
    }

    let store = PardosaEventStore::<TestEvent>::open_pgno(&path).expect("open adapter over pgno");
    let mut aggregates = store.list_indexed_aggregates().expect("list aggregates");
    aggregates.sort_unstable();
    assert_eq!(aggregates, vec![id(1), id(2)]);

    let loaded = store.load_indexed(id(1)).expect("load aggregate 1");
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].sequence().get(), 1);
    assert_eq!(loaded[1].sequence().get(), 2);
    assert_eq!(loaded[0].payload(), repo_one_first.payload());
    assert_eq!(loaded[1].payload(), repo_one_second.payload());

    let loaded_b = store.load_indexed(id(2)).expect("load aggregate 2");
    assert_eq!(loaded_b.len(), 1);
    assert_eq!(loaded_b[0].payload(), repo_two_first.payload());
}

#[test]
fn read_seam_recovers_sequence_by_decoding_envelope_bytes() {
    let env = envelope(7, 3, 99);
    let payload = payload(&env);
    let decoded: EventEnvelope<TestEvent> =
        rmp_serde::from_slice(payload.envelope_bytes()).expect("decode envelope");
    assert_eq!(decoded.sequence().get(), 3);
    assert_eq!(payload.aggregate_id, 7);
}

fn id(raw: u64) -> AggregateId {
    AggregateId::new(NonZeroU64::new(raw).expect("non-zero aggregate id"))
}
