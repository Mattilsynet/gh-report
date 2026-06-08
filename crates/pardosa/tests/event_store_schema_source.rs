//! Path-backed container metadata via the adopter-facing
//! [`pardosa::store::EventStore::<T>::metadata`] (ADR-0018
//! §D7 / § Naming).
//!
//! Pins the contract that `EventStore::create` wires
//! `T::EVENT_SCHEMA_SOURCE` into the container header and that
//! adopters can read it back through `pardosa::store` alone —
//! no `pardosa_file::Reader` reach-through.
use pardosa::store::{EventStore, GenomeSafe, HasEventSchemaSource, StoreMetadata};
use tempfile::TempDir;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Tagged {
    v: u64,
}
impl HasEventSchemaSource for Tagged {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = Some("pardosa-tests/Tagged");
}
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Untagged {
    v: u64,
}
impl HasEventSchemaSource for Untagged {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
#[test]
fn create_wires_event_schema_source_into_container_header() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("tagged.pgno");
    {
        let mut store: EventStore<Tagged> = EventStore::<Tagged>::create(&journal).expect("create");
        let mut writer = store.writer();
        let _ = writer.begin(Tagged { v: 1 }).expect("begin");
        let _ = writer.sync().expect("sync");
    }
    let meta: StoreMetadata = EventStore::<Tagged>::metadata(&journal).expect("metadata");
    assert_eq!(
        meta.schema_source(),
        Some("pardosa-tests/Tagged"),
        "EventStore::create must wire T::EVENT_SCHEMA_SOURCE into the container header"
    );
    assert_eq!(meta.len(), 1, "one event was appended");
    assert!(!meta.is_empty(), "non-empty log");
    assert_ne!(meta.schema_hash(), 0, "schema hash must be populated");
}
#[test]
fn create_leaves_slot_empty_when_event_schema_source_is_none() {
    let td = TempDir::new().expect("tempdir");
    let journal = td.path().join("untagged.pgno");
    {
        let mut store: EventStore<Untagged> =
            EventStore::<Untagged>::create(&journal).expect("create");
        let mut writer = store.writer();
        let _ = writer.begin(Untagged { v: 1 }).expect("begin");
        let _ = writer.sync().expect("sync");
    }
    let meta: StoreMetadata = EventStore::<Untagged>::metadata(&journal).expect("metadata");
    assert_eq!(
        meta.schema_source(),
        None,
        "no schema source declared → empty slot"
    );
    assert_eq!(meta.len(), 1);
}
