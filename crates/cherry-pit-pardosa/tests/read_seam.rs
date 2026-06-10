use cherry_pit_core::{AggregateId, DomainEvent, EventEnvelope};
use cherry_pit_pardosa::PardosaEventStore;
use cherry_pit_pardosa::payload::EnvelopePayload;
use pardosa::store::{EventStore as PardosaStore, JetStreamBackend};
use pardosa_nats::{
    JetStreamBackend as SubstrateJetStreamBackend, JetStreamConfig, JetStreamHandle, RuntimeHandle,
};
use pardosa_nats::test_support::LiveNatsServer;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;

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

#[test]
#[ignore = "requires nats-server on PATH"]
fn read_seam_lists_and_loads_logical_streams_after_jetstream_rehydrate() {
    let server = LiveNatsServer::acquire();
    let runtime = Runtime::new().expect("tokio runtime");
    let tag = unique_tag();
    let stream_name = format!("CHERRY_PIT_PARDOSA_READ_SEAM_{tag}");
    seed_empty_blob(&tag, &runtime, &server);
    {
        let handle = jetstream_handle(&tag, &runtime, &server);
        let backend = JetStreamBackend::open(handle);
        let mut store = PardosaStore::<EnvelopePayload>::open_with_backend(backend)
            .expect("open seeded jetstream store");
        let first = envelope(1, 1, 10);
        let second = envelope(1, 2, 20);
        let other = envelope(2, 1, 30);
        let _ = store.writer().begin(payload(&second)).expect("begin aggregate 1 sequence 2");
        let _ = store.writer().begin(payload(&other)).expect("begin aggregate 2 sequence 1");
        let _ = store.writer().begin(payload(&first)).expect("begin aggregate 1 sequence 1");
        let _ = store.writer().sync().expect("sync jetstream store");
    }

    let handle = jetstream_handle(&tag, &runtime, &server);
    let backend = JetStreamBackend::open(handle);
    let store = PardosaEventStore::<TestEvent>::open_jetstream(backend).expect("open adapter");
    let mut aggregates = store.list_indexed_aggregates().expect("list aggregates");
    aggregates.sort_unstable();
    assert_eq!(aggregates, vec![id(1), id(2)]);
    let loaded = store.load_indexed(id(1)).expect("load aggregate 1");
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].sequence().get(), 1);
    assert_eq!(loaded[1].sequence().get(), 2);

    runtime.block_on(delete_stream(&server, &stream_name));
}

fn seed_empty_blob(tag: &str, runtime: &Runtime, server: &Arc<LiveNatsServer>) {
    let seed = canonical_empty_pgno_bytes(tag);
    let handle = jetstream_handle(tag, runtime, server);
    let _ = handle.append(&seed).expect("append seed blob");
    let _ = handle.sync().expect("sync seed blob");
}

fn canonical_empty_pgno_bytes(tag: &str) -> Vec<u8> {
    let mut path = std::env::temp_dir();
    path.push(format!("cherry-pit-pardosa-empty-{tag}.pgno"));
    {
        let mut store = PardosaStore::<EnvelopePayload>::create(&path).expect("create empty pgno");
        let _ = store.writer().sync().expect("sync empty pgno");
    }
    let bytes = std::fs::read(&path).expect("read seed bytes");
    let _ = std::fs::remove_file(path);
    bytes
}

fn jetstream_handle(
    tag: &str,
    runtime: &Runtime,
    server: &LiveNatsServer,
) -> JetStreamHandle {
    let cfg = JetStreamConfig::builder()
        .stream_name(format!("CHERRY_PIT_PARDOSA_READ_SEAM_{tag}"))
        .subject(format!("cherry.pit.pardosa.read.{tag}"))
        .durable_consumer(format!("cherry-pit-pardosa-read-{tag}"))
        .runtime_handle(RuntimeHandle::from_tokio(runtime.handle().clone()))
        .nats_url(server.url().to_owned())
        .build()
        .expect("jetstream config");
    SubstrateJetStreamBackend::open(cfg)
}

fn unique_tag() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("{}_{nanos}", std::process::id())
}

async fn delete_stream(server: &LiveNatsServer, stream_name: &str) {
    let Ok(client) = async_nats::connect(server.url()).await else {
        return;
    };
    let jetstream = async_nats::jetstream::new(client);
    let _ = jetstream.delete_stream(stream_name).await;
}
