//! Migration-truth runtime gates (ADR-0018 §D7).
//!
//! 1. [`EventStore::open`] never auto-migrates — schema-hash mismatch
//!    is a hard error, bytes intact.
//! 2. [`migrate_keep`] round-trips through `EventStore::<New>::open`
//!    and the public reader views.
//! 3. Migration is one-way at the file boundary.
//! 4. A failed `migrate_keep` leaves no openable migrated stream.
//! 5. A mismatched `open` is non-destructive.
//! 6. Detach/rescue topology survives `migrate_keep`.
//! 7. A truncated old file aborts `migrate_keep` before any sink sync.
use pardosa::store::migrate::{MigrationError, migrate_keep};
use pardosa::store::{Event, EventStore, FiberId, GenomeSafe, HasEventSchemaSource};
use std::io::Read;
use tempfile::TempDir;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct OldV1 {
    v: u32,
}
impl HasEventSchemaSource for OldV1 {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct NewV2 {
    v: u64,
    tag: u8,
}
impl HasEventSchemaSource for NewV2 {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
#[derive(Debug, PartialEq, Eq)]
struct UpcastFailed(&'static str);
#[expect(
    clippy::needless_pass_by_value,
    clippy::unnecessary_wraps,
    reason = "migration-callback signature is fixed by migrate_keep's FnMut(Event<Old>) -> Result<New, E> API: it requires an owned event and Result return"
)]
fn upcast_double(e: Event<OldV1>) -> Result<NewV2, UpcastFailed> {
    Ok(NewV2 {
        v: u64::from(e.domain_event().v) * 2,
        tag: 7,
    })
}
fn write_old_via_store(path: &std::path::Path, values: &[u32]) {
    let mut store: EventStore<OldV1> = EventStore::<OldV1>::create(path).expect("create old");
    let mut writer = store.writer();
    for v in values {
        let _ = writer.begin(OldV1 { v: *v }).expect("begin");
    }
    let _ = writer.sync().expect("sync");
}
fn read_file_bytes(path: &std::path::Path) -> Vec<u8> {
    let mut f = std::fs::File::open(path).expect("open file for snapshot");
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes).expect("read file");
    bytes
}
#[test]
fn event_store_open_on_schema_mismatch_does_not_rewrite_old_bytes() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("journal.pgno");
    write_old_via_store(&journal, &[1, 2, 3]);
    let before = read_file_bytes(&journal);
    let err = EventStore::<NewV2>::open(&journal)
        .err()
        .expect("schema-hash mismatch must surface, not auto-migrate");
    let msg = format!("{err}");
    assert!(
        msg.contains("cursor read") || msg.contains("schema") || msg.contains("hash"),
        "expected schema-hash-mismatch wrapping; got: {msg}"
    );
    let after = read_file_bytes(&journal);
    assert_eq!(
        before, after,
        "failed open as wrong-schema EventStore must not mutate the on-disk bytes"
    );
    let store: EventStore<OldV1> = EventStore::<OldV1>::open(&journal)
        .expect("original-schema open must still work after failed wrong-schema open");
    let _ = store.reader();
}
#[test]
fn migrate_keep_output_round_trips_through_event_store_open() {
    let td = TempDir::new().expect("tempdir");
    let old_path = td.path().join("old.pgno");
    let new_path = td.path().join("new.pgno");
    let sidecar = td.path().join("ack.sidecar");
    write_old_via_store(&old_path, &[10, 20, 30, 40]);
    let report = migrate_keep::<OldV1, NewV2, UpcastFailed, _>(&old_path, &new_path, upcast_double)
        .expect("migrate_keep ok");
    assert_eq!(report.old_event_count(), 4);
    assert_eq!(report.new_event_count(), 4);
    let sink = report.into_inner();
    drop(sink);
    let store: EventStore<NewV2> =
        EventStore::<NewV2>::open(&new_path).expect("EventStore::open on migrated file");
    let reader = store.reader();
    let fids: Vec<FiberId> = {
        let mut cur = reader.cursor(&sidecar).expect("cursor on migrated file");
        cur.tail()
            .map(|r| r.expect("tail item").fiber_id())
            .collect()
    };
    assert_eq!(fids.len(), 4, "migrated stream must surface 4 events");
    let payloads: Vec<u64> = fids
        .iter()
        .map(|fid| {
            reader
                .fiber(*fid)
                .iter()
                .expect("history iter")
                .next()
                .expect("at least one event per fiber")
                .domain_event()
                .v
        })
        .collect();
    assert_eq!(
        payloads,
        vec![20, 40, 60, 80],
        "migrated payloads must be the upcast outputs in commit order"
    );
}
#[test]
fn migrated_new_file_cannot_be_reopened_as_old_schema() {
    let td = TempDir::new().expect("tempdir");
    let old_path = td.path().join("old.pgno");
    let new_path = td.path().join("new.pgno");
    write_old_via_store(&old_path, &[1, 2, 3]);
    let report = migrate_keep::<OldV1, NewV2, UpcastFailed, _>(&old_path, &new_path, upcast_double)
        .expect("migrate_keep ok");
    drop(report);
    let _ok: EventStore<NewV2> =
        EventStore::<NewV2>::open(&new_path).expect("EventStore::<New> opens migrated file");
    let err = EventStore::<OldV1>::open(&new_path)
        .err()
        .expect("opening migrated file as Old must fail with schema-hash mismatch");
    let msg = format!("{err}");
    assert!(
        msg.contains("cursor read") || msg.contains("schema") || msg.contains("hash"),
        "expected schema-hash-mismatch wrapping when opening migrated file as Old; got: {msg}"
    );
}
#[test]
fn failed_migrate_keep_upcast_leaves_no_openable_event_store() {
    let td = TempDir::new().expect("tempdir");
    let old_path = td.path().join("old.pgno");
    let new_path = td.path().join("new.pgno");
    write_old_via_store(&old_path, &[10, 20, 30, 40]);
    let new_before_exists = new_path.exists();
    assert!(
        !new_before_exists,
        "new path must not exist before migrate_keep"
    );
    let upcast = |e: Event<OldV1>| -> Result<NewV2, UpcastFailed> {
        if e.domain_event().v == 30 {
            Err(UpcastFailed("v=30 rejected"))
        } else {
            Ok(NewV2 {
                v: u64::from(e.domain_event().v),
                tag: 0,
            })
        }
    };
    let err = migrate_keep::<OldV1, NewV2, UpcastFailed, _>(&old_path, &new_path, upcast)
        .expect_err("upcast error must abort");
    match err {
        MigrationError::Upcast { position, source } => {
            assert_eq!(position, 2);
            assert_eq!(source, UpcastFailed("v=30 rejected"));
        }
        other => panic!("expected MigrationError::Upcast, got {other:?}"),
    }
    let open_err = EventStore::<NewV2>::open(&new_path).err();
    assert!(
        open_err.is_some(),
        "EventStore::open on a non-synced migration-aborted sink must error"
    );
}
#[test]
fn migrate_keep_does_not_consume_or_mutate_the_old_journal_path() {
    let td = TempDir::new().expect("tempdir");
    let old_path = td.path().join("old.pgno");
    let new_path = td.path().join("new.pgno");
    write_old_via_store(&old_path, &[7, 9, 11]);
    let old_before = read_file_bytes(&old_path);
    let report = migrate_keep::<OldV1, NewV2, UpcastFailed, _>(&old_path, &new_path, upcast_double)
        .expect("migrate_keep ok");
    drop(report);
    let old_after = read_file_bytes(&old_path);
    assert_eq!(
        old_before, old_after,
        "migrate_keep must not mutate the old source path's bytes"
    );
    let store: EventStore<OldV1> = EventStore::<OldV1>::open(&old_path)
        .expect("old path must remain openable as Old after migration");
    let _ = store.reader();
}
#[test]
fn migrate_keep_preserves_detach_rescue_topology() {
    let td = TempDir::new().expect("tempdir");
    let old_path = td.path().join("old.pgno");
    let new_path = td.path().join("new.pgno");
    let sidecar = td.path().join("ack.sidecar");
    {
        let mut store: EventStore<OldV1> =
            EventStore::<OldV1>::create(&old_path).expect("create old");
        let mut writer = store.writer();
        let r_a0 = writer.begin(OldV1 { v: 1 }).expect("a0 begin");
        let live_a = r_a0.fiber();
        let r_a1 = writer.append(live_a, OldV1 { v: 2 }).expect("a1 append");
        let live_a = r_a1.fiber();
        let _r_b0 = writer.begin(OldV1 { v: 100 }).expect("b0 begin");
        let dr = writer.detach(live_a, OldV1 { v: 3 }).expect("a detach");
        let detached_a = dr.fiber();
        let _ra = writer.resume(detached_a, OldV1 { v: 4 }).expect("a resume");
        let _ = writer.sync().expect("sync old");
    }
    let report = migrate_keep::<OldV1, NewV2, UpcastFailed, _>(&old_path, &new_path, upcast_double)
        .expect("migrate_keep ok");
    assert_eq!(report.old_event_count(), 5);
    assert_eq!(report.new_event_count(), 5);
    drop(report);
    let store: EventStore<NewV2> = EventStore::<NewV2>::open(&new_path).expect("open migrated new");
    let reader = store.reader();
    let events: Vec<(FiberId, bool, u64)> = {
        let mut cur = reader.cursor(&sidecar).expect("cursor");
        cur.tail()
            .map(|r| {
                let e = r.expect("tail item");
                (e.fiber_id(), e.detached(), e.domain_event().v)
            })
            .collect()
    };
    assert_eq!(events.len(), 5, "migrated stream must carry 5 events");
    let (f0, d0, v0) = events[0];
    let (f1, d1, v1) = events[1];
    let (f2, d2, v2) = events[2];
    let (f3, d3, v3) = events[3];
    let (f4, d4, v4) = events[4];
    assert_eq!(f0, f1, "events 0,1 must share fiber A across migration");
    assert_ne!(f2, f0, "event 2 must be fiber B");
    assert_eq!(f3, f0, "detach event must remain on fiber A");
    assert_eq!(f4, f0, "resume event must remain on fiber A");
    assert!(!d0, "event 0 (begin) must not be flagged detached");
    assert!(!d1, "event 1 (append) must not be flagged detached");
    assert!(!d2, "event 2 (begin B) must not be flagged detached");
    assert!(d3, "event 3 (detach) must be flagged detached");
    assert!(!d4, "event 4 (resume) must not be flagged detached");
    assert_eq!(
        (v0, v1, v2, v3, v4),
        (2, 4, 200, 6, 8),
        "upcast_double must apply uniformly across the detach/rescue topology"
    );
}
#[test]
fn migrate_keep_on_truncated_old_file_aborts_with_old_read_or_open_error() {
    let td = TempDir::new().expect("tempdir");
    let old_path = td.path().join("old.pgno");
    let new_path = td.path().join("new.pgno");
    {
        let mut store: EventStore<OldV1> =
            EventStore::<OldV1>::create(&old_path).expect("create old");
        let mut writer = store.writer();
        for v in &[10u32, 20, 30] {
            let _ = writer.begin(OldV1 { v: *v }).expect("begin");
        }
        let _ = writer.sync().expect("sync old");
    }
    let old_len = std::fs::metadata(&old_path).expect("stat old").len();
    let truncate_to = old_len.saturating_sub(80);
    assert!(
        truncate_to < old_len,
        "precondition: file must be longer than 80 bytes; got {old_len}"
    );
    std::fs::OpenOptions::new()
        .write(true)
        .open(&old_path)
        .expect("open old for truncate")
        .set_len(truncate_to)
        .expect("truncate old file");
    let err = migrate_keep::<OldV1, NewV2, UpcastFailed, _>(&old_path, &new_path, upcast_double)
        .expect_err("truncated old stream must abort migrate_keep");
    match err {
        MigrationError::OldOpen(_) | MigrationError::OldRead { .. } => {}
        other => panic!("expected OldOpen/OldRead, got {other:?}"),
    }
}
