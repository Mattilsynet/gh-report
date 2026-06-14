use pardosa::store::{
    EventId, EventStore, GenomeSafe, HasEventSchemaSource, JetStreamBackend, PardosaError,
};
use pardosa_nats::test_support::LiveNatsServer;
use pardosa_nats::{JetStreamBackend as SubstrateJetStreamBackend, JetStreamConfig, RuntimeHandle};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct SpikeAlpha {
    id: u64,
    code: u64,
}

impl HasEventSchemaSource for SpikeAlpha {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct SpikeBeta {
    id: u64,
    enabled: bool,
}

impl HasEventSchemaSource for SpikeBeta {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StreamNames {
    stream_name: String,
    subject: String,
    durable_consumer: String,
}

impl StreamNames {
    fn new(tag: &str, stream: &str) -> Self {
        Self {
            stream_name: format!("TWOSTREAM_SPIKE_{stream}_{tag}"),
            subject: format!("twostream.spike.{stream}.{tag}"),
            durable_consumer: format!("twostream-spike-{stream}-{tag}"),
        }
    }
}

fn unique_tag() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{}_{}_{}", std::process::id(), nanos, seq)
}

fn live_config(names: &StreamNames, rt: &Runtime, server: &LiveNatsServer) -> JetStreamConfig {
    JetStreamConfig::builder()
        .stream_name(names.stream_name.clone())
        .subject(names.subject.clone())
        .durable_consumer(names.durable_consumer.clone())
        .runtime_handle(RuntimeHandle::from_tokio(rt.handle().clone()))
        .nats_url(server.url().to_owned())
        .build()
        .expect("live JetStream config")
}

fn backend(names: &StreamNames, rt: &Runtime, server: &LiveNatsServer) -> JetStreamBackend {
    JetStreamBackend::open(SubstrateJetStreamBackend::open(live_config(
        names, rt, server,
    )))
}

fn delete_stream(rt: &Runtime, server: &LiveNatsServer, stream_name: &str) {
    let url = server.url().to_owned();
    let stream_name = stream_name.to_owned();
    rt.block_on(async move {
        let Ok(client) = async_nats::connect(&url).await else {
            return;
        };
        let js = async_nats::jetstream::new(client);
        let _ = js.delete_stream(&stream_name).await;
    });
}

fn assert_decode_rejection(err: &PardosaError) {
    let PardosaError::CursorRead { source } = err else {
        panic!(
            "foreign JetStream event must be rejected while opening the wrong typed store; \
             expected CursorRead wrapping persist::Error::SchemaHashMismatch, got {err:?}"
        );
    };
    assert!(
        matches!(
            source.as_ref(),
            pardosa::store::replay::Error::SchemaHashMismatch { .. }
        ),
        "G3 fix: JetStream per-event-frame foreign-event rejection must fire through \
         the explicit ENVELOPE_HASH gate, not decode; got {source:?}"
    );
}

fn alpha_payloads(store: &EventStore<SpikeAlpha>) -> Vec<SpikeAlpha> {
    store
        .reader()
        .causal_chain(EventId::ZERO)
        .iter()
        .map(|event| event.domain_event().clone())
        .collect()
}

fn beta_payloads(store: &EventStore<SpikeBeta>) -> Vec<SpikeBeta> {
    store
        .reader()
        .causal_chain(EventId::ZERO)
        .iter()
        .map(|event| event.domain_event().clone())
        .collect()
}

#[test]
fn two_typed_event_stores_coexist_over_distinct_jetstream_subjects() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_tag();
    let alpha_names = StreamNames::new(&tag, "alpha");
    let beta_names = StreamNames::new(&tag, "beta");

    let alpha_event = SpikeAlpha { id: 1, code: 10 };
    let beta_event = SpikeBeta {
        id: 2,
        enabled: true,
    };

    let mut alpha =
        EventStore::<SpikeAlpha>::create_with_backend(backend(&alpha_names, &rt, &server))
            .expect("create alpha JetStream store");
    let mut beta = EventStore::<SpikeBeta>::create_with_backend(backend(&beta_names, &rt, &server))
        .expect("create beta JetStream store");

    let _ = alpha
        .writer()
        .begin(alpha_event.clone())
        .expect("append alpha event");
    let _ = beta
        .writer()
        .begin(beta_event.clone())
        .expect("append beta event");
    let _ = alpha.writer().sync().expect("sync alpha JetStream store");
    let _ = beta.writer().sync().expect("sync beta JetStream store");
    drop(alpha);
    drop(beta);

    let reopened_alpha =
        EventStore::<SpikeAlpha>::open_with_backend(backend(&alpha_names, &rt, &server))
            .expect("reopen alpha JetStream store");
    let reopened_beta =
        EventStore::<SpikeBeta>::open_with_backend(backend(&beta_names, &rt, &server))
            .expect("reopen beta JetStream store");
    assert_eq!(alpha_payloads(&reopened_alpha), vec![alpha_event]);
    assert_eq!(beta_payloads(&reopened_beta), vec![beta_event]);

    let Err(foreign_err) =
        EventStore::<SpikeAlpha>::open_with_backend(backend(&beta_names, &rt, &server))
    else {
        panic!("opening beta stream as alpha must reject the foreign event");
    };
    assert_decode_rejection(&foreign_err);

    delete_stream(&rt, &server, &alpha_names.stream_name);
    delete_stream(&rt, &server, &beta_names.stream_name);
}
