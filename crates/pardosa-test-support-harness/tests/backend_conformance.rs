//! Shared `BackendSink` contract conformance suite (ADR-0022 §D2 /
//! §D11).
//!
//! Runs the contract against both admitted backends:
//! [`PgnoFileSink`] over a `.pgno`-shaped temp file and
//! [`InMemoryBackend`] via `test-support`. Assertions
//! parameterised over `S: BackendSink`.
//!
//! Coverage: monotonic `append` (§D2); byte-identical fidelity;
//! `sync ≥ last append` (§D2 fence); idempotent re-`sync`.
use pardosa::store::test_support::InMemoryBackend;
use pardosa::store::{AckPosition, BackendSink, PgnoFileSink};
use std::path::{Path, PathBuf};
fn assert_append_returns_monotonic_ack_position<S: BackendSink>(sink: &mut S) {
    let p1 = sink.append(b"a").expect("append 1");
    let p2 = sink.append(b"bb").expect("append 2");
    let p3 = sink.append(b"ccc").expect("append 3");
    assert!(
        p1 < p2,
        "first AckPosition must be strictly less than second"
    );
    assert!(
        p2 < p3,
        "second AckPosition must be strictly less than third"
    );
}
fn assert_append_carries_canonical_bytes_unchanged<S, R>(sink: &mut S, mut read_back: R)
where
    S: BackendSink,
    R: FnMut(&mut S) -> Vec<u8>,
{
    let chunks: &[&[u8]] = &[b"alpha", b"-", b"beta", b"-", b"gamma"];
    let mut expected: Vec<u8> = Vec::new();
    for chunk in chunks {
        let _ = sink.append(chunk).expect("append chunk");
        expected.extend_from_slice(chunk);
    }
    let _ = sink.sync().expect("sync before read-back");
    let observed = read_back(sink);
    assert_eq!(
        observed, expected,
        "backend must carry canonical bytes unchanged across appends"
    );
}
fn assert_sync_returns_position_at_or_beyond_last_append<S: BackendSink>(sink: &mut S) {
    let appended = sink.append(b"to-be-fenced").expect("append");
    let synced = sink.sync().expect("sync");
    assert!(
        synced >= appended,
        "sync AckPosition ({synced:?}) must be at or beyond last append ({appended:?})"
    );
}
fn assert_sync_no_op_after_prior_sync_is_idempotent<S: BackendSink>(sink: &mut S) {
    let _ = sink.append(b"once").expect("append");
    let first = sink.sync().expect("first sync");
    let second = sink.sync().expect("second sync");
    assert_eq!(
        first, second,
        "no-op sync (no intervening append) must be idempotent"
    );
}
fn unique_tmp_pgno(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut p = std::env::temp_dir();
    p.push(format!(
        "pardosa-backend-conformance-{tag}-{pid}-{nanos}-{seq}.pgno"
    ));
    p
}
struct PgnoTempFile {
    path: PathBuf,
}
impl PgnoTempFile {
    fn new(tag: &str) -> Self {
        Self {
            path: unique_tmp_pgno(tag),
        }
    }
    fn open_sink(&self) -> PgnoFileSink<std::fs::File> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.path)
            .expect("open .pgno temp file");
        PgnoFileSink::<std::fs::File>::new(file)
    }
    fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for PgnoTempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
#[test]
fn pgno_file_sink_append_returns_monotonic_ack_position() {
    let tmp = PgnoTempFile::new("monotonic");
    let mut sink = tmp.open_sink();
    assert_append_returns_monotonic_ack_position(&mut sink);
}
#[test]
fn pgno_file_sink_carries_canonical_bytes_unchanged() {
    let tmp = PgnoTempFile::new("bytes");
    let mut sink = tmp.open_sink();
    let path = tmp.path().to_path_buf();
    assert_append_carries_canonical_bytes_unchanged(&mut sink, move |_sink| {
        std::fs::read(&path).expect("read .pgno back from disk")
    });
}
#[test]
fn pgno_file_sink_sync_returns_position_at_or_beyond_last_append() {
    let tmp = PgnoTempFile::new("syncpos");
    let mut sink = tmp.open_sink();
    assert_sync_returns_position_at_or_beyond_last_append(&mut sink);
}
#[test]
fn pgno_file_sink_sync_no_op_is_idempotent() {
    let tmp = PgnoTempFile::new("idempotent");
    let mut sink = tmp.open_sink();
    assert_sync_no_op_after_prior_sync_is_idempotent(&mut sink);
}
#[test]
fn in_memory_backend_append_returns_monotonic_ack_position() {
    let mut sink = InMemoryBackend::new();
    assert_append_returns_monotonic_ack_position(&mut sink);
}
#[test]
fn in_memory_backend_carries_canonical_bytes_unchanged() {
    let mut sink = InMemoryBackend::new();
    assert_append_carries_canonical_bytes_unchanged(&mut sink, |sink| sink.bytes().to_vec());
}
#[test]
fn in_memory_backend_sync_returns_position_at_or_beyond_last_append() {
    let mut sink = InMemoryBackend::new();
    assert_sync_returns_position_at_or_beyond_last_append(&mut sink);
}
#[test]
fn in_memory_backend_sync_no_op_is_idempotent() {
    let mut sink = InMemoryBackend::new();
    assert_sync_no_op_after_prior_sync_is_idempotent(&mut sink);
}
#[test]
fn ack_position_is_uniform_return_type_across_backends() {
    fn require_ack<S: BackendSink>(sink: &mut S) {
        let _: AckPosition = sink.append(b"x").expect("append");
        let _: AckPosition = sink.sync().expect("sync");
    }
    let tmp = PgnoTempFile::new("typecheck");
    let mut pgno = tmp.open_sink();
    require_ack(&mut pgno);
    let mut mem = InMemoryBackend::new();
    require_ack(&mut mem);
}
