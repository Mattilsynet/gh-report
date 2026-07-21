//! TEST 2 (bd `adr-fmt-krndg`, epic `adr-fmt-3dvym`): the load-bearing
//! true-reject counter-proof for the pgno arm.
//!
//! Builds a real two-event chained fiber through the public write API,
//! then re-emits the SAME `.pgno` container with event 0's trailing
//! payload byte flipped via [`pardosa_file::Writer`] — which recomputes
//! the per-message `xxh64` checksum honestly over the tampered bytes
//! (the checksum is not a MAC; ADR-0006 §D4). Event 1's `precursor_hash`
//! was captured at write time against the ORIGINAL event 0 bytes, so the
//! tamper is isolated to the precursor layer: container/file integrity
//! for message 0 still passes at `Reader::read_message`, but
//! `verify_precursor` / `precursor_would_fail` sees a mismatch.
//!
//! Asserts the REAL [`EventStore::open_with_backend`] entry point:
//! under `PARDOSA_PRECURSOR_CHECK_MODE=enforce` the boot fails with a
//! matchable `CheckedReplayKind::PrecursorHashMismatch` (PGN-0016:R9,
//! no string-flatten); under the default (env unset, `ObserveOnly`)
//! the same tampered stream boots and emits the
//! `precursor_check_would_fail` warn without rejecting.
use pardosa::store::replay::{CheckedReplayKind, Error as ReplayError};
use pardosa::store::{
    Event, EventStore, GenomeSafe, HasEventSchemaSource, PardosaError, PgnoBackend,
};
use pardosa_file::{Reader, Writer};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Tagged {
    seq: u64,
}

impl HasEventSchemaSource for Tagged {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}

const TAMPER_FIXTURE_PATH_ENV: &str = "PARDOSA_TAMPER_FIXTURE_PATH";

fn pgno_path(dir: &tempfile::TempDir, name: &str) -> PathBuf {
    dir.path().join(format!("{name}.pgno"))
}

/// Build a real 2-event chained fiber via the public write API, then
/// overwrite the same `.pgno` on disk with event 0's trailing payload
/// byte flipped, re-emitted through [`Writer`] so the message checksum
/// is recomputed honestly over the tampered bytes.
fn build_tampered_chain(path: &Path) {
    {
        let mut store = EventStore::<Tagged>::create(path).expect("create scratch pgno");
        let first = store
            .writer()
            .begin(Tagged { seq: 1 })
            .expect("begin genesis event");
        let _second = store
            .writer()
            .append(first.fiber(), Tagged { seq: 2 })
            .expect("append chained event referencing genesis as precursor");
        let _lsn = store.writer().sync().expect("sync clean chain");
    }
    let schema_hash = Event::<Tagged>::ENVELOPE_HASH;
    let mut messages: Vec<Vec<u8>> = {
        let file = std::fs::File::open(path).expect("reopen clean pgno for raw read");
        let mut reader = Reader::open(file).expect("open pgno reader");
        let count = reader.index().len();
        (0..count)
            .map(|i| reader.read_message(i).expect("read raw canonical message"))
            .collect()
    };
    assert_eq!(
        messages.len(),
        2,
        "fixture is a two-event chained-fiber log"
    );
    let last = messages[0].len() - 1;
    messages[0][last] ^= 0xFF;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .expect("reopen for tamper rewrite");
    {
        let mut writer = Writer::new(&mut file, schema_hash);
        for msg in &messages {
            writer.write_message(msg).expect("re-write message body");
        }
        writer.finish().expect("finish tampered container");
    }
    file.flush().expect("flush tampered container");
}

fn run_isolated_worker(worker_name: &str, tampered_path: &Path) {
    let exe = std::env::current_exe().expect("current test binary path");
    let output = std::process::Command::new(exe)
        .arg(worker_name)
        .arg("--exact")
        .arg("--ignored")
        .env("PARDOSA_PRECURSOR_CHECK_MODE", "enforce")
        .env(TAMPER_FIXTURE_PATH_ENV, tampered_path)
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
fn enforce_real_boot_rejects_tampered_pgno_stream_via_open_with_backend() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = pgno_path(&dir, "enforce-reject");
    build_tampered_chain(&path);
    run_isolated_worker("enforce_worker_pgno_rejects_via_open_with_backend", &path);
}

#[test]
#[ignore = "isolated worker spawned as a subprocess by \
            enforce_real_boot_rejects_tampered_pgno_stream_via_open_with_backend \
            with PARDOSA_PRECURSOR_CHECK_MODE=enforce set on the child process only; \
            never runs under a plain `cargo test`"]
fn enforce_worker_pgno_rejects_via_open_with_backend() {
    let path = PathBuf::from(
        std::env::var(TAMPER_FIXTURE_PATH_ENV).expect("fixture path env set by parent test"),
    );
    let backend = PgnoBackend::open(&path);
    let err = match EventStore::<Tagged>::open_with_backend(backend) {
        Ok(_store) => {
            panic!("Enforce must reject the tampered stream at the real open_with_backend boot")
        }
        Err(err) => err,
    };
    match err {
        PardosaError::CursorRead { source } => match *source {
            ReplayError::CheckedReplay {
                kind: CheckedReplayKind::PrecursorHashMismatch { .. },
            } => {}
            other => panic!(
                "expected CheckedReplay::PrecursorHashMismatch via real \
                 open_with_backend under Enforce, got: {other:?}"
            ),
        },
        other => panic!("expected PardosaError::CursorRead wrapping CheckedReplay, got: {other:?}"),
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
fn observe_only_real_boot_boots_tampered_pgno_stream_without_rejecting() {
    assert!(
        std::env::var("PARDOSA_PRECURSOR_CHECK_MODE").is_err(),
        "this test asserts the default ObserveOnly path; env must stay unset here"
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let path = pgno_path(&dir, "observe-only-boot");
    build_tampered_chain(&path);

    let (store, captured) = capture_tracing(|| {
        let backend = PgnoBackend::open(&path);
        EventStore::<Tagged>::open_with_backend(backend)
            .expect("ObserveOnly must boot the same tampered stream without rejecting")
    });

    assert_ne!(
        store.reader().frontier(),
        pardosa::store::Frontier::GENESIS,
        "ObserveOnly boot must observe the committed (tampered) chain in the frontier"
    );
    assert!(
        captured.contains("precursor_check_would_fail"),
        "ObserveOnly must emit the precursor_check_would_fail warn for the tampered \
         chain even though it does not reject; captured tracing: {captured}"
    );
}
