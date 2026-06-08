//! Public-surface checked replay (ADR-0018 Amendment 1, ADR-0003 §1).
//!
//! Demonstrates that
//! [`pardosa::store::replay::stream_checked`] rejects corrupt
//! `precursor_hash` and cross-fiber `precursor` references via
//! [`CheckedReplayKind`] and surfaces them through
//! [`pardosa::store::replay::Error::is_tamper_suspicious`].
//!
//! Lives under the workspace harness because envelope forging
//! requires the `test-support`-gated `Event::new_unchecked`
//! (ADR-0018 §D7); the `stream_checked` / `Error` surface itself
//! is adopter-public. Baseline bytes come from
//! `EventStore::create` and are reframed via `pardosa_file::Writer`
//! so only the typed envelope invariant is broken.
use pardosa::store::replay::{CheckedReplayKind, Error as ReplayError, stream_checked};
use pardosa::store::{Encode, Event, EventStore, GenomeSafe, HasEventSchemaSource};
use pardosa_file::{Syncable, Writer as FileWriter};
use std::io::{Cursor, Read};
use tempfile::TempDir;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Payload {
    v: u64,
}
impl HasEventSchemaSource for Payload {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
fn read_file_bytes(path: &std::path::Path) -> Vec<u8> {
    let mut f = std::fs::File::open(path).expect("open file for read-back");
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes).expect("read file");
    bytes
}
fn stream_from_path(path: &std::path::Path) -> Cursor<Vec<u8>> {
    Cursor::new(read_file_bytes(path))
}
fn build_two_event_log() -> Vec<Event<Payload>> {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    {
        let mut store: EventStore<Payload> =
            EventStore::<Payload>::create(&journal).expect("create EventStore for baseline");
        let mut writer = store.writer();
        let r0 = writer.begin(Payload { v: 10 }).expect("begin 0");
        let _ = writer
            .append(r0.fiber(), Payload { v: 20 })
            .expect("append 1");
        let _ = writer.sync().expect("sync baseline");
    }
    let sink = stream_from_path(&journal);
    let stream = stream_checked::<_, Payload>(sink, None).expect("stream_checked open");
    let events: Vec<Event<Payload>> = stream
        .map(|r| r.expect("baseline stream item must be Ok"))
        .collect();
    assert_eq!(events.len(), 2, "baseline log must carry two events");
    events
}
fn forge<T>(events: &[Event<T>]) -> Cursor<Vec<u8>>
where
    T: Encode + GenomeSafe + Clone,
{
    let mut sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    {
        let writer_sink: &mut Cursor<Vec<u8>> = &mut sink;
        let mut writer = FileWriter::new(writer_sink, Event::<T>::ENVELOPE_HASH);
        let mut buf: Vec<u8> = Vec::new();
        for ev in events {
            buf.clear();
            ev.encode(&mut buf);
            writer.write_message(&buf).expect("write_message");
        }
        writer.finish().expect("finish");
    }
    Syncable::sync_data(&mut sink).expect("sync_data on forged sink");
    sink.set_position(0);
    sink
}
#[test]
fn forged_precursor_hash_surfaces_as_typed_checked_replay_error() {
    let events = build_two_event_log();
    let e0 = &events[0];
    let e1 = &events[1];
    let mut bad_hash = e1.precursor_hash();
    bad_hash[0] ^= 0xFF;
    let forged_e1 = Event::<Payload>::new_unchecked(
        e1.event_id(),
        e1.fiber_id(),
        e1.detached(),
        e1.precursor(),
        bad_hash,
        e1.domain_event().clone(),
    );
    let forged_sink = forge::<Payload>(&[e0.clone(), forged_e1]);
    let mut stream = stream_checked::<_, Payload>(forged_sink, None)
        .expect("forged log re-opens (header still valid)");
    let first = stream
        .next()
        .expect("event 0 must be yielded")
        .expect("event 0 must still pass checked replay");
    assert_eq!(*first.domain_event(), Payload { v: 10 });
    let err = stream
        .next()
        .expect("event 1 must surface an error item")
        .expect_err("event 1 must fail checked replay");
    assert!(
        err.is_tamper_suspicious(),
        "precursor-hash mismatch must surface as tamper-suspicious: {err}"
    );
    let kind = match err {
        ReplayError::CheckedReplay { kind } => kind,
        other => panic!("expected CheckedReplay, got {other:?}"),
    };
    match kind {
        CheckedReplayKind::PrecursorHashMismatch { event_id, .. } => {
            assert_eq!(event_id, e1.event_id().value());
        }
        other => panic!("expected PrecursorHashMismatch, got {other:?}"),
    }
    assert!(
        stream.next().is_none(),
        "checked stream must poison after first replay error"
    );
}
#[test]
fn forged_fiber_mismatch_surfaces_as_typed_checked_replay_error() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    {
        let mut store: EventStore<Payload> =
            EventStore::<Payload>::create(&journal).expect("create EventStore for baseline");
        let mut writer = store.writer();
        let r_a0 = writer.begin(Payload { v: 10 }).expect("begin A (event 0)");
        let _r_b = writer.begin(Payload { v: 99 }).expect("begin B (event 1)");
        let _r_a1 = writer
            .append(r_a0.fiber(), Payload { v: 20 })
            .expect("append A (event 2)");
        let _ = writer.sync().expect("sync");
    }
    let sink = stream_from_path(&journal);
    let stream = stream_checked::<_, Payload>(sink, None).expect("baseline stream_checked open");
    let events: Vec<Event<Payload>> = stream
        .map(|r| r.expect("baseline must replay cleanly"))
        .collect();
    assert_eq!(events.len(), 3, "expected three events in baseline log");
    let fiber_a = events[0].fiber_id();
    let fiber_b = events[1].fiber_id();
    assert_ne!(
        fiber_a, fiber_b,
        "two begin calls must mint distinct FiberIds"
    );
    let e2 = &events[2];
    assert_eq!(
        e2.fiber_id(),
        fiber_a,
        "event 2 must originally live on fiber A (precursor points at event 0)"
    );
    let forged_e2 = Event::<Payload>::new_unchecked(
        e2.event_id(),
        fiber_b,
        e2.detached(),
        e2.precursor(),
        e2.precursor_hash(),
        e2.domain_event().clone(),
    );
    let forged_sink = forge::<Payload>(&[events[0].clone(), events[1].clone(), forged_e2]);
    let mut stream = stream_checked::<_, Payload>(forged_sink, None)
        .expect("forged log re-opens (header still valid)");
    let _e0 = stream
        .next()
        .expect("event 0 yields")
        .expect("event 0 still passes");
    let _e1 = stream
        .next()
        .expect("event 1 yields")
        .expect("event 1 still passes");
    let err = stream
        .next()
        .expect("event 2 yields error item")
        .expect_err("event 2 must fail with fiber mismatch");
    assert!(
        err.is_tamper_suspicious(),
        "precursor-fiber mismatch must surface as tamper-suspicious: {err}"
    );
    let kind = match err {
        ReplayError::CheckedReplay { kind } => kind,
        other => panic!("expected CheckedReplay, got {other:?}"),
    };
    match kind {
        CheckedReplayKind::PrecursorFiberMismatch {
            event_id,
            expected_fiber,
            actual_fiber,
            ..
        } => {
            assert_eq!(event_id, e2.event_id().value());
            assert_eq!(expected_fiber, fiber_b);
            assert_eq!(actual_fiber, fiber_a);
        }
        other => panic!("expected PrecursorFiberMismatch, got {other:?}"),
    }
}
#[test]
fn untampered_log_round_trips_through_public_checked_stream() {
    let events = build_two_event_log();
    let forged_sink = forge::<Payload>(&events);
    let stream = stream_checked::<_, Payload>(forged_sink, None).expect("open re-framed log");
    let round_tripped: Vec<Payload> = stream
        .map(|r| {
            r.expect("untampered re-framing must round-trip")
                .into_inner()
        })
        .collect();
    assert_eq!(
        round_tripped,
        vec![Payload { v: 10 }, Payload { v: 20 }],
        "re-framing without envelope mutation must round-trip cleanly"
    );
}
