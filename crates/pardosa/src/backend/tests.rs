use super::*;
use std::io::Cursor;
#[test]
fn backend_rehydrate_from_pgno_bytes_round_trips_without_opening_a_file() {
    use crate::dragline::DEFAULT_ANCHOR_BUFFER_CAP;
    use crate::dragline::Line;
    use crate::persist::persist_with_source;
    let mut line: Line<u64> = Line::with_anchor_config(
        "backend-rehydrate-bytes".to_owned(),
        1,
        DEFAULT_ANCHOR_BUFFER_CAP,
    );
    for i in 0..5u64 {
        let _ = line.create(i).expect("commit");
    }
    let original_line: Vec<u64> = line.read_line().iter().map(|e| *e.domain_event()).collect();
    let mut sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    persist_with_source(&line, &mut sink, None).expect("persist to in-memory sink");
    let pgno_bytes: Vec<u8> = sink.into_inner();
    assert!(
        !pgno_bytes.is_empty(),
        "preflight: persist_with_source must produce a non-empty .pgno blob",
    );
    let rehydrated: Line<u64> =
        crate::backend::rehydrate::from_pgno_bytes_unchecked::<u64>(&pgno_bytes)
            .expect("backend rehydrate from pgno bytes");
    let recovered_line: Vec<u64> = rehydrated
        .read_line()
        .iter()
        .map(|e| *e.domain_event())
        .collect();
    assert_eq!(
        recovered_line, original_line,
        "rehydrated event line must equal the original (byte-level recovery via backend seam)",
    );
}
#[test]
fn backend_rehydrate_validated_from_pgno_bytes_round_trips_without_opening_a_file() {
    use crate::dragline::DEFAULT_ANCHOR_BUFFER_CAP;
    use crate::dragline::Line;
    use crate::persist::persist_with_source;
    use pardosa_derive::GenomeSafe;
    use pardosa_wire::Validate;
    #[derive(Debug, Clone, Copy, PartialEq, Eq, GenomeSafe)]
    struct Marker {
        v: u64,
    }
    #[derive(Debug, PartialEq, Eq)]
    struct MarkerInvalid;
    impl Validate for Marker {
        type Error = MarkerInvalid;
        fn validate(&self) -> Result<(), Self::Error> {
            if self.v == 0 {
                Err(MarkerInvalid)
            } else {
                Ok(())
            }
        }
    }
    let mut line: Line<Marker> = Line::with_anchor_config(
        "backend-rehydrate-validated-bytes".to_owned(),
        1,
        DEFAULT_ANCHOR_BUFFER_CAP,
    );
    for i in 1..=4u64 {
        let _ = line.create(Marker { v: i }).expect("commit");
    }
    let original_line: Vec<u64> = line
        .read_line()
        .iter()
        .map(|e| e.domain_event().v)
        .collect();
    let mut sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    persist_with_source(&line, &mut sink, None).expect("persist to in-memory sink (validated arm)");
    let pgno_bytes: Vec<u8> = sink.into_inner();
    let rehydrated: Line<Marker> =
        crate::backend::rehydrate::from_pgno_bytes_validated::<Marker>(&pgno_bytes)
            .expect("backend rehydrate validated from pgno bytes");
    let recovered_line: Vec<u64> = rehydrated
        .read_line()
        .iter()
        .map(|e| e.domain_event().v)
        .collect();
    assert_eq!(
        recovered_line, original_line,
        "validated rehydrate must observe every persisted event in commit order \
             (byte-only path, Validate impl green for all v >= 1)",
    );
}
#[test]
fn pgno_sink_append_tracks_stream_position() {
    let mut sink = PgnoFileSink::new(Cursor::new(Vec::<u8>::new()));
    let p1 = sink.append(b"hello").expect("append 1");
    assert_eq!(p1.as_u64(), 5, "first append moves position to byte 5");
    let p2 = sink.append(b" world").expect("append 2");
    assert_eq!(p2.as_u64(), 11, "second append moves position to byte 11");
    assert!(p1 < p2, "AckPosition monotonic within a single sink");
}
#[test]
fn pgno_sink_sync_returns_post_fence_position() {
    let mut sink = PgnoFileSink::new(Cursor::new(Vec::<u8>::new()));
    let _ = sink.append(b"durable-payload").expect("append");
    let ack = sink.sync().expect("sync");
    assert_eq!(
        ack.as_u64(),
        "durable-payload".len() as u64,
        "sync returns post-fence stream position",
    );
}
#[test]
fn pgno_sink_via_real_file_roundtrips_bytes() {
    let path = tempdir_for_test().join("backend-sink.pgno");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .expect("open tempfile");
    let mut sink = PgnoFileSink::<std::fs::File>::new(file);
    let payload = b"backend-neutral-roundtrip";
    let _ = sink.append(payload).expect("append");
    let ack = sink.sync().expect("sync");
    assert_eq!(ack.as_u64(), payload.len() as u64);
    let on_disk = std::fs::read(&path).expect("read back");
    assert_eq!(&on_disk, payload, ".pgno bytes survive sync");
}
fn tempdir_for_test() -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    p.push(format!("pardosa-backend-sink-{nanos}"));
    std::fs::create_dir_all(&p).expect("mkdir tempdir");
    p
}
#[test]
fn ack_position_chain_is_strictly_monotonic() {
    let mut sink = PgnoFileSink::new(Cursor::new(Vec::<u8>::new()));
    let a = sink.append(b"a").expect("a");
    let b = sink.append(b"bb").expect("bb");
    let c = sink.append(b"ccc").expect("ccc");
    let d = sink.sync().expect("sync");
    assert!(a < b);
    assert!(b < c);
    assert_eq!(c, d, "sync returns the same position as the last append");
    assert_eq!(a.as_u64(), 1);
    assert_eq!(b.as_u64(), 3);
    assert_eq!(c.as_u64(), 6);
}
#[test]
fn backend_journal_round_trips_via_in_memory_backend() {
    use crate::authoritative::fake::InMemoryBackend;
    use crate::backend::journal::BackendDragline;
    let backend = InMemoryBackend::new();
    let mut bj: BackendDragline<u64, InMemoryBackend> = BackendDragline::new(backend);
    for i in 0..5u64 {
        let _ = bj.commit_event(i).expect("commit");
    }
    let synced_pos = bj.sync().expect("sync");
    assert!(
        synced_pos.as_u64() > 0,
        "sync must report a positive post-fence position after appending non-empty .pgno bytes",
    );
    let original_line: Vec<u64> = bj
        .line()
        .read_line()
        .iter()
        .map(|e| *e.domain_event())
        .collect();
    let bytes_on_backend: Vec<u8> = bj.into_backend().bytes().to_vec();
    assert!(
        !bytes_on_backend.is_empty(),
        "backend must hold the appended .pgno bytes after sync",
    );
    let reopened: BackendDragline<u64, InMemoryBackend> = {
        let mut fresh = InMemoryBackend::new();
        let _ = fresh.append(&bytes_on_backend).expect("seed fresh backend");
        let _ = fresh.sync().expect("sync fresh backend");
        BackendDragline::rehydrate(fresh).expect("rehydrate from backend bytes")
    };
    let recovered_line: Vec<u64> = reopened
        .line()
        .read_line()
        .iter()
        .map(|e| *e.domain_event())
        .collect();
    assert_eq!(
        recovered_line, original_line,
        "rehydrated event line equals the original (append+sync+reopen+read-back via BackendSink)",
    );
}
#[test]
fn backend_journal_sync_position_strictly_monotonic_across_syncs() {
    use crate::authoritative::fake::InMemoryBackend;
    use crate::backend::journal::BackendDragline;
    let backend = InMemoryBackend::new();
    let mut bj: BackendDragline<u64, InMemoryBackend> = BackendDragline::new(backend);
    let _ = bj.commit_event(1).expect("commit 1");
    let p1 = bj.sync().expect("sync 1");
    let _ = bj.commit_event(2).expect("commit 2");
    let p2 = bj.sync().expect("sync 2");
    assert!(
        p1 < p2,
        "successive sync positions must strictly advance when bytes were appended between them",
    );
}
#[test]
fn backend_journal_rehydrate_observes_only_durable_prefix_not_staged_extent() {
    use crate::authoritative::fake::InMemoryBackend;
    use crate::backend::journal::BackendDragline;
    let backend = InMemoryBackend::new();
    let mut bj: BackendDragline<u64, InMemoryBackend> = BackendDragline::new(backend);
    for i in 0..3u64 {
        let _ = bj.commit_event(i).expect("commit");
    }
    let _ = bj.sync().expect("sync");
    let original_line: Vec<u64> = bj
        .line()
        .read_line()
        .iter()
        .map(|e| *e.domain_event())
        .collect();
    let synced_bytes: Vec<u8> = bj.into_backend().bytes().to_vec();
    let mut tampered = InMemoryBackend::new();
    let _ = tampered.append(&synced_bytes).expect("seed durable prefix");
    let _ = tampered.sync().expect("fence durable prefix");
    let _ = tampered
        .append(b"unsynced-trailing-garbage-that-would-break-pgno-framing")
        .expect("append unsynced trailing bytes");
    let reopened: BackendDragline<u64, InMemoryBackend> = BackendDragline::rehydrate(tampered)
        .expect(
            "rehydrate must observe only the post-sync durable prefix, \
                 not the staged extent including unsynced trailing bytes \
                 (ADR-0022 §D2 sync-as-fence)",
        );
    let recovered_line: Vec<u64> = reopened
        .line()
        .read_line()
        .iter()
        .map(|e| *e.domain_event())
        .collect();
    assert_eq!(
        recovered_line, original_line,
        "rehydrated event line must equal the original — staged-but-unsynced trailing bytes \
             must NOT contaminate the rehydrate (ADR-0022 §D2 sync-as-fence)",
    );
}
#[test]
fn backend_journal_rehydrate_from_round_trips_via_in_memory_backend() {
    use crate::authoritative::fake::InMemoryBackend;
    use crate::backend::journal::BackendDragline;
    let backend = InMemoryBackend::new();
    let mut bj: BackendDragline<u64, InMemoryBackend> = BackendDragline::new(backend);
    for i in 0..4u64 {
        let _ = bj.commit_event(i).expect("commit");
    }
    let _ = bj.sync().expect("sync");
    let original_line: Vec<u64> = bj
        .line()
        .read_line()
        .iter()
        .map(|e| *e.domain_event())
        .collect();
    let synced_bytes: Vec<u8> = bj.into_backend().bytes().to_vec();
    let reopened: BackendDragline<u64, InMemoryBackend> = {
        let mut fresh = InMemoryBackend::new();
        let _ = fresh.append(&synced_bytes).expect("seed fresh backend");
        let _ = fresh.sync().expect("sync fresh backend");
        BackendDragline::rehydrate_from(fresh).expect(
            "rehydrate_from must round-trip via the fetch-based seam \
                 just like rehydrate does via the borrow-based seam",
        )
    };
    let recovered_line: Vec<u64> = reopened
        .line()
        .read_line()
        .iter()
        .map(|e| *e.domain_event())
        .collect();
    assert_eq!(
        recovered_line, original_line,
        "rehydrate_from (fetch-based) must observe every persisted event in commit \
             order — same contract as rehydrate (borrow-based), different transport \
             (ADR-0022 §D2 reader-side seam; mission \
             nats-followups-jetstream-open-06)",
    );
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendCall {
    Append { len: usize },
    Sync,
}
struct RecordingSink {
    calls: Vec<BackendCall>,
    appended: Vec<u8>,
}
impl RecordingSink {
    fn new() -> Self {
        Self {
            calls: Vec::new(),
            appended: Vec::new(),
        }
    }
}
impl super::sealed::Sealed for RecordingSink {}
impl BackendSink for RecordingSink {
    fn append(&mut self, bytes: &[u8]) -> Result<AckPosition, BackendError> {
        self.calls.push(BackendCall::Append { len: bytes.len() });
        self.appended.extend_from_slice(bytes);
        let pos = u64::try_from(self.appended.len()).expect("64-bit target enforced at crate root");
        Ok(AckPosition::from_u64(pos))
    }
    fn sync(&mut self) -> Result<AckPosition, BackendError> {
        self.calls.push(BackendCall::Sync);
        let pos = u64::try_from(self.appended.len()).expect("64-bit target enforced at crate root");
        Ok(AckPosition::from_u64(pos))
    }
}
#[test]
fn backend_journal_sync_invokes_append_then_sync_in_order_on_substrate() {
    use crate::backend::journal::BackendDragline;
    let mut bj: BackendDragline<u64, RecordingSink> = BackendDragline::new(RecordingSink::new());
    for i in 0..4u64 {
        let _ = bj.commit_event(i).expect("commit");
    }
    let _ = bj.sync().expect("sync");
    let sink = bj.into_backend();
    assert_eq!(
        sink.calls.len(),
        2,
        "BackendDragline::sync must drive exactly two substrate calls per sync \
             (one append carrying the full .pgno blob, then one sync to fence); \
             observed calls: {:?}",
        sink.calls,
    );
    assert!(
        matches!(sink.calls[0], BackendCall::Append { .. }),
        "first substrate call must be Append (the append-shape strategy stages \
             bytes before fencing); observed {:?}",
        sink.calls[0],
    );
    assert_eq!(
        sink.calls[1],
        BackendCall::Sync,
        "second substrate call must be Sync (ADR-0022 §D2 sync-is-the-fence; \
             sub-mission 03 append-shape strategy ordering invariant); \
             observed {:?}",
        sink.calls[1],
    );
}
#[test]
fn backend_journal_sync_passes_full_pgno_blob_to_append_as_single_call() {
    use crate::backend::journal::BackendDragline;
    let mut bj: BackendDragline<u64, RecordingSink> = BackendDragline::new(RecordingSink::new());
    for i in 0..6u64 {
        let _ = bj.commit_event(i).expect("commit");
    }
    let _ = bj.sync().expect("sync");
    let sink = bj.into_backend();
    let append_calls: Vec<BackendCall> = sink
        .calls
        .iter()
        .copied()
        .filter(|c| matches!(c, BackendCall::Append { .. }))
        .collect();
    assert_eq!(
        append_calls.len(),
        1,
        "BackendDragline::sync must hand the substrate exactly one .pgno blob per \
             sync via a single BackendSink::append call — chunked / partial-frame \
             append shapes would force the substrate to reassemble framing, which \
             is not part of the §D2 sealed contract; observed append calls: {append_calls:?}",
    );
    let appended_len = sink.appended.len();
    match append_calls[0] {
        BackendCall::Append { len } => {
            assert_eq!(
                len, appended_len,
                "the single Append call's byte count must equal the total appended \
                 length (no later appends contributed bytes between the strategy's \
                 append and its fencing sync)",
            );
        }
        other @ BackendCall::Sync => panic!("expected Append, got {other:?}"),
    }
    assert!(
        appended_len > 0,
        "the single Append call must carry a non-empty .pgno blob (header + index \
             + footer at minimum)",
    );
}
#[test]
fn backend_journal_sync_bytes_byte_identical_to_full_rewrite_persist() {
    use crate::backend::journal::BackendDragline;
    use crate::dragline::DEFAULT_ANCHOR_BUFFER_CAP;
    use crate::dragline::Line;
    use crate::persist::persist_with_source;
    let mut reference_line: Line<u64> = Line::with_anchor_config(
        "sub-03-byte-parity".to_owned(),
        1,
        DEFAULT_ANCHOR_BUFFER_CAP,
    );
    for i in 0..5u64 {
        let _ = reference_line.create(i).expect("commit reference");
    }
    let mut reference_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    persist_with_source(&reference_line, &mut reference_sink, None)
        .expect("full-rewrite persist of reference dragline");
    let reference_bytes = reference_sink.into_inner();
    let mut bj: BackendDragline<u64, RecordingSink> = BackendDragline::new(RecordingSink::new());
    for i in 0..5u64 {
        let _ = bj.commit_event(i).expect("commit via backend journal");
    }
    let _ = bj.sync().expect("sync via backend journal");
    let strategy_bytes: Vec<u8> = bj.into_backend().appended;
    assert_eq!(
        strategy_bytes, reference_bytes,
        "I1 oracle invariant at the BackendDragline-layer write path: the bytes \
             the append-shape strategy hands the substrate via BackendSink::append \
             MUST be byte-identical to the bytes the full-rewrite persist_with_source \
             path would produce for the same dragline. Substrate parity at the \
             persist function layer is pinned by persist.rs::persist_with_source_append_*; \
             this test pins it one layer up at the production-relevant write path \
             that consumes the append-shape strategy (sub-mission 03; oracle bead \
             rescue-pardosa-v0id; ADR-0022 §D2).",
    );
}
