mod support_live_nats;

use pardosa::store::{
    Event, EventStore, GenomeSafe, HasEventSchemaSource, JetStreamBackend, PardosaError,
};
use pardosa_nats::{JetStreamBackend as SubstrateJetStreamBackend, JetStreamConfig, RuntimeHandle};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use support_live_nats::LiveNatsServer;
use tokio::runtime::Runtime;

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Ledger {
    seq: u64,
}

impl HasEventSchemaSource for Ledger {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct ForeignLedger {
    seq: u64,
}

impl HasEventSchemaSource for ForeignLedger {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}

fn unique_stream_tag() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let pid = std::process::id();
    format!("{pid}_{nanos}")
}

fn live_config(tag: &str, rt: &Runtime, server: &LiveNatsServer) -> JetStreamConfig {
    JetStreamConfig::builder()
        .stream_name(format!("PARDOSA_SCHEMA_GATE_{tag}"))
        .subject(format!("pardosa.schema_gate.{tag}"))
        .durable_consumer(format!("pardosa-schema-gate-{tag}"))
        .runtime_handle(RuntimeHandle::from_tokio(rt.handle().clone()))
        .nats_url(server.url().to_owned())
        .build()
        .expect("live JetStream config")
}

async fn teardown_stream(server: &LiveNatsServer, stream_name: &str) {
    let Ok(client) = async_nats::connect(server.url()).await else {
        return;
    };
    let js = async_nats::jetstream::new(client);
    let _ = js.delete_stream(stream_name).await;
}

fn schema_marker<T: GenomeSafe>() -> String {
    format!("{:032x}", Event::<T>::ENVELOPE_HASH)
}

fn pgno_bytes_for<T>(event: T) -> Vec<u8>
where
    T: GenomeSafe + HasEventSchemaSource + pardosa_wire::Decode + pardosa_wire::Encode,
{
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("seed.pgno");
    let mut store = EventStore::<T>::create(&path).expect("create seed pgno");
    let _receipt = store.writer().begin(event).expect("begin seed event");
    let _lsn = store.writer().sync().expect("sync seed pgno");
    drop(store);
    std::fs::read(path).expect("read seed pgno bytes")
}

async fn seed_markerless_stream(
    server: &LiveNatsServer,
    stream_name: &str,
    subject: &str,
    payload: Vec<u8>,
    replay_schema_tag: Option<&str>,
) {
    use async_nats::jetstream::stream::{
        Config as StreamConfig, DiscardPolicy, RetentionPolicy, StorageType,
    };

    let client = async_nats::connect(server.url()).await.expect("connect");
    let js = async_nats::jetstream::new(client);
    js.get_or_create_stream(StreamConfig {
        name: stream_name.to_owned(),
        subjects: vec![subject.to_owned()],
        storage: StorageType::File,
        num_replicas: 1,
        retention: RetentionPolicy::Limits,
        discard: DiscardPolicy::New,
        duplicate_window: Duration::from_mins(2),
        ..Default::default()
    })
    .await
    .expect("provision markerless stream");

    let mut headers = async_nats::HeaderMap::new();
    if let Some(tag) = replay_schema_tag {
        headers.insert("Pardosa-Envelope-Hash", tag);
    }
    let ack = js
        .publish_with_headers(subject.to_owned(), headers, payload.into())
        .await
        .expect("publish accepted by JetStream")
        .await
        .expect("publish ack received");
    assert!(ack.sequence > 0, "seed payload reached JetStream");
}

async fn stream_description(server: &LiveNatsServer, stream_name: &str) -> Option<String> {
    let client = async_nats::connect(server.url()).await.expect("connect");
    let js = async_nats::jetstream::new(client);
    let stream = js.get_stream(stream_name).await.expect("get stream");
    stream
        .get_info()
        .await
        .expect("read stream info")
        .config
        .description
}

fn assert_schema_hash_mismatch(err: &PardosaError) {
    let PardosaError::CursorRead { source } = err else {
        panic!("expected CursorRead wrapping SchemaHashMismatch, got {err:?}");
    };
    assert!(
        matches!(
            source.as_ref(),
            pardosa::store::replay::Error::SchemaHashMismatch { .. }
        ),
        "expected SchemaHashMismatch, got {source:?}"
    );
}

