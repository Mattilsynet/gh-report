use cherry_pit_core::{AggregateId, DomainEvent, EventEnvelope};
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

#[test]
fn genome_safe_payload_round_trips_serde_envelope_through_pardosa() {
    let aggregate_id = AggregateId::new(NonZeroU64::new(41).expect("non-zero aggregate id"));
    let envelope = EventEnvelope::new(
        uuid::Uuid::now_v7(),
        aggregate_id,
        NonZeroU64::new(1).expect("non-zero sequence"),
        jiff::Timestamp::now(),
        Some(uuid::Uuid::now_v7()),
        None,
        TestEvent::Created {
            domain_key: String::from("acme/widget"),
            value: 7,
        },
    )
    .expect("valid envelope");
    let encoded = rmp_serde::to_vec_named(&envelope).expect("encode envelope");
    let payload = EnvelopePayload::new(encoded, aggregate_id.get(), String::from("acme/widget"))
        .expect("payload fits bounds");
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("events.pgno");
    let mut store = PardosaStore::<EnvelopePayload>::create(&path).expect("create pardosa store");
    let receipt = store.writer().begin(payload).expect("begin fiber");
    let fiber = receipt.fiber().fiber_id();
    let _lsn = store.writer().sync().expect("sync pardosa store");
    let reopened = PardosaStore::<EnvelopePayload>::open_validated(&path).expect("reopen");
    let stored = reopened
        .reader()
        .fiber(fiber)
        .iter()
        .expect("fiber exists")
        .next()
        .expect("one payload")
        .domain_event()
        .clone();
    assert_eq!(stored.aggregate_id, aggregate_id.get());
    assert_eq!(stored.domain_key(), "acme/widget");
    let decoded: EventEnvelope<TestEvent> =
        rmp_serde::from_slice(stored.envelope_bytes()).expect("decode envelope");
    assert_eq!(decoded.event_id(), envelope.event_id());
    assert_eq!(decoded.aggregate_id(), envelope.aggregate_id());
    assert_eq!(decoded.sequence(), envelope.sequence());
    assert_eq!(decoded.timestamp(), envelope.timestamp());
    assert_eq!(decoded.correlation_id(), envelope.correlation_id());
    assert_eq!(decoded.causation_id(), envelope.causation_id());
    assert_eq!(decoded.payload(), envelope.payload());
}
