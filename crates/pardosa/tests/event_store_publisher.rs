//! Publisher-seam round-trip for path-backed `EventStore<T>`
//! (ADR-0018 §12 bullet 3; ADR-0016 §§D5–D8, §D9 §2).
//!
//! Exercises `FrontierPublisher`, `PublishError`, and
//! `EventStore::open_with_publisher` via a local `LocalPublisher`
//! fake. Asserts the reconstructed-anchors drain produces the
//! expected `(subject, payload)` sequence and reopen suppresses
//! re-publish of already-acked anchors via the durable
//! publish-watermark sidecar (ADR-0016 §D7).
//!
//! The `jetstream_soak` submodule drives the 100× publisher-failure
//! / recovery soak against a real `nats-server`.
mod support_live_nats;
use pardosa::store::{
    EventStore, FrontierPublisher, GenomeSafe, HasEventSchemaSource, PublishError,
};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
type PublishLog = Arc<Mutex<Vec<(String, Vec<u8>)>>>;
#[derive(Clone, Debug)]
struct LocalPublisher {
    log: PublishLog,
}
impl LocalPublisher {
    fn new() -> Self {
        Self {
            log: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn published(&self) -> Vec<(String, Vec<u8>)> {
        self.log.lock().expect("mutex").clone()
    }
}
impl FrontierPublisher for LocalPublisher {
    fn publish(&mut self, subject: &str, payload: &[u8]) -> Result<(), PublishError> {
        self.log
            .lock()
            .expect("mutex")
            .push((subject.to_owned(), payload.to_vec()));
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Order {
    id: u64,
}
impl HasEventSchemaSource for Order {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
#[test]
fn open_with_publisher_durable_round_trip() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    {
        let mut seed = EventStore::<Order>::create(&journal).expect("create");
        for id in 0..3u64 {
            let _ = seed.writer().begin(Order { id }).expect("begin");
        }
        let _ = seed.writer().sync().expect("seed sync");
    }
    let publisher = LocalPublisher::new();
    let probe = publisher.clone();
    let mut store = EventStore::<Order>::open_with_publisher(
        &journal,
        sidecar.clone(),
        "round-trip".to_owned(),
        1,
        Box::new(publisher),
    )
    .expect("open_with_publisher");
    let _ = store.writer().sync().expect("first sync drains anchors");
    let first = probe.published();
    assert_eq!(
        first.len(),
        3,
        "every reconstructed anchor must be published on first sync (ADR-0016 §D6)"
    );
    for (subject, payload) in &first {
        assert_eq!(subject, "pardosa.round-trip.frontier");
        assert_eq!(payload.len(), 32, "frontier payload is 32-byte BLAKE3");
    }
    let unique_payloads: std::collections::HashSet<_> =
        first.iter().map(|(_, p)| p.clone()).collect();
    assert_eq!(
        unique_payloads.len(),
        3,
        "rolling-frontier digest advances per anchor (ADR-0004)"
    );
    drop(store);
    assert!(
        sidecar.exists(),
        "publish-watermark sidecar must be fsync-ed before drain returns (ADR-0016 §D5)"
    );
    let republisher = LocalPublisher::new();
    let republish_probe = republisher.clone();
    let mut reopened = EventStore::<Order>::open_with_publisher(
        &journal,
        sidecar.clone(),
        "round-trip".to_owned(),
        1,
        Box::new(republisher),
    )
    .expect("reopen_with_publisher");
    let _ = reopened
        .writer()
        .sync()
        .expect("second sync respects watermark");
    let after_reopen = republish_probe.published();
    assert!(
        after_reopen.is_empty(),
        "watermark covering the full line suppresses re-publish (ADR-0016 §D7); got {} anchors",
        after_reopen.len(),
    );
    let mut writer = reopened.writer();
    let _ = writer.begin(Order { id: 99 }).expect("begin post-reopen");
    let _ = writer.sync().expect("post-reopen sync");
    let after_extend = republish_probe.published();
    assert_eq!(
        after_extend.len(),
        1,
        "only the new anchor publishes after watermark advance"
    );
    assert_eq!(after_extend[0].0, "pardosa.round-trip.frontier");
}
#[derive(Clone, Debug)]
struct FlakeyPublisher {
    fail: Arc<Mutex<bool>>,
    log: PublishLog,
}
impl FlakeyPublisher {
    fn new_failing() -> Self {
        Self {
            fail: Arc::new(Mutex::new(true)),
            log: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn set_failing(&self, failing: bool) {
        *self.fail.lock().expect("flakey publisher mutex") = failing;
    }
    fn published(&self) -> Vec<(String, Vec<u8>)> {
        self.log.lock().expect("flakey publisher log mutex").clone()
    }
}
impl FrontierPublisher for FlakeyPublisher {
    fn publish(&mut self, subject: &str, payload: &[u8]) -> Result<(), PublishError> {
        if *self.fail.lock().expect("flakey publisher mutex") {
            return Err(PublishError::Transport);
        }
        self.log
            .lock()
            .expect("flakey publisher log mutex")
            .push((subject.to_owned(), payload.to_vec()));
        Ok(())
    }
}
#[test]
fn publisher_transport_failure_does_not_make_local_sync_fail() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    {
        let mut seed = EventStore::<Order>::create(&journal).expect("create");
        for id in 0..2u64 {
            let _ = seed.writer().begin(Order { id }).expect("seed begin");
        }
        let _ = seed.writer().sync().expect("seed sync");
    }
    let publisher = FlakeyPublisher::new_failing();
    let probe = publisher.clone();
    let mut store = EventStore::<Order>::open_with_publisher(
        &journal,
        sidecar.clone(),
        "pub-fail-local-ok".to_owned(),
        1,
        Box::new(publisher),
    )
    .expect("open_with_publisher");
    let lsn = store
        .writer()
        .sync()
        .expect("StoreWriter::sync must return Ok despite publisher Err (ADR-0015 §D2/§D4)");
    assert_eq!(
        store.writer().acked_lsn(),
        Some(lsn),
        "acked_lsn must reflect the returned Lsn even when publish failed"
    );
    assert!(
        probe.published().is_empty(),
        "failing publisher must not have recorded any successful publishes; got {} entries",
        probe.published().len(),
    );
    let mut writer = store.writer();
    let _ = writer.begin(Order { id: 9 }).expect("post-fail begin");
    let _ = writer
        .sync()
        .expect("post-fail extension sync must still return Ok");
}
#[test]
fn unpublished_suffix_is_retryable_after_publisher_recovers() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    {
        let mut seed = EventStore::<Order>::create(&journal).expect("create");
        for id in 0..3u64 {
            let _ = seed.writer().begin(Order { id }).expect("seed begin");
        }
        let _ = seed.writer().sync().expect("seed sync");
    }
    let publisher = FlakeyPublisher::new_failing();
    let probe = publisher.clone();
    let mut store = EventStore::<Order>::open_with_publisher(
        &journal,
        sidecar.clone(),
        "pub-retry-suffix".to_owned(),
        1,
        Box::new(publisher),
    )
    .expect("open_with_publisher");
    let _ = store
        .writer()
        .sync()
        .expect("sync Ok despite publisher failures (re-buffer per ADR-0015 §D3)");
    assert!(
        probe.published().is_empty(),
        "while publisher is failing nothing must reach the success log"
    );
    probe.set_failing(false);
    let _ = store
        .writer()
        .sync()
        .expect("retry sync must Ok and drain re-buffered anchors");
    let drained = probe.published();
    assert_eq!(
        drained.len(),
        3,
        "previously-failed suffix must be retried (ADR-0015 §D3); \
         expected 3 drained anchors, got {}",
        drained.len(),
    );
    for (subject, payload) in &drained {
        assert_eq!(subject, "pardosa.pub-retry-suffix.frontier");
        assert_eq!(payload.len(), 32, "frontier payload is 32-byte BLAKE3");
    }
    let unique: std::collections::HashSet<_> = drained.iter().map(|(_, p)| p.clone()).collect();
    assert_eq!(
        unique.len(),
        3,
        "rolling-frontier digest must advance per anchor across the drained retry"
    );
}
#[test]
fn watermark_suppresses_fully_acked_anchors_after_reopen_with_flakey_publisher() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let sidecar = td.path().join("sidecar.ack");
    {
        let mut seed = EventStore::<Order>::create(&journal).expect("create");
        for id in 0..2u64 {
            let _ = seed.writer().begin(Order { id }).expect("seed begin");
        }
        let _ = seed.writer().sync().expect("seed sync");
    }
    {
        let publisher = FlakeyPublisher::new_failing();
        publisher.set_failing(false);
        let probe = publisher.clone();
        let mut store = EventStore::<Order>::open_with_publisher(
            &journal,
            sidecar.clone(),
            "pub-watermark-flakey".to_owned(),
            1,
            Box::new(publisher),
        )
        .expect("open_with_publisher");
        let _ = store.writer().sync().expect("first sync drains anchors");
        let first = probe.published();
        assert_eq!(
            first.len(),
            2,
            "all reconstructed anchors must publish before watermark is recorded"
        );
        drop(store);
    }
    assert!(
        sidecar.exists(),
        "publish-watermark sidecar must be fsync-ed after successful drain (ADR-0016 §D5)"
    );
    let republisher = FlakeyPublisher::new_failing();
    republisher.set_failing(false);
    let probe = republisher.clone();
    let mut reopened = EventStore::<Order>::open_with_publisher(
        &journal,
        sidecar.clone(),
        "pub-watermark-flakey".to_owned(),
        1,
        Box::new(republisher),
    )
    .expect("reopen_with_publisher");
    let _ = reopened
        .writer()
        .sync()
        .expect("reopen sync must Ok and respect watermark");
    assert!(
        probe.published().is_empty(),
        "watermark covering the full line must suppress re-publish; got {} anchors",
        probe.published().len(),
    );
    let in_mem = LocalPublisher::new();
    let in_mem_probe = in_mem.clone();
    drop(reopened);
    let mut reopened2 = EventStore::<Order>::open_with_publisher(
        &journal,
        sidecar.clone(),
        "pub-watermark-flakey".to_owned(),
        1,
        Box::new(in_mem),
    )
    .expect("reopen_with_publisher 2");
    let mut writer = reopened2.writer();
    let _ = writer
        .begin(Order { id: 77 })
        .expect("post-watermark begin");
    let _ = writer.sync().expect("post-watermark sync");
    let after = in_mem_probe.published();
    assert_eq!(
        after.len(),
        1,
        "only the new anchor must publish after the watermark advance"
    );
    assert_eq!(after[0].0, "pardosa.pub-watermark-flakey.frontier");
}
mod jetstream_adapter {
    use super::{EventStore, Order, PublishError, TempDir};
    use pardosa::store::{FrontierPublisher, JetStreamFrontierPublisher};
    use pardosa_nats::{JetStreamBackend, JetStreamConfig, RuntimeHandle};
    fn detached_config(stream_name: &str, subject: &str) -> JetStreamConfig {
        JetStreamConfig::builder()
            .stream_name(stream_name.to_owned())
            .subject(subject.to_owned())
            .durable_consumer(format!("{stream_name}-c"))
            .runtime_handle(RuntimeHandle::detached_for_tests())
            .build()
            .expect("offline config is valid")
    }
    #[test]
    fn subject_mismatch_is_publish_error_not_silent_route() {
        let cfg = detached_config("soak-a", "pardosa.soak-a.frontier");
        let handle = JetStreamBackend::open(cfg);
        let mut adapter = JetStreamFrontierPublisher::open(handle);
        let err = adapter
            .publish("pardosa.soak-b.frontier", &[0u8; 32])
            .expect_err("subject mismatch must surface as PublishError");
        assert!(
            matches!(err, PublishError::Custom { .. }),
            "subject mismatch maps to PublishError::Custom; got {err:?}",
        );
    }
    #[test]
    fn detached_runtime_is_publish_error_not_panic() {
        let cfg = detached_config("soak", "pardosa.soak.frontier");
        let handle = JetStreamBackend::open(cfg);
        let mut adapter = JetStreamFrontierPublisher::open(handle);
        let err = adapter
            .publish("pardosa.soak.frontier", &[7u8; 32])
            .expect_err(
                "detached runtime must surface as PublishError, never panic (ADR-0015 §D4)",
            );
        assert!(
            matches!(err, PublishError::Custom { .. }),
            "detached runtime maps to PublishError::Custom; got {err:?}",
        );
    }
    #[test]
    fn open_with_publisher_sync_remains_ok_under_detached_runtime() {
        let td = TempDir::new().expect("tempdir");
        let journal = td.path().join("journal.pgno");
        let sidecar = td.path().join("sidecar.ack");
        {
            let mut seed = EventStore::<Order>::create(&journal).expect("create");
            for id in 0..3u64 {
                let _ = seed.writer().begin(Order { id }).expect("seed begin");
            }
            let _ = seed.writer().sync().expect("seed sync");
        }
        let stream_name = "jetstream-detached-rebuffer";
        let cfg = detached_config(stream_name, &format!("pardosa.{stream_name}.frontier"));
        let handle = JetStreamBackend::open(cfg);
        let publisher = JetStreamFrontierPublisher::open(handle);
        let mut store = EventStore::<Order>::open_with_publisher(
            &journal,
            sidecar.clone(),
            stream_name.to_owned(),
            1,
            Box::new(publisher),
        )
        .expect("open_with_publisher");
        let lsn = store
            .writer()
            .sync()
            .expect("StoreWriter::sync must return Ok under publisher failure (ADR-0015 §D2/§D4)");
        assert_eq!(
            store.writer().acked_lsn(),
            Some(lsn),
            "acked_lsn must reflect the returned Lsn even when publish failed",
        );
        assert!(
            !sidecar.exists(),
            "watermark sidecar must not advance while every publish has failed (ADR-0016 §D5)",
        );
    }
}
/// Live 100× publisher failure/recovery soak.
///
/// Per iteration: seed (3 events, sync), failure phase (sync
/// returns `Ok` per ADR-0015 §D2/§D4; sidecar absent per §D5;
/// stream empty), recovery phase (flag off; sync; 3 distinct
/// 32-byte payloads per ADR-0004; sidecar exists), reopen phase
/// (fresh publisher; sync; watermark suppresses re-publish per
/// ADR-0016 §D7). All `SOAK_ITERATIONS` must satisfy all phases.
mod jetstream_soak {
    use super::support_live_nats::LiveNatsServer;
    use super::{EventStore, FrontierPublisher, Order, PublishError, TempDir};
    use pardosa::store::JetStreamFrontierPublisher;
    use pardosa_nats::{JetStreamBackend, JetStreamConfig, RuntimeHandle};
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::runtime::Runtime;
    /// Soak iteration count per the mission brief.
    const SOAK_ITERATIONS: usize = 100;
    /// Number of seed events per iteration — small enough to keep
    /// the soak under a reasonable wall-clock budget while still
    /// proving the rolling-frontier digest advances across more
    /// than one anchor (so duplicate-detection is meaningful).
    const SEED_EVENTS_PER_ITER: usize = 3;
    /// Test-only [`FrontierPublisher`] wrapper that delegates to
    /// an inner publisher (the production
    /// [`JetStreamFrontierPublisher`]) **except** while the
    /// shared `fail` flag is `true`, in which case every
    /// `publish` returns a typed `PublishError::Custom`
    /// containing an [`InjectedFailure`] cause.
    ///
    /// The wrapper exists purely for failure injection — it adds
    /// no behaviour beyond the toggle. The recovery phase flips
    /// the flag to `false` and drives the inner publisher
    /// (which talks to real `JetStream`) for the actual drain.
    #[derive(Debug)]
    struct FailThenForwardPublisher {
        inner: JetStreamFrontierPublisher,
        fail: Arc<std::sync::Mutex<bool>>,
    }
    impl FailThenForwardPublisher {
        fn new(inner: JetStreamFrontierPublisher, fail: Arc<std::sync::Mutex<bool>>) -> Self {
            Self { inner, fail }
        }
    }
    impl FrontierPublisher for FailThenForwardPublisher {
        fn publish(&mut self, subject: &str, payload: &[u8]) -> Result<(), PublishError> {
            let blocked = *self
                .fail
                .lock()
                .expect("FailThenForwardPublisher fail mutex");
            if blocked {
                return Err(PublishError::Custom {
                    source: Box::new(InjectedFailure),
                });
            }
            self.inner.publish(subject, payload)
        }
    }
    /// Marker error returned by [`FailThenForwardPublisher`]
    /// while the shared fail flag is set.
    ///
    /// Only used as the `source` chain element on
    /// `PublishError::Custom`; the soak distinguishes injected
    /// failures from substrate failures by matching this type
    /// downcast at iteration boundaries when triaging unexpected
    /// `Err` shapes — not by inspecting the `Display` text.
    #[derive(Debug)]
    struct InjectedFailure;
    impl std::fmt::Display for InjectedFailure {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "test-injected publisher failure (fail flag set)")
        }
    }
    impl std::error::Error for InjectedFailure {}
    /// Per-iteration unique tag — combines pid, monotonic clock,
    /// iteration index, and a process-wide atomic so neither
    /// concurrent test runs nor sequential iterations within one
    /// run can collide on stream/subject/durable names.
    fn iter_tag(iter: usize) -> String {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let pid = std::process::id();
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        format!("soak02_{pid}_{nanos}_{iter}_{seq}")
    }
    /// Build a live [`JetStreamConfig`] bound to a shared tokio
    /// runtime, spawned server, and unique stream/subject/durable
    /// for this iter.
    fn live_config(tag: &str, rt: &Runtime, server: &LiveNatsServer) -> JetStreamConfig {
        JetStreamConfig::builder()
            .stream_name(format!("PARDOSA_SOAK02_{tag}"))
            .subject(format!("pardosa.PARDOSA_SOAK02_{tag}.frontier"))
            .durable_consumer(format!("pardosa-soak02-c-{tag}"))
            .runtime_handle(RuntimeHandle::from_tokio(rt.handle().clone()))
            .nats_url(server.url().to_owned())
            .build()
            .expect("config valid for soak iteration")
    }
    /// Typed outcome shape for [`read_stream_payloads`].
    ///
    /// The substrate creates the stream lazily on the first
    /// successful `append`. During the failure phase nothing
    /// reaches `JetStreamHandle::append`, so the stream does not
    /// exist when the failure-phase read runs — expected, not an
    /// error. [`ReadError::StreamNotFound`] is the typed signal;
    /// every other error surfaces as [`ReadError::Other`] so the
    /// failure-phase guard fails loud on regression.
    #[derive(Debug)]
    enum ReadError {
        StreamNotFound,
        Other(String),
    }
    impl std::fmt::Display for ReadError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::StreamNotFound => {
                    write!(
                        f,
                        "stream does not exist (substrate lazily creates streams on first publish)"
                    )
                }
                Self::Other(msg) => write!(f, "{msg}"),
            }
        }
    }
    /// Read every message in `stream_name` matching `subject` and
    /// return per-message payloads in stream order.
    ///
    /// Uses [`async_nats::jetstream`] directly (substrate exports
    /// no low-level read API; mission brief forbids extending it).
    ///
    /// # Errors
    ///
    /// * [`ReadError::StreamNotFound`] — server has no stream by
    ///   `stream_name`; expected when no publish has completed
    ///   (substrate creates streams lazily on first append).
    /// * [`ReadError::Other`] — every other failure (connect,
    ///   request, get-info, get-raw-message). Soak treats as
    ///   iteration failure.
    fn read_stream_payloads(
        rt: &Runtime,
        server: &LiveNatsServer,
        stream_name: &str,
        subject: &str,
    ) -> Result<Vec<Vec<u8>>, ReadError> {
        let url = server.url().to_owned();
        let stream_name = stream_name.to_owned();
        let subject = subject.to_owned();
        rt.block_on(async move {
            let client = async_nats::connect(&url)
                .await
                .map_err(|e| ReadError::Other(format!("connect {url}: {e}")))?;
            let js = async_nats::jetstream::new(client);
            let stream = match js.get_stream(&stream_name).await {
                Ok(s) => s,
                Err(err) => return Err(classify_get_stream_error(&stream_name, &err)),
            };
            let info = stream
                .get_info()
                .await
                .map_err(|e| ReadError::Other(format!("get_info {stream_name}: {e}")))?;
            let mut out = Vec::new();
            for seq in 1..=info.state.last_sequence {
                let raw = stream
                    .get_raw_message(seq)
                    .await
                    .map_err(|e| ReadError::Other(format!("get_raw_message seq={seq}: {e}")))?;
                if raw.subject.as_str() == subject {
                    out.push(raw.payload.to_vec());
                }
            }
            Ok(out)
        })
    }
    /// Classify a [`async_nats::jetstream::context::GetStreamError`]
    /// into [`ReadError::StreamNotFound`] when the server reported
    /// `JetStream` error code
    /// [`async_nats::jetstream::ErrorCode::STREAM_NOT_FOUND`]
    /// (10059), or [`ReadError::Other`] otherwise.
    ///
    /// Typed match against the inner `JetStream` error code — no
    /// substring matching on the rendered `Display` — so the
    /// guardrail does not silently rot if `async-nats` rewords
    /// its error messages between minor versions.
    fn classify_get_stream_error(
        stream_name: &str,
        err: &async_nats::jetstream::context::GetStreamError,
    ) -> ReadError {
        use async_nats::jetstream::context::GetStreamErrorKind;
        match err.kind() {
            GetStreamErrorKind::JetStream(inner)
                if inner.kind() == async_nats::jetstream::ErrorCode::STREAM_NOT_FOUND =>
            {
                ReadError::StreamNotFound
            }
            _ => ReadError::Other(format!("get_stream {stream_name}: {err}")),
        }
    }
    /// Best-effort stream delete at iteration end. Errors are
    /// swallowed: the soak's correctness assertions ran already,
    /// and a leaked stream on the per-spawn tempdir is reaped
    /// when the [`LiveNatsServer`] `Drop`s anyway.
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
    /// Per-iteration tempdir + derived journal + sidecar paths.
    /// `TempDir`'s `Drop` reclaims the whole directory when
    /// `IterPaths` drops.
    struct IterPaths {
        _td: TempDir,
        journal: PathBuf,
        sidecar: PathBuf,
    }
    impl IterPaths {
        fn new() -> Self {
            let td = TempDir::new().expect("tempdir");
            let journal = td.path().join("journal.pgno");
            let sidecar = td.path().join("sidecar.ack");
            Self {
                _td: td,
                journal,
                sidecar,
            }
        }
    }
    struct IterCtx<'a> {
        iter: usize,
        rt: &'a Runtime,
        server: &'a LiveNatsServer,
        stream_name: String,
        subject: String,
        paths: IterPaths,
    }
    impl IterCtx<'_> {
        fn delete_stream(&self) {
            delete_stream(self.rt, self.server, &self.stream_name);
        }
    }
    /// Drive one iteration of the soak. Returns `Ok` only if
    /// every contract assertion held; otherwise returns a string
    /// naming the first violation and the iteration index for
    /// triage.
    fn run_iteration(iter: usize, rt: &Runtime, server: &LiveNatsServer) -> Result<(), String> {
        let tag = iter_tag(iter);
        let ctx = IterCtx {
            iter,
            rt,
            server,
            stream_name: format!("PARDOSA_SOAK02_{tag}"),
            subject: format!("pardosa.PARDOSA_SOAK02_{tag}.frontier"),
            paths: IterPaths::new(),
        };
        seed_iteration_journal(iter, &ctx.paths)?;
        let fail_flag = Arc::new(std::sync::Mutex::new(true));
        let mut store = open_iteration_store(&ctx, &tag, &fail_flag)?;
        let lsn_fail = store
            .writer()
            .sync()
            .map_err(|e| format!("iter {iter}: failure-phase sync returned Err: {e:?}"))?;
        assert_failure_phase(&ctx, &mut store, lsn_fail)?;
        let drained = recover_and_validate(&ctx, &fail_flag, &mut store)?;
        drop(store);
        reopen_and_validate(&ctx, &tag, &drained)?;
        ctx.delete_stream();
        Ok(())
    }

    fn seed_iteration_journal(iter: usize, paths: &IterPaths) -> Result<(), String> {
        let mut seed = EventStore::<Order>::create(&paths.journal)
            .map_err(|e| format!("iter {iter}: EventStore::create: {e:?}"))?;
        for n in 0..SEED_EVENTS_PER_ITER {
            let id = u64::try_from(n).expect("seed index fits u64");
            let _ = seed
                .writer()
                .begin(Order { id })
                .map_err(|e| format!("iter {iter}: seed begin id={id}: {e:?}"))?;
        }
        let _ = seed
            .writer()
            .sync()
            .map_err(|e| format!("iter {iter}: seed sync: {e:?}"))?;
        Ok(())
    }

    fn open_iteration_store(
        ctx: &IterCtx<'_>,
        tag: &str,
        fail_flag: &Arc<std::sync::Mutex<bool>>,
    ) -> Result<EventStore<Order>, String> {
        let cfg = live_config(tag, ctx.rt, ctx.server);
        let handle = JetStreamBackend::open(cfg);
        let inner = JetStreamFrontierPublisher::open(handle);
        let publisher = FailThenForwardPublisher::new(inner, Arc::clone(fail_flag));
        EventStore::<Order>::open_with_publisher(
            &ctx.paths.journal,
            ctx.paths.sidecar.clone(),
            ctx.stream_name.clone(),
            1,
            Box::new(publisher),
        )
        .map_err(|e| format!("iter {}: open_with_publisher: {e:?}", ctx.iter))
    }

    fn assert_failure_phase(
        ctx: &IterCtx<'_>,
        store: &mut EventStore<Order>,
        lsn_fail: pardosa::store::Lsn,
    ) -> Result<(), String> {
        if store.writer().acked_lsn() != Some(lsn_fail) {
            return Err(format!(
                "iter {}: failure-phase acked_lsn {:?} != returned lsn {lsn_fail:?}",
                ctx.iter,
                store.writer().acked_lsn(),
            ));
        }
        if ctx.paths.sidecar.exists() {
            ctx.delete_stream();
            return Err(format!(
                "iter {}: watermark sidecar advanced while every publish failed \
                 (ADR-0016 §D5 violated)",
                ctx.iter,
            ));
        }
        match read_stream_payloads(ctx.rt, ctx.server, &ctx.stream_name, &ctx.subject) {
            Err(ReadError::StreamNotFound) => Ok(()),
            Ok(payloads) if payloads.is_empty() => Ok(()),
            Ok(payloads) => {
                ctx.delete_stream();
                Err(format!(
                    "iter {}: JetStream stream holds {} payload(s) after failure-phase sync, \
                     expected 0 (ADR-0015 §D2 fence violated)",
                    ctx.iter,
                    payloads.len(),
                ))
            }
            Err(ReadError::Other(e)) => {
                ctx.delete_stream();
                Err(format!(
                    "iter {}: failure-phase read_stream_payloads returned unexpected error: {e}",
                    ctx.iter,
                ))
            }
        }
    }

    fn recover_and_validate(
        ctx: &IterCtx<'_>,
        fail_flag: &Arc<std::sync::Mutex<bool>>,
        store: &mut EventStore<Order>,
    ) -> Result<Vec<Vec<u8>>, String> {
        *fail_flag.lock().expect("fail flag mutex") = false;
        let _ = store
            .writer()
            .sync()
            .map_err(|e| format!("iter {}: recovery-phase sync returned Err: {e:?}", ctx.iter))?;
        if !ctx.paths.sidecar.exists() {
            ctx.delete_stream();
            return Err(format!(
                "iter {}: watermark sidecar must exist after successful drain \
                 (ADR-0016 §D5)",
                ctx.iter,
            ));
        }
        let drained = read_stream_payloads(ctx.rt, ctx.server, &ctx.stream_name, &ctx.subject)
            .map_err(|e| format!("iter {}: read_stream_payloads (recovery): {e:?}", ctx.iter))?;
        validate_drained_payloads(ctx, &drained)?;
        Ok(drained)
    }

    fn validate_drained_payloads(ctx: &IterCtx<'_>, drained: &[Vec<u8>]) -> Result<(), String> {
        if drained.len() != SEED_EVENTS_PER_ITER {
            ctx.delete_stream();
            return Err(format!(
                "iter {}: recovered {} payload(s) from JetStream, expected {} \
                 (ADR-0015 §D3 ordering or loss)",
                ctx.iter,
                drained.len(),
                SEED_EVENTS_PER_ITER,
            ));
        }
        for (n, payload) in drained.iter().enumerate() {
            if payload.len() != 32 {
                ctx.delete_stream();
                return Err(format!(
                    "iter {}: payload[{n}] has {} bytes, expected 32 (BLAKE3 frontier)",
                    ctx.iter,
                    payload.len(),
                ));
            }
        }
        let unique: HashSet<Vec<u8>> = drained.iter().cloned().collect();
        if unique.len() != drained.len() {
            ctx.delete_stream();
            return Err(format!(
                "iter {}: duplicate frontier payloads in JetStream stream \
                 (unique={}, total={}); rolling BLAKE3 invariant violated (ADR-0004)",
                ctx.iter,
                unique.len(),
                drained.len(),
            ));
        }
        Ok(())
    }

    fn reopen_and_validate(
        ctx: &IterCtx<'_>,
        tag: &str,
        drained: &[Vec<u8>],
    ) -> Result<(), String> {
        let republisher_cfg = live_config(tag, ctx.rt, ctx.server);
        let republisher_handle = JetStreamBackend::open(republisher_cfg);
        let republisher = JetStreamFrontierPublisher::open(republisher_handle);
        let mut reopened = EventStore::<Order>::open_with_publisher(
            &ctx.paths.journal,
            ctx.paths.sidecar.clone(),
            ctx.stream_name.clone(),
            1,
            Box::new(republisher),
        )
        .map_err(|e| format!("iter {}: reopen_with_publisher: {e:?}", ctx.iter))?;
        let _ = reopened
            .writer()
            .sync()
            .map_err(|e| format!("iter {}: reopen sync returned Err: {e:?}", ctx.iter))?;
        let after_reopen = read_stream_payloads(ctx.rt, ctx.server, &ctx.stream_name, &ctx.subject)
            .map_err(|e| format!("iter {}: read_stream_payloads (reopen): {e:?}", ctx.iter))?;
        validate_reopen_payloads(ctx, drained, &after_reopen)?;
        drop(reopened);
        Ok(())
    }

    fn validate_reopen_payloads(
        ctx: &IterCtx<'_>,
        drained: &[Vec<u8>],
        after_reopen: &[Vec<u8>],
    ) -> Result<(), String> {
        if after_reopen.len() != SEED_EVENTS_PER_ITER {
            ctx.delete_stream();
            return Err(format!(
                "iter {}: reopen produced {} payload(s), expected {} unchanged \
                 (ADR-0016 §D7 watermark must suppress duplicate republish)",
                ctx.iter,
                after_reopen.len(),
                SEED_EVENTS_PER_ITER,
            ));
        }
        if after_reopen != drained {
            ctx.delete_stream();
            return Err(format!(
                "iter {}: reopen mutated stream contents \
                 (ADR-0016 §D7 violated; bytes-by-position mismatch)",
                ctx.iter,
            ));
        }
        Ok(())
    }
    #[test]
    #[ignore = "live JetStream 100x publisher failure/recovery soak (mission nats-phase5-publisher-soak-02); requires nats-server matching tools/.nats-server-version on PATH"]
    fn publisher_failure_recovery_soak_100x_with_jetstream() {
        let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
        let rt = Runtime::new().expect("tokio runtime");
        let mut failures: Vec<String> = Vec::new();
        for iter in 0..SOAK_ITERATIONS {
            if let Err(msg) = run_iteration(iter, &rt, &server) {
                eprintln!("[soak02 iter {iter}] FAIL: {msg}");
                failures.push(msg);
            }
        }
        eprintln!(
            "soak02: N={SOAK_ITERATIONS} failure-phase / recovery / reopen cycles; failures={}",
            failures.len(),
        );
        assert!(
            failures.is_empty(),
            "publisher_failure_recovery_soak_100x_with_jetstream: {} iteration(s) failed; \
             first failure: {}",
            failures.len(),
            failures.first().cloned().unwrap_or_default(),
        );
    }
}
