//! Open-time integrity gate for the public load path.
//!
//! [`EventStore::open`] / `open_with_publisher` / `open_validated` route
//! the rehydrate through `Dragline::from_raw_parts`, enforcing `EventId`
//! monotonicity: a `.pgno` whose per-event `event_id` field has been
//! mutated away from the event's line position must fail to open with
//! a structured invariant violation rather than rehydrating.
use pardosa::store::replay::stream_checked;
use pardosa::store::{Encode, Event, EventStore, GenomeSafe, HasEventSchemaSource};
use pardosa_file::{Syncable, Writer as FileWriter};
use std::io::{Cursor, Read, Write};
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
fn write_bytes_to_path(path: &std::path::Path, bytes: &[u8]) {
    let mut f = std::fs::File::create(path).expect("create file for tampered fixture");
    f.write_all(bytes).expect("write tampered bytes");
    f.sync_all().expect("sync_all tampered fixture");
}
fn build_two_event_log_at(path: &std::path::Path) -> Vec<Event<Payload>> {
    {
        let mut store: EventStore<Payload> =
            EventStore::<Payload>::create(path).expect("create EventStore for baseline");
        let mut writer = store.writer();
        let r0 = writer.begin(Payload { v: 10 }).expect("begin 0");
        let _ = writer
            .append(r0.fiber(), Payload { v: 20 })
            .expect("append 1");
        let _ = writer.sync().expect("sync baseline");
    }
    let sink = Cursor::new(read_file_bytes(path));
    let stream = stream_checked::<_, Payload>(sink, None).expect("stream_checked open");
    let events: Vec<Event<Payload>> = stream
        .map(|r| r.expect("baseline stream item must be Ok"))
        .collect();
    assert_eq!(events.len(), 2, "baseline log must carry two events");
    events
}
fn forge_to_path<T>(path: &std::path::Path, events: &[Event<T>])
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
    let bytes = sink.into_inner();
    write_bytes_to_path(path, &bytes);
}
#[test]
fn open_round_trips_a_valid_persisted_store() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    let fid;
    {
        let mut store: EventStore<Payload> =
            EventStore::<Payload>::create(&journal).expect("path-backed create");
        let mut writer = store.writer();
        let r0 = writer.begin(Payload { v: 1 }).expect("begin");
        let live = r0.fiber();
        fid = live.fiber_id();
        let live = writer
            .append(live, Payload { v: 2 })
            .expect("append v=2")
            .fiber();
        let _ = writer.append(live, Payload { v: 3 }).expect("append v=3");
        let _ = writer.sync().expect("sync");
    }
    let store: EventStore<Payload> =
        EventStore::<Payload>::open(&journal).expect("open through the integrity gate");
    let reader = store.reader();
    let history: Vec<u64> = reader
        .fiber(fid)
        .iter()
        .expect("fiber history present after open")
        .map(|ev| ev.domain_event().v)
        .collect();
    assert_eq!(
        history,
        vec![1, 2, 3],
        "open through the integrity gate must preserve commit order on a valid store"
    );
}
#[test]
fn open_rejects_event_id_position_mismatch() {
    let td = TempDir::new().expect("tempdir");
    let baseline = td.path().join("baseline.pgno");
    let forged = td.path().join("forged.pgno");
    let events = build_two_event_log_at(&baseline);
    let e0 = &events[0];
    let e1 = &events[1];
    let bad_event_id: u64 = e1.event_id().value() + 7;
    let forged_e1 = Event::<Payload>::new_unchecked(
        bad_event_id,
        e1.fiber_id(),
        e1.detached(),
        e1.precursor(),
        e1.precursor_hash(),
        e1.domain_event().clone(),
    );
    forge_to_path::<Payload>(&forged, &[e0.clone(), forged_e1]);
    let Err(err) = EventStore::<Payload>::open(&forged) else {
        panic!("open must reject mutated event_id")
    };
    let msg = format!("{err}");
    assert!(
        msg.contains("event_id") && msg.contains("position"),
        "expected EventIdPositionMismatch surfaced through open's CursorRead, got: {err}"
    );
}