#[test]
#[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission g3-jetstream-schema-gate-exec)"]
fn live_open_refuses_populated_stream_with_foreign_marker() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_SCHEMA_GATE_{tag}");
    let subject = format!("pardosa.schema_gate.{tag}");
    let foreign_marker = "fedcba9876543210fedcba9876543210";

    rt.block_on(async {
        use async_nats::jetstream::stream::{
            Config as StreamConfig, DiscardPolicy, RetentionPolicy, StorageType,
        };

        let client = async_nats::connect(server.url()).await.expect("connect");
        let js = async_nats::jetstream::new(client);
        js.get_or_create_stream(StreamConfig {
            name: stream_name.clone(),
            subjects: vec![subject.clone()],
            storage: StorageType::File,
            num_replicas: 1,
            retention: RetentionPolicy::Limits,
            discard: DiscardPolicy::New,
            duplicate_window: Duration::from_mins(2),
            description: Some(foreign_marker.to_owned()),
            ..Default::default()
        })
        .await
        .expect("provision stream with foreign marker");

        let publish_ack = js
            .publish(
                subject.clone(),
                Vec::from(&b"foreign-marker-probe"[..]).into(),
            )
            .await
            .expect("publish accepted by JetStream")
            .await
            .expect("publish ack received");
        assert!(publish_ack.sequence > 0, "probe reached JetStream");
    });

    let backend = JetStreamBackend::open(SubstrateJetStreamBackend::open(live_config(
        &tag, &rt, &server,
    )));
    let Err(err) = EventStore::<Ledger>::open_with_backend(backend) else {
        rt.block_on(teardown_stream(&server, &stream_name));
        panic!("opening a populated stream with a foreign marker must refuse");
    };
    assert_schema_hash_mismatch(&err);

    rt.block_on(teardown_stream(&server, &stream_name));
}

#[test]
fn live_open_backfills_marker_on_populated_markerless_stream() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_SCHEMA_GATE_{tag}");
    let subject = format!("pardosa.schema_gate.{tag}");
    let marker = schema_marker::<Ledger>();
    let payload = pgno_bytes_for(Ledger { seq: 7 });

    rt.block_on(seed_markerless_stream(
        &server,
        &stream_name,
        &subject,
        payload,
        Some(&marker),
    ));

    let backend = JetStreamBackend::open(SubstrateJetStreamBackend::open(live_config(
        &tag, &rt, &server,
    )));
    let store = EventStore::<Ledger>::open_with_backend(backend)
        .expect("markerless populated stream must open after provisioning back-fills the marker");
    drop(store);
    let read_back = rt.block_on(stream_description(&server, &stream_name));
    assert_eq!(read_back.as_deref(), Some(marker.as_str()));

    rt.block_on(teardown_stream(&server, &stream_name));
}

#[test]
fn live_open_keeps_foreign_pgno_payload_fail_closed_after_marker_backfill() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_SCHEMA_GATE_{tag}");
    let subject = format!("pardosa.schema_gate.{tag}");
    let ledger_marker = schema_marker::<Ledger>();
    let foreign_payload = pgno_bytes_for(ForeignLedger { seq: 13 });
    assert_ne!(
        Event::<Ledger>::ENVELOPE_HASH,
        Event::<ForeignLedger>::ENVELOPE_HASH,
        "guard fixture must carry a genuinely foreign schema hash"
    );

    rt.block_on(seed_markerless_stream(
        &server,
        &stream_name,
        &subject,
        foreign_payload,
        None,
    ));

    let backend = JetStreamBackend::open(SubstrateJetStreamBackend::open(live_config(
        &tag, &rt, &server,
    )));
    let Err(err) = EventStore::<Ledger>::open_with_backend(backend) else {
        rt.block_on(teardown_stream(&server, &stream_name));
        panic!("foreign pgno payload must fail closed after marker back-fill");
    };
    assert_schema_hash_mismatch(&err);
    let read_back = rt.block_on(stream_description(&server, &stream_name));
    assert_eq!(read_back.as_deref(), Some(ledger_marker.as_str()));

    rt.block_on(teardown_stream(&server, &stream_name));
}
