//! TEST 2 (bd `adr-fmt-krndg`, epic `adr-fmt-3dvym`): the load-bearing
//! true-reject counter-proof for the `JetStream` arm — sibling of
//! `precursor_tamper_real_boot_pgno.rs`.
//!
//! Builds two chained-fiber events via a scratch `.pgno` store, reads
//! back their exact canonical bytes (the same bytes the `.pgno` arm
//! folds — see `golden_byte_roundtrip_dual_backend.rs`), flips a
//! trailing payload byte in event 0, and republishes both frames
//! verbatim to a real `JetStream` stream. Unlike the `.pgno` container
//! there is no per-frame checksum layer on the `JetStream` arm (no
//! `xxh64` index entry to patch): `gate_replay_schema_tag` only checks
//! the envelope-hash header tag, so the tamper is already isolated to
//! the precursor-hash layer without any extra fixup — the strongest
//! form of this counter-proof, landed committed (not merely
//! operational) because the live-NATS harness this repo already
//! ships (`support_live_nats::LiveNatsServer`) makes it cheap.
//!
//! Each of the two assertions (`Enforce` rejects / `ObserveOnly` warns
//! without rejecting) provisions its own ephemeral stream and its own
//! live `nats-server` instance so the `Enforce` arm can run in an
//! isolated subprocess (env-var isolation, mirroring
//! `precursor_tamper_real_boot_pgno.rs` and
//! `store/inner/lifecycle.rs`'s `run_isolated_worker`) without any
//! cross-process nats-server hand-off.
mod support_live_nats;
use pardosa::store::replay::{CheckedReplayKind, Error as ReplayError};
use pardosa::store::{
    Event, EventStore, GenomeSafe, HasEventSchemaSource, JetStreamBackend, PardosaError,
};
use pardosa_file::Reader;
use pardosa_nats::{JetStreamBackend as SubstrateJetStreamBackend, JetStreamConfig, RuntimeHandle};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use support_live_nats::LiveNatsServer;
use tokio::runtime::Runtime;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Tagged {
    seq: u64,
}

impl HasEventSchemaSource for Tagged {
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
    format!("{:032x}", Event::<Tagged>::ENVELOPE_HASH)
}

/// Two chained events (genesis + one same-fiber successor) via the
/// public write API, returned as their exact canonical `.pgno` bytes
/// (the second referencing the first as `Precursor::Of(0)`).
fn canonical_chained_event_bytes() -> Vec<Vec<u8>> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("nats-tamper-scratch.pgno");
    {
        let mut store = EventStore::<Tagged>::create(&path).expect("create scratch pgno");
        let first = store
            .writer()
            .begin(Tagged { seq: 1 })
            .expect("begin genesis event");
        let _second = store
            .writer()
            .append(first.fiber(), Tagged { seq: 2 })
            .expect("append chained event referencing genesis as precursor");
        let _lsn = store.writer().sync().expect("sync scratch chain");
    }
    let file = std::fs::File::open(&path).expect("reopen scratch pgno for raw read");
    let mut reader = Reader::open(file).expect("open pgno reader");
    let count = reader.index().len();
    (0..count)
        .map(|i| reader.read_message(i).expect("read raw canonical message"))
        .collect()
}

/// Flip the trailing payload byte of `messages[0]` (the last-encoded
/// field per `Event::encode`, PGN wire order) — the same isolation
/// technique as the `.pgno` sibling test, minus any checksum fixup:
/// `JetStream` frames carry no per-frame integrity layer, so this
/// directly targets the precursor-hash check with no other control
/// class in the way.
fn tamper_trailing_payload_byte(messages: &mut [Vec<u8>]) {
    let last = messages[0].len() - 1;
    messages[0][last] ^= 0xFF;
}

async fn provision_and_publish_tampered_chain(
    server: &LiveNatsServer,
    stream_name: &str,
    subject: &str,
) {
    use async_nats::jetstream::stream::{
        Config as StreamConfig, DiscardPolicy, RetentionPolicy, StorageType,
    };
    let mut messages = canonical_chained_event_bytes();
    tamper_trailing_payload_byte(&mut messages);

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
    .expect("provision tamper-target stream");

    for bytes in messages {
        let mut headers = async_nats::HeaderMap::new();
        headers.insert("Pardosa-Envelope-Hash", schema_marker().as_str());
        js.publish_with_headers(subject.to_owned(), headers, bytes.into())
            .await
            .expect("publish accepted")
            .await
            .expect("publish ack received");
    }
}

async fn teardown_stream(server: &LiveNatsServer, stream_name: &str) {
    let Ok(client) = async_nats::connect(server.url()).await else {
        return;
    };
    let js = async_nats::jetstream::new(client);
    let _ = js.delete_stream(stream_name).await;
}

