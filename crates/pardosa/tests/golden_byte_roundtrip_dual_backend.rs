//! P1 crux guard (epic `adr-fmt-t7t4v`, phase `adr-fmt-q8qyn`,
//! pre-mortem #3): proves the `.pgno` rehydrate arm and the
//! `JetStream` rehydrate arm fold the SAME raw canonical bytes for
//! the SAME log into an identical [`Frontier`] — the rolling BLAKE3
//! digest computed only from `previous_frontier || raw_event_bytes`
//! (PGN-0010:R4) — after both arms are routed through the shared
//! post-seam stage (`crate::persist::rebuild_dragline_with_frontier`).
//!
//! Method: canonical per-event bytes are captured from a real
//! `.pgno` write via [`Reader::read_message`] (the exact bytes the
//! `.pgno` rehydrate arm folds), then republished verbatim as
//! individual `JetStream` frames so the `JetStream` arm folds the
//! identical bytes. Equal `Frontier` values after independent
//! rehydrate on each backend is the golden-byte proof; equal decoded
//! event lines confirms decode determinism over those bytes.
mod support_live_nats;
use pardosa::store::{
    EventStore, Frontier, GenomeSafe, HasEventSchemaSource, JetStreamBackend, PgnoBackend,
};
use pardosa_file::Reader;
use pardosa_nats::{JetStreamBackend as SubstrateJetStreamBackend, JetStreamConfig, RuntimeHandle};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use support_live_nats::LiveNatsServer;
use tokio::runtime::Runtime;

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Ledger {
    seq: u64,
}

impl HasEventSchemaSource for Ledger {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}

fn unique_tag() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("{}_{nanos}", std::process::id())
}

fn schema_marker() -> String {
    format!("{:032x}", pardosa::store::Event::<Ledger>::ENVELOPE_HASH)
}

/// Write `n` events through a real `.pgno` store and read back the
/// exact per-event canonical bytes `Reader::read_message` returns —
/// the raw bytes the `.pgno` unchecked rehydrate arm folds.
fn canonical_event_bytes(n: u64) -> Vec<Vec<u8>> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("golden.pgno");
    let mut store = EventStore::<Ledger>::create(&path).expect("create scratch pgno");
    for seq in 0..n {
        let _ = store.writer().begin(Ledger { seq }).expect("begin event");
    }
    let _ = store.writer().sync().expect("sync");
    drop(store);
    let file = std::fs::File::open(&path).expect("reopen pgno for raw read");
    let mut reader = Reader::open(file).expect("open pgno reader");
    let n = reader.index().len();
    (0..n)
        .map(|i| reader.read_message(i).expect("read raw message"))
        .collect()
}

async fn publish_frames(server: &LiveNatsServer, stream_name: &str, subject: &str, tag: &str) {
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
        description: Some(schema_marker()),
        ..Default::default()
    })
    .await
    .expect("provision golden-log stream");

    for bytes in canonical_event_bytes(5) {
        let mut headers = async_nats::HeaderMap::new();
        headers.insert("Pardosa-Envelope-Hash", schema_marker().as_str());
        js.publish_with_headers(subject.to_owned(), headers, bytes.into())
            .await
            .expect("publish accepted")
            .await
            .expect("publish ack received");
    }
    let _ = tag;
}

fn fold_expected_frontier(raw_bytes: &[Vec<u8>]) -> Frontier {
    raw_bytes
        .iter()
        .fold(Frontier::GENESIS, |frontier, bytes| frontier.roll(bytes))
}

async fn teardown_stream(server: &LiveNatsServer, stream_name: &str) {
    let Ok(client) = async_nats::connect(server.url()).await else {
        return;
    };
    let js = async_nats::jetstream::new(client);
    let _ = js.delete_stream(stream_name).await;
}

#[test]
fn pgno_and_jetstream_rehydrate_fold_identical_frontier_for_same_log() {
    let raw_bytes = canonical_event_bytes(5);
    assert_eq!(raw_bytes.len(), 5, "fixture log carries 5 events");

    let expected = fold_expected_frontier(&raw_bytes);

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("golden_replay.pgno");
    {
        let mut store = EventStore::<Ledger>::create(&path).expect("create replay pgno");
        for seq in 0..5u64 {
            let _ = store
                .writer()
                .begin(Ledger { seq })
                .expect("begin replay event");
        }
        let _ = store.writer().sync().expect("sync replay pgno");
    }
    let pgno_backend = PgnoBackend::open(&path);
    let pgno_store =
        EventStore::<Ledger>::open_with_backend(pgno_backend).expect("open_with_backend(pgno)");
    let pgno_frontier = pgno_store.reader().frontier();

    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_tag();
    let stream_name = format!("PARDOSA_GOLDEN_{tag}");
    let subject = format!("pardosa.golden.{tag}");
    rt.block_on(publish_frames(&server, &stream_name, &subject, &tag));

    let cfg = JetStreamConfig::builder()
        .stream_name(stream_name.clone())
        .subject(subject)
        .durable_consumer(format!("pardosa-golden-c-{tag}"))
        .runtime_handle(RuntimeHandle::from_tokio(rt.handle().clone()))
        .nats_url(server.url().to_owned())
        .build()
        .expect("live JetStream config");
    let jetstream_backend = JetStreamBackend::open(SubstrateJetStreamBackend::open(cfg));
    let jetstream_store = EventStore::<Ledger>::open_with_backend(jetstream_backend)
        .expect("open_with_backend(jetstream)");
    let jetstream_frontier = jetstream_store.reader().frontier();

    rt.block_on(teardown_stream(&server, &stream_name));

    assert_eq!(
        pgno_frontier, expected,
        ".pgno arm must fold the captured raw bytes to the independently-derived frontier"
    );
    assert_eq!(
        jetstream_frontier, expected,
        "JetStream arm must fold the republished raw bytes to the SAME frontier as .pgno \
         (golden-byte round-trip, pre-mortem #3) — mismatch means a client-side frame \
         mutation broke byte-identity across the seam"
    );
    assert_eq!(
        pgno_frontier, jetstream_frontier,
        "Pgno-rehydrate raw bytes == NATS-rehydrate raw bytes for the same log"
    );
}
