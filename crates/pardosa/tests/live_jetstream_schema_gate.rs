mod support_live_nats;

use pardosa::store::{
    EventStore, GenomeSafe, HasEventSchemaSource, JetStreamBackend, PardosaError,
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
