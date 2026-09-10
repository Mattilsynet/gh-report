use pardosa::store::migrate::{MigrationError, migrate_keep};
use pardosa::store::replay::stream_validated;
use pardosa::store::{Event, EventStore, GenomeSafe, HasEventSchemaSource, Validate};
use std::convert::Infallible;
use std::path::PathBuf;
use tempfile::TempDir;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("injected upcast failure at event position {position}")]
struct InjectedUpcastError {
    position: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct OldPayload {
    v: u64,
}

impl HasEventSchemaSource for OldPayload {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}

impl Validate for OldPayload {
    type Error = Infallible;

    fn validate(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct NewPayload {
    v: u64,
    migrated: bool,
}

impl HasEventSchemaSource for NewPayload {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}

impl Validate for NewPayload {
    type Error = Infallible;

    fn validate(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn test_paths(dir: &TempDir) -> (PathBuf, PathBuf) {
    (dir.path().join("old.pgno"), dir.path().join("new.pgno"))
}

#[test]
fn success_asserts_new_sink_valid_openable_and_old_sink_unchanged() {
    let dir = TempDir::new().expect("tempdir");
    let (old_path, new_path) = test_paths(&dir);

    {
        let mut store = EventStore::<OldPayload>::create(&old_path).expect("create old store");
        let receipt1 = store
            .writer()
            .begin(OldPayload { v: 10 })
            .expect("begin fiber 1");
        let live1 = receipt1.fiber();

        let receipt2 = store
            .writer()
            .begin(OldPayload { v: 20 })
            .expect("begin fiber 2");
        let _live2 = receipt2.fiber();

        let receipt3 = store
            .writer()
            .append(live1, OldPayload { v: 11 })
            .expect("append fiber 1");
        let live1 = receipt3.fiber();

        let receipt4 = store
            .writer()
            .detach(live1, OldPayload { v: 12 })
            .expect("detach fiber 1");
        let detached1 = receipt4.fiber();

        let _receipt5 = store
            .writer()
            .resume(detached1, OldPayload { v: 13 })
            .expect("resume fiber 1");

        let _ = store.writer().sync().expect("sync old store");
    }

    let old_bytes_before = std::fs::read(&old_path).expect("read old bytes before migration");

    let upcast = |event: Event<OldPayload>| -> Result<NewPayload, Infallible> {
        Ok(NewPayload {
            v: event.domain_event().v + 1_000,
            migrated: true,
        })
    };

    let report =
        migrate_keep::<OldPayload, NewPayload, Infallible, _>(&old_path, &new_path, upcast)
            .expect("migrate_keep must succeed");

    assert_eq!(report.old_event_count(), 5);
    assert_eq!(report.new_event_count(), 5);
    assert!(report.synced_lsn().value() > 0);

    let old_bytes_after = std::fs::read(&old_path).expect("read old bytes after migration");
    assert_eq!(
        old_bytes_before, old_bytes_after,
        "old sink bytes must remain byte-identical"
    );

    let _new_store = EventStore::<NewPayload>::open_validated(&new_path)
        .expect("new sink must be valid openable journal");
    let meta =
        EventStore::<NewPayload>::metadata(&new_path).expect("new sink metadata must be readable");
    assert_eq!(meta.len(), 5);

    let file = std::fs::File::open(&new_path).expect("open new file");
    let stream = stream_validated::<_, NewPayload>(file, None).expect("stream new file");
    let mut count = 0u64;
    for item in stream {
        let event = item.expect("read valid event");
        assert!(event.domain_event().migrated);
        assert!(event.domain_event().v >= 1_000);
        count += 1;
    }
    assert_eq!(count, 5);
}

#[test]
fn failure_on_upcast_asserts_old_sink_unchanged_and_new_sink_unopenable() {
    let dir = TempDir::new().expect("tempdir");
    let (old_path, new_path) = test_paths(&dir);

    {
        let mut store = EventStore::<OldPayload>::create(&old_path).expect("create old store");
        let receipt1 = store
            .writer()
            .begin(OldPayload { v: 100 })
            .expect("begin fiber 1");
        let live1 = receipt1.fiber();

        let receipt2 = store
            .writer()
            .begin(OldPayload { v: 200 })
            .expect("begin fiber 2");
        let _live2 = receipt2.fiber();

        let _receipt3 = store
            .writer()
            .append(live1, OldPayload { v: 101 })
            .expect("append fiber 1");

        let _ = store.writer().sync().expect("sync old store");
    }

    let old_bytes_before = std::fs::read(&old_path).expect("read old bytes before migration");

    let mut seen = 0u64;
    let upcast = move |event: Event<OldPayload>| -> Result<NewPayload, InjectedUpcastError> {
        let current = seen;
        seen += 1;
        if current == 1 {
            return Err(InjectedUpcastError { position: current });
        }
        Ok(NewPayload {
            v: event.domain_event().v + 1_000,
            migrated: true,
        })
    };

    let result = migrate_keep::<OldPayload, NewPayload, InjectedUpcastError, _>(
        &old_path, &new_path, upcast,
    );

    match result {
        Err(MigrationError::Upcast { position, source }) => {
            assert_eq!(position, 1);
            assert_eq!(source, InjectedUpcastError { position: 1 });
        }
        Ok(_) => panic!("migrate_keep must fail when upcast fails"),
        Err(other) => panic!("expected Upcast error, got: {other:?}"),
    }

    let old_bytes_after = std::fs::read(&old_path).expect("read old bytes after migration");
    assert_eq!(
        old_bytes_before, old_bytes_after,
        "old sink bytes must remain byte-identical after failure"
    );

    let open_result = EventStore::<NewPayload>::open_validated(&new_path);
    assert!(
        open_result.is_err(),
        "new sink must not be openable as a valid journal after failure"
    );

    let meta_result = EventStore::<NewPayload>::metadata(&new_path);
    assert!(
        meta_result.is_err(),
        "new sink metadata must fail after failure"
    );

    let file_result = std::fs::File::open(&new_path);
    if let Ok(file) = file_result {
        let stream_result = stream_validated::<_, NewPayload>(file, None);
        assert!(
            stream_result.is_err(),
            "stream_validated on new sink must fail after failure"
        );
    }
}