fn build_config(
    server: &LiveNatsServer,
    rt: &Runtime,
    stream_name: &str,
    subject: &str,
    tag: &str,
) -> JetStreamConfig {
    JetStreamConfig::builder()
        .stream_name(stream_name.to_owned())
        .subject(subject.to_owned())
        .durable_consumer(format!("pardosa-tamper-c-{tag}"))
        .runtime_handle(RuntimeHandle::from_tokio(rt.handle().clone()))
        .nats_url(server.url().to_owned())
        .build()
        .expect("live JetStream config")
}

fn run_isolated_worker(worker_name: &str) {
    let exe = std::env::current_exe().expect("current test binary path");
    let output = std::process::Command::new(exe)
        .arg(worker_name)
        .arg("--exact")
        .arg("--ignored")
        .env("PARDOSA_PRECURSOR_CHECK_MODE", "enforce")
        .output()
        .expect("spawn isolated worker subprocess");
    assert!(
        output.status.success(),
        "worker {worker_name} failed under env PARDOSA_PRECURSOR_CHECK_MODE=enforce: \
         stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn enforce_real_boot_rejects_tampered_jetstream_stream_via_open_with_backend() {
    run_isolated_worker("enforce_worker_nats_rejects_via_open_with_backend");
}

#[test]
#[ignore = "isolated worker spawned as a subprocess by \
            enforce_real_boot_rejects_tampered_jetstream_stream_via_open_with_backend \
            with PARDOSA_PRECURSOR_CHECK_MODE=enforce set on the child process only; \
            never runs under a plain `cargo test`; provisions its own live nats-server \
            so no cross-process server hand-off is required"]
fn enforce_worker_nats_rejects_via_open_with_backend() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_tag();
    let stream_name = format!("PARDOSA_TAMPER_ENFORCE_{tag}");
    let subject = format!("pardosa.tamper.enforce.{tag}");
    rt.block_on(provision_and_publish_tampered_chain(
        &server,
        &stream_name,
        &subject,
    ));

    let cfg = build_config(&server, &rt, &stream_name, &subject, &tag);
    let backend = JetStreamBackend::open(SubstrateJetStreamBackend::open(cfg));
    let err = match EventStore::<Tagged>::open_with_backend(backend) {
        Ok(_store) => panic!(
            "Enforce must reject the tampered JetStream stream at the real \
             open_with_backend boot"
        ),
        Err(err) => err,
    };
    rt.block_on(teardown_stream(&server, &stream_name));
    match err {
        PardosaError::CursorRead { source } => match *source {
            ReplayError::CheckedReplay {
                kind: CheckedReplayKind::PrecursorHashMismatch { .. },
            } => {}
            other => panic!(
                "expected CheckedReplay::PrecursorHashMismatch via real \
                 open_with_backend under Enforce (JetStream arm), got: {other:?}"
            ),
        },
        other => panic!(
            "expected PardosaError::CursorRead wrapping CheckedReplay (JetStream arm), \
             got: {other:?}"
        ),
    }
}

#[derive(Clone, Default)]
struct TraceWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl TraceWriter {
    fn snapshot(&self) -> String {
        String::from_utf8(self.buf.lock().expect("buffer mutex").clone()).expect("utf-8")
    }
}

impl std::io::Write for TraceWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf
            .lock()
            .expect("buffer mutex")
            .extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for TraceWriter {
    type Writer = TraceWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn capture_tracing<R>(f: impl FnOnce() -> R) -> (R, String) {
    let writer = TraceWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::WARN)
        .with_writer(writer.clone())
        .with_ansi(false)
        .with_target(false)
        .finish();
    let result = tracing::subscriber::with_default(subscriber, f);
    (result, writer.snapshot())
}

#[test]
fn observe_only_real_boot_boots_tampered_jetstream_stream_without_rejecting() {
    assert!(
        std::env::var("PARDOSA_PRECURSOR_CHECK_MODE").is_err(),
        "this test asserts the default ObserveOnly path; env must stay unset here"
    );
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_tag();
    let stream_name = format!("PARDOSA_TAMPER_OBSERVE_{tag}");
    let subject = format!("pardosa.tamper.observe.{tag}");
    rt.block_on(provision_and_publish_tampered_chain(
        &server,
        &stream_name,
        &subject,
    ));

    let cfg = build_config(&server, &rt, &stream_name, &subject, &tag);
    let (store, captured) = capture_tracing(|| {
        let backend = JetStreamBackend::open(SubstrateJetStreamBackend::open(cfg));
        EventStore::<Tagged>::open_with_backend(backend)
            .expect("ObserveOnly must boot the same tampered JetStream stream without rejecting")
    });
    rt.block_on(teardown_stream(&server, &stream_name));

    assert_ne!(
        store.reader().frontier(),
        pardosa::store::Frontier::GENESIS,
        "ObserveOnly boot must observe the committed (tampered) chain in the frontier"
    );
    assert!(
        captured.contains("precursor_check_would_fail"),
        "ObserveOnly must emit the precursor_check_would_fail warn for the tampered \
         JetStream chain even though it does not reject; captured tracing: {captured}"
    );
}
