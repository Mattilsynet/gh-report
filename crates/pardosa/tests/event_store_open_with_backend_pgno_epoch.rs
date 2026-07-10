//! PGN-0021 R8/R9 through the PUBLIC `.pgno` admission seam:
//! `PgnoBackend::with_adopter_epoch` threaded end-to-end via
//! `EventStore::<T>::create_with_backend` /
//! `EventStore::<T>::open_with_backend` (no `#[cfg(test)]` seam).
//!
//! Parity sibling of the live `JetStream` coverage in
//! `dragline::runtime::tests::osf_epoch_gate` (in-crate, `#[ignore]`);
//! this file exercises the `.pgno` backend at the same public
//! entry point adopters use.
use pardosa::store::PardosaError;
use pardosa::store::replay::Error as ReplayError;
use pardosa::store::{EventStore, GenomeSafe, HasEventSchemaSource, PgnoBackend};
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Ledger {
    seq: u64,
}
impl HasEventSchemaSource for Ledger {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
fn scratch_path(tag: &str) -> std::path::PathBuf {
    let mut tmp = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    tmp.push(format!(
        "pardosa-pgno-epoch-{tag}-{}-{nanos}.pgno",
        std::process::id()
    ));
    tmp
}
#[test]
fn none_epoch_reproduces_todays_create_and_reopen_behavior_byte_for_byte() {
    let plain_path = scratch_path("none-plain");
    let backend_path = scratch_path("none-backend");
    let mut plain: EventStore<Ledger> =
        EventStore::<Ledger>::create(&plain_path).expect("plain create");
    let _ = plain.writer().begin(Ledger { seq: 1 }).expect("begin");
    let _ = plain.writer().sync().expect("sync");
    drop(plain);
    let plain_bytes = std::fs::read(&plain_path).expect("read plain .pgno");
    std::fs::remove_file(&plain_path).expect("cleanup plain");
    let mut store: EventStore<Ledger> =
        EventStore::<Ledger>::create_with_backend(PgnoBackend::open(&backend_path))
            .expect("create_with_backend(None) must reproduce EventStore::create exactly");
    let _ = store
        .writer()
        .begin(Ledger { seq: 1 })
        .expect("begin event");
    let _ = store.writer().sync().expect("sync");
    drop(store);
    let backend_bytes = std::fs::read(&backend_path).expect("read backend-created .pgno");
    assert_eq!(
        plain_bytes, backend_bytes,
        "create_with_backend(PgnoBackend::open(_)) with no with_adopter_epoch call must be \
         byte-identical to EventStore::create for the same committed event (R8)"
    );
    let reopened: Result<EventStore<Ledger>, _> =
        EventStore::<Ledger>::open_with_backend(PgnoBackend::open(&backend_path));
    reopened.expect("None-epoch .pgno must reopen unchanged under open_with_backend(None) (R8)");
    let _ = std::fs::remove_file(&backend_path);
}
#[test]
fn some_epoch_seeds_on_create_matches_on_reopen_mismatches_on_differing_epoch() {
    let path = scratch_path("some-seed");
    let mut store: EventStore<Ledger> = EventStore::<Ledger>::create_with_backend(
        PgnoBackend::open(&path).with_adopter_epoch(b"16.0"),
    )
    .expect("Some(_) epoch create succeeds (R8)");
    let _ = store
        .writer()
        .begin(Ledger { seq: 1 })
        .expect("begin event");
    let _ = store.writer().sync().expect("sync");
    drop(store);
    let matching: Result<EventStore<Ledger>, _> = EventStore::<Ledger>::open_with_backend(
        PgnoBackend::open(&path).with_adopter_epoch(b"16.0"),
    );
    matching.expect("re-opening with the seeded epoch matches byte-for-byte");
    let mismatching: Result<EventStore<Ledger>, _> = EventStore::<Ledger>::open_with_backend(
        PgnoBackend::open(&path).with_adopter_epoch(b"15.0"),
    );
    let Err(err) = mismatching else {
        panic!("differing adopter_epoch must refuse before rehydrate (R7)")
    };
    match err {
        PardosaError::CursorRead { source } => match *source {
            ReplayError::SemanticEpochMismatch { expected, found } => {
                assert_eq!(expected.as_deref(), Some(b"15.0".as_slice()));
                assert_eq!(found.as_deref(), Some(b"16.0".as_slice()));
            }
            other => panic!("expected SemanticEpochMismatch, got {other:?}"),
        },
        other => panic!("expected CursorRead, got {other:?}"),
    }
    let _ = std::fs::remove_file(&path);
}
#[test]
fn none_on_populated_some_stream_is_refused_r9() {
    let path = scratch_path("downgrade");
    let mut store: EventStore<Ledger> = EventStore::<Ledger>::create_with_backend(
        PgnoBackend::open(&path).with_adopter_epoch(b"16.0"),
    )
    .expect("Some(_) epoch create succeeds (R8)");
    let _ = store
        .writer()
        .begin(Ledger { seq: 1 })
        .expect("begin event");
    let _ = store.writer().sync().expect("sync");
    drop(store);
    let downgrade: Result<EventStore<Ledger>, _> =
        EventStore::<Ledger>::open_with_backend(PgnoBackend::open(&path));
    let Err(err) = downgrade else {
        panic!("Some(_) -> None downgrade on a populated .pgno must be refused (R9)")
    };
    match err {
        PardosaError::CursorRead { source } => match *source {
            ReplayError::SemanticEpochMismatch { expected, found } => {
                assert_eq!(expected, None);
                assert_eq!(found.as_deref(), Some(b"16.0".as_slice()));
            }
            other => panic!("expected SemanticEpochMismatch, got {other:?}"),
        },
        other => panic!("expected CursorRead, got {other:?}"),
    }
    let _ = std::fs::remove_file(&path);
}
#[test]
fn builder_default_no_with_adopter_epoch_call_equals_none() {
    let path = scratch_path("builder-default");
    let mut store: EventStore<Ledger> =
        EventStore::<Ledger>::create_with_backend(PgnoBackend::open(&path))
            .expect("builder default (no with_adopter_epoch call) must behave as None");
    let _ = store
        .writer()
        .begin(Ledger { seq: 1 })
        .expect("begin event");
    let _ = store.writer().sync().expect("sync");
    drop(store);
    let reopened: Result<EventStore<Ledger>, _> =
        EventStore::<Ledger>::open_with_backend(PgnoBackend::open(&path));
    reopened.expect("builder-default (None) reopens unchanged, matching today's behavior");
    let _ = std::fs::remove_file(&path);
}
