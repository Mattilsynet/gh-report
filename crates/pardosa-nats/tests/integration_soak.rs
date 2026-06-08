//! Phase 5 paired-flake-window integration soak.
//!
//! `JetStream` flake rate over window `N` must be no worse than
//! `.pgno` over the same window. Per-iteration properties:
//! monotonic `append`, byte-identical fidelity (distinct chunks
//! to avoid dedup), `sync ≥ last append`, idempotent re-`sync`.
//!
//! ```text
//! cargo test -p pardosa-nats --test integration_soak \
//!   -- --ignored --nocapture
//! ```
//!
//! `--ignored`: pinned `nats-server` required (Phase 1.5 §8.4).
//! Default `N = 50`.
//!
//! Not covered: the 100× publisher-failure / recovery soak —
//! needs a `FrontierPublisher` over `JetStream` and
//! `open_with_backend(JetStreamBackendAdapter)`. Status in
//! `docs/nats-jetstream-roadmap.md` §Phase 5.
mod support;
use pardosa_nats::{JetStreamBackend, JetStreamConfig, JetStreamHandle, RuntimeHandle};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use support::LiveNatsServer;
use tokio::runtime::Runtime;
/// Phase 5 window size (roadmap §Phase 5 line 692 — default
/// `N = 50` unless a Phase 5 test-plan PR justifies another value).
const PHASE5_WINDOW_N: usize = 50;
/// One-shot per-iteration tag so streams, subjects, durable
/// consumers, and `.pgno` files never collide across iterations
/// in the soak loop or across parallel test runs.
fn unique_iter_tag(prefix: &str, iter: usize) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let pid = std::process::id();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{pid}_{nanos}_{iter}_{seq}")
}
/// Tally of contract-property failures observed in a single
/// soak window for one backend.
#[derive(Debug, Default)]
struct FailureTally {
    monotonic: u32,
    byte_fidelity: u32,
    sync_floor: u32,
    sync_idempotent: u32,
}
impl FailureTally {
    const fn total(&self) -> u32 {
        self.monotonic + self.byte_fidelity + self.sync_floor + self.sync_idempotent
    }
}
impl std::fmt::Display for FailureTally {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "total={} (monotonic={}, byte_fidelity={}, sync_floor={}, sync_idempotent={})",
            self.total(),
            self.monotonic,
            self.byte_fidelity,
            self.sync_floor,
            self.sync_idempotent,
        )
    }
}
fn record_fail(counter: &mut u32, label: &str, iter: usize, why: impl std::fmt::Display) {
    *counter = counter.saturating_add(1);
    eprintln!("[soak iter {iter}] FAIL {label}: {why}");
}
struct PgnoTempFile {
    path: PathBuf,
    file: std::fs::File,
}
impl PgnoTempFile {
    fn new(tag: &str) -> Result<Self, std::io::Error> {
        let mut p = std::env::temp_dir();
        p.push(format!("pardosa-soak-{tag}.pgno"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&p)?;
        Ok(Self { path: p, file })
    }
    fn append_bytes(&mut self, bytes: &[u8]) -> Result<u64, String> {
        self.file
            .write_all(bytes)
            .map_err(|e| format!("write_all: {e}"))?;
        self.file
            .stream_position()
            .map_err(|e| format!("stream_position: {e}"))
    }
    fn sync_position(&mut self) -> Result<u64, String> {
        self.file
            .sync_data()
            .map_err(|e| format!("sync_data: {e}"))?;
        self.file
            .stream_position()
            .map_err(|e| format!("stream_position: {e}"))
    }
    fn read_all_from_disk(&mut self) -> Result<Vec<u8>, String> {
        self.file
            .sync_data()
            .map_err(|e| format!("sync_data before read: {e}"))?;
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|e| format!("seek 0: {e}"))?;
        let mut buf = Vec::new();
        self.file
            .read_to_end(&mut buf)
            .map_err(|e| format!("read_to_end: {e}"))?;
        self.file
            .seek(SeekFrom::End(0))
            .map_err(|e| format!("seek end: {e}"))?;
        Ok(buf)
    }
}
impl Drop for PgnoTempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
fn pgno_check_monotonic(iter: usize, tally: &mut FailureTally) {
    let tag = unique_iter_tag("pgno-mono", iter);
    let mut sink = match PgnoTempFile::new(&tag) {
        Ok(s) => s,
        Err(e) => {
            record_fail(
                &mut tally.monotonic,
                "pgno.monotonic.setup",
                iter,
                e.to_string(),
            );
            return;
        }
    };
    let a = match sink.append_bytes(b"a") {
        Ok(v) => v,
        Err(e) => {
            record_fail(
                &mut tally.monotonic,
                "pgno.monotonic",
                iter,
                format!("append 1: {e}"),
            );
            return;
        }
    };
    let b = match sink.append_bytes(b"bb") {
        Ok(v) => v,
        Err(e) => {
            record_fail(
                &mut tally.monotonic,
                "pgno.monotonic",
                iter,
                format!("append 2: {e}"),
            );
            return;
        }
    };
    let c = match sink.append_bytes(b"ccc") {
        Ok(v) => v,
        Err(e) => {
            record_fail(
                &mut tally.monotonic,
                "pgno.monotonic",
                iter,
                format!("append 3: {e}"),
            );
            return;
        }
    };
    if !(a < b && b < c) {
        record_fail(
            &mut tally.monotonic,
            "pgno.monotonic",
            iter,
            format!("non-monotonic a={a} b={b} c={c}"),
        );
    }
}
fn pgno_check_byte_fidelity(iter: usize, tally: &mut FailureTally) {
    let tag = unique_iter_tag("pgno-bytes", iter);
    let mut sink = match PgnoTempFile::new(&tag) {
        Ok(s) => s,
        Err(e) => {
            record_fail(
                &mut tally.byte_fidelity,
                "pgno.byte_fidelity.setup",
                iter,
                e.to_string(),
            );
            return;
        }
    };
    let chunks: &[&[u8]] = &[b"alpha", b"-sep1-", b"beta", b"-sep2-", b"gamma"];
    let mut expected: Vec<u8> = Vec::new();
    for chunk in chunks {
        if let Err(e) = sink.append_bytes(chunk) {
            record_fail(
                &mut tally.byte_fidelity,
                "pgno.byte_fidelity",
                iter,
                format!("append chunk len={}: {e}", chunk.len()),
            );
            return;
        }
        expected.extend_from_slice(chunk);
    }
    let observed = match sink.read_all_from_disk() {
        Ok(v) => v,
        Err(e) => {
            record_fail(
                &mut tally.byte_fidelity,
                "pgno.byte_fidelity",
                iter,
                format!("read_back: {e}"),
            );
            return;
        }
    };
    if observed != expected {
        record_fail(
            &mut tally.byte_fidelity,
            "pgno.byte_fidelity",
            iter,
            format!(
                "byte mismatch expected={} observed={}",
                expected.len(),
                observed.len()
            ),
        );
    }
}
fn pgno_check_sync_floor(iter: usize, tally: &mut FailureTally) {
    let tag = unique_iter_tag("pgno-floor", iter);
    let mut sink = match PgnoTempFile::new(&tag) {
        Ok(s) => s,
        Err(e) => {
            record_fail(
                &mut tally.sync_floor,
                "pgno.sync_floor.setup",
                iter,
                e.to_string(),
            );
            return;
        }
    };
    let appended = match sink.append_bytes(b"to-be-fenced") {
        Ok(v) => v,
        Err(e) => {
            record_fail(
                &mut tally.sync_floor,
                "pgno.sync_floor",
                iter,
                format!("append: {e}"),
            );
            return;
        }
    };
    let synced = match sink.sync_position() {
        Ok(v) => v,
        Err(e) => {
            record_fail(
                &mut tally.sync_floor,
                "pgno.sync_floor",
                iter,
                format!("sync: {e}"),
            );
            return;
        }
    };
    if synced < appended {
        record_fail(
            &mut tally.sync_floor,
            "pgno.sync_floor",
            iter,
            format!("regression appended={appended} synced={synced}"),
        );
    }
}
fn pgno_check_sync_idempotent(iter: usize, tally: &mut FailureTally) {
    let tag = unique_iter_tag("pgno-idemp", iter);
    let mut sink = match PgnoTempFile::new(&tag) {
        Ok(s) => s,
        Err(e) => {
            record_fail(
                &mut tally.sync_idempotent,
                "pgno.sync_idempotent.setup",
                iter,
                e.to_string(),
            );
            return;
        }
    };
    if let Err(e) = sink.append_bytes(b"once") {
        record_fail(
            &mut tally.sync_idempotent,
            "pgno.sync_idempotent",
            iter,
            format!("pre-append: {e}"),
        );
        return;
    }
    let first = match sink.sync_position() {
        Ok(v) => v,
        Err(e) => {
            record_fail(
                &mut tally.sync_idempotent,
                "pgno.sync_idempotent",
                iter,
                format!("first sync: {e}"),
            );
            return;
        }
    };
    let second = match sink.sync_position() {
        Ok(v) => v,
        Err(e) => {
            record_fail(
                &mut tally.sync_idempotent,
                "pgno.sync_idempotent",
                iter,
                format!("second sync: {e}"),
            );
            return;
        }
    };
    if first != second {
        record_fail(
            &mut tally.sync_idempotent,
            "pgno.sync_idempotent",
            iter,
            format!("not idempotent first={first} second={second}"),
        );
    }
}
struct JetStreamIterContext {
    rt: Runtime,
    server: Arc<LiveNatsServer>,
}
impl JetStreamIterContext {
    fn acquire() -> Result<Self, String> {
        let server = std::panic::catch_unwind(LiveNatsServer::acquire)
            .map_err(|_| "LiveNatsServer::acquire panicked".to_string())?;
        let rt = Runtime::new().map_err(|e| format!("tokio runtime: {e}"))?;
        Ok(Self { rt, server })
    }
    fn build_handle(&self, stream_name: &str, subject: &str, durable: &str) -> JetStreamHandle {
        let cfg = JetStreamConfig::builder()
            .stream_name(stream_name.to_string())
            .subject(subject.to_string())
            .durable_consumer(durable.to_string())
            .runtime_handle(RuntimeHandle::from_tokio(self.rt.handle().clone()))
            .nats_url(self.server.url().to_owned())
            .build()
            .expect("config valid for soak iteration");
        JetStreamBackend::open(cfg)
    }
    fn delete_stream(&self, stream_name: &str) {
        let server = Arc::clone(&self.server);
        let stream_name = stream_name.to_string();
        self.rt.block_on(async move {
            let Ok(client) = async_nats::connect(server.url()).await else {
                return;
            };
            let js = async_nats::jetstream::new(client);
            let _ = js.delete_stream(&stream_name).await;
        });
    }
}
fn jetstream_check_monotonic(ctx: &JetStreamIterContext, iter: usize, tally: &mut FailureTally) {
    let tag = unique_iter_tag("js-mono", iter);
    let stream_name = format!("PARDOSA_SOAK_MONO_{tag}");
    let handle = ctx.build_handle(
        &stream_name,
        &format!("pardosa.soak.mono.{tag}"),
        &format!("pardosa-soak-c-mono-{tag}"),
    );
    let a = match handle.append(b"a") {
        Ok(p) => p.as_u64(),
        Err(e) => {
            record_fail(
                &mut tally.monotonic,
                "jetstream.monotonic",
                iter,
                format!("append 1: {e}"),
            );
            ctx.delete_stream(&stream_name);
            return;
        }
    };
    let b = match handle.append(b"bb") {
        Ok(p) => p.as_u64(),
        Err(e) => {
            record_fail(
                &mut tally.monotonic,
                "jetstream.monotonic",
                iter,
                format!("append 2: {e}"),
            );
            ctx.delete_stream(&stream_name);
            return;
        }
    };
    let c = match handle.append(b"ccc") {
        Ok(p) => p.as_u64(),
        Err(e) => {
            record_fail(
                &mut tally.monotonic,
                "jetstream.monotonic",
                iter,
                format!("append 3: {e}"),
            );
            ctx.delete_stream(&stream_name);
            return;
        }
    };
    if !(a < b && b < c) {
        record_fail(
            &mut tally.monotonic,
            "jetstream.monotonic",
            iter,
            format!("non-monotonic a={a} b={b} c={c}"),
        );
    }
    ctx.delete_stream(&stream_name);
}
fn jetstream_check_byte_fidelity(
    ctx: &JetStreamIterContext,
    iter: usize,
    tally: &mut FailureTally,
) {
    let tag = unique_iter_tag("js-bytes", iter);
    let stream_name = format!("PARDOSA_SOAK_BYTES_{tag}");
    let subject = format!("pardosa.soak.bytes.{tag}");
    let handle = ctx.build_handle(
        &stream_name,
        &subject,
        &format!("pardosa-soak-c-bytes-{tag}"),
    );
    let chunks: &[&[u8]] = &[b"alpha", b"-sep1-", b"beta", b"-sep2-", b"gamma"];
    let mut expected: Vec<u8> = Vec::new();
    for chunk in chunks {
        if let Err(e) = handle.append(chunk) {
            record_fail(
                &mut tally.byte_fidelity,
                "jetstream.byte_fidelity",
                iter,
                format!("append chunk len={}: {e}", chunk.len()),
            );
            ctx.delete_stream(&stream_name);
            return;
        }
        expected.extend_from_slice(chunk);
    }
    let records = match handle.replay_all() {
        Ok(v) => v,
        Err(e) => {
            record_fail(
                &mut tally.byte_fidelity,
                "jetstream.byte_fidelity",
                iter,
                format!("replay_all: {e}"),
            );
            ctx.delete_stream(&stream_name);
            return;
        }
    };
    let mut observed: Vec<u8> = Vec::new();
    for r in &records {
        observed.extend_from_slice(&r.payload);
    }
    if observed != expected {
        record_fail(
            &mut tally.byte_fidelity,
            "jetstream.byte_fidelity",
            iter,
            format!(
                "byte mismatch expected={} observed={}",
                expected.len(),
                observed.len()
            ),
        );
    }
    ctx.delete_stream(&stream_name);
}
fn jetstream_check_sync_floor(ctx: &JetStreamIterContext, iter: usize, tally: &mut FailureTally) {
    let tag = unique_iter_tag("js-floor", iter);
    let stream_name = format!("PARDOSA_SOAK_FLOOR_{tag}");
    let handle = ctx.build_handle(
        &stream_name,
        &format!("pardosa.soak.floor.{tag}"),
        &format!("pardosa-soak-c-floor-{tag}"),
    );
    let appended = match handle.append(b"to-be-fenced") {
        Ok(p) => p.as_u64(),
        Err(e) => {
            record_fail(
                &mut tally.sync_floor,
                "jetstream.sync_floor",
                iter,
                format!("append: {e}"),
            );
            ctx.delete_stream(&stream_name);
            return;
        }
    };
    let synced = match handle.sync() {
        Ok(p) => p.as_u64(),
        Err(e) => {
            record_fail(
                &mut tally.sync_floor,
                "jetstream.sync_floor",
                iter,
                format!("sync: {e}"),
            );
            ctx.delete_stream(&stream_name);
            return;
        }
    };
    if synced < appended {
        record_fail(
            &mut tally.sync_floor,
            "jetstream.sync_floor",
            iter,
            format!("regression appended={appended} synced={synced}"),
        );
    }
    ctx.delete_stream(&stream_name);
}
fn jetstream_check_sync_idempotent(
    ctx: &JetStreamIterContext,
    iter: usize,
    tally: &mut FailureTally,
) {
    let tag = unique_iter_tag("js-idemp", iter);
    let stream_name = format!("PARDOSA_SOAK_IDEMP_{tag}");
    let handle = ctx.build_handle(
        &stream_name,
        &format!("pardosa.soak.idemp.{tag}"),
        &format!("pardosa-soak-c-idemp-{tag}"),
    );
    if let Err(e) = handle.append(b"once") {
        record_fail(
            &mut tally.sync_idempotent,
            "jetstream.sync_idempotent",
            iter,
            format!("pre-append: {e}"),
        );
        ctx.delete_stream(&stream_name);
        return;
    }
    let first = match handle.sync() {
        Ok(p) => p.as_u64(),
        Err(e) => {
            record_fail(
                &mut tally.sync_idempotent,
                "jetstream.sync_idempotent",
                iter,
                format!("first sync: {e}"),
            );
            ctx.delete_stream(&stream_name);
            return;
        }
    };
    let second = match handle.sync() {
        Ok(p) => p.as_u64(),
        Err(e) => {
            record_fail(
                &mut tally.sync_idempotent,
                "jetstream.sync_idempotent",
                iter,
                format!("second sync: {e}"),
            );
            ctx.delete_stream(&stream_name);
            return;
        }
    };
    if first != second {
        record_fail(
            &mut tally.sync_idempotent,
            "jetstream.sync_idempotent",
            iter,
            format!("not idempotent first={first} second={second}"),
        );
    }
    ctx.delete_stream(&stream_name);
}
fn run_pgno_iteration(iter: usize, tally: &mut FailureTally) {
    pgno_check_monotonic(iter, tally);
    pgno_check_byte_fidelity(iter, tally);
    pgno_check_sync_floor(iter, tally);
    pgno_check_sync_idempotent(iter, tally);
}
fn run_jetstream_iteration(ctx: &JetStreamIterContext, iter: usize, tally: &mut FailureTally) {
    jetstream_check_monotonic(ctx, iter, tally);
    jetstream_check_byte_fidelity(ctx, iter, tally);
    jetstream_check_sync_floor(ctx, iter, tally);
    jetstream_check_sync_idempotent(ctx, iter, tally);
}
#[test]
#[ignore = "Phase 5 paired flake-window soak — requires nats-server (mission nats-recovery-05-integration-soak)"]
fn phase5_paired_flake_window_pgno_vs_jetstream() {
    let ctx = JetStreamIterContext::acquire()
        .expect("JetStream context (live nats-server harness) must be reachable");
    let mut pgno = FailureTally::default();
    let mut jetstream = FailureTally::default();
    for iter in 0..PHASE5_WINDOW_N {
        run_pgno_iteration(iter, &mut pgno);
        run_jetstream_iteration(&ctx, iter, &mut jetstream);
    }
    eprintln!(
        "Phase 5 paired flake window (N={PHASE5_WINDOW_N}):\n  .pgno     : {pgno}\n  jetstream : {jetstream}"
    );
    assert!(
        jetstream.total() <= pgno.total(),
        "Phase 5 gate (roadmap line 692): JetStream-attributable failures ({jetstream}) must be \
         ≤ .pgno-attributable failures ({pgno}) over the same N={PHASE5_WINDOW_N} window"
    );
}
