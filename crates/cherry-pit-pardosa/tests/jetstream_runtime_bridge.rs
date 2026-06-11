use cherry_pit_core::{CorrelationContext, DomainEvent, EventEnvelope, EventStore};
use cherry_pit_pardosa::PardosaEventStore;
use pardosa::store::JetStreamBackend;
use pardosa_nats::{JetStreamBackend as SubstrateJetStreamBackend, JetStreamConfig, RuntimeHandle};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum TestEvent {
    Created { value: u64 },
    Updated { value: u64 },
}

impl DomainEvent for TestEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Created { .. } => "runtime-bridge.created",
            Self::Updated { .. } => "runtime-bridge.updated",
        }
    }
}

fn live_nats_url() -> String {
    std::env::var("PARDOSA_LIVE_NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string())
}

fn unique_tag() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("{}_{nanos}", std::process::id())
}

fn stream_name(tag: &str) -> String {
    format!("CHERRY_PIT_PARDOSA_RUNTIME_BRIDGE_{tag}")
}

fn jetstream_backend(
    tag: &str,
    nats_url: &str,
    handle: tokio::runtime::Handle,
) -> JetStreamBackend {
    let cfg = JetStreamConfig::builder()
        .stream_name(stream_name(tag))
        .subject(format!("cherry.pit.pardosa.runtime.bridge.{tag}"))
        .durable_consumer(format!("cherry-pit-pardosa-runtime-bridge-{tag}"))
        .runtime_handle(RuntimeHandle::from_tokio(handle))
        .nats_url(nats_url.to_owned())
        .build()
        .expect("jetstream config");
    JetStreamBackend::open(SubstrateJetStreamBackend::open(cfg))
}

fn assert_loaded_events(loaded: &[EventEnvelope<TestEvent>]) {
    assert_eq!(loaded.len(), 2, "create plus append must rehydrate");
    assert_eq!(loaded[0].sequence().get(), 1, "create sequence");
    assert_eq!(loaded[1].sequence().get(), 2, "append sequence");
    assert_eq!(loaded[0].payload(), &TestEvent::Created { value: 1 });
    assert_eq!(loaded[1].payload(), &TestEvent::Updated { value: 2 });
}

async fn delete_stream(nats_url: &str, stream_name: &str) {
    let Ok(client) = async_nats::connect(nats_url).await else {
        return;
    };
    let jetstream = async_nats::jetstream::new(client);
    let _ = jetstream.delete_stream(stream_name).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires live nats-server at PARDOSA_LIVE_NATS_URL or nats://localhost:4222"]
async fn runtime_task_create_append_load_rehydrates_jetstream_without_nested_runtime_panic() {
    let nats_url = live_nats_url();
    let tag = unique_tag();
    let stream_name = stream_name(&tag);
    let handle = tokio::runtime::Handle::current();
    let create_backend = jetstream_backend(&tag, &nats_url, handle.clone());
    let store = tokio::task::spawn_blocking(move || {
        PardosaEventStore::<TestEvent>::create_jetstream(create_backend)
    })
    .await
    .expect("create_jetstream join")
    .expect("create_jetstream adapter");
    let store = Arc::new(store);
    let aggregate_id = tokio::spawn({
        let store = Arc::clone(&store);
        async move {
            let (id, created) = store
                .create(
                    vec![TestEvent::Created { value: 1 }],
                    CorrelationContext::none(),
                )
                .await
                .expect("create from runtime worker");
            assert_eq!(created.len(), 1, "one create envelope");
            let expected = NonZeroU64::new(created.len() as u64).expect("non-zero sequence");
            let appended = store
                .append(
                    id,
                    expected,
                    vec![TestEvent::Updated { value: 2 }],
                    CorrelationContext::none(),
                )
                .await
                .expect("append from runtime worker");
            assert_eq!(appended.len(), 1, "one append envelope");
            let loaded = store.load(id).await.expect("load from runtime worker");
            assert_loaded_events(&loaded);
            id
        }
    })
    .await
    .expect("merger-like runtime task must not panic");
    drop(store);

    let reopen_backend = jetstream_backend(&tag, &nats_url, handle);
    let reopened = tokio::task::spawn_blocking(move || {
        PardosaEventStore::<TestEvent>::open_jetstream(reopen_backend)
    })
    .await
    .expect("open_jetstream join")
    .expect("open_jetstream adapter after write");
    let loaded = reopened
        .load_indexed(aggregate_id)
        .expect("load rehydrated aggregate");
    assert_loaded_events(&loaded);
    delete_stream(&nats_url, &stream_name).await;
}
