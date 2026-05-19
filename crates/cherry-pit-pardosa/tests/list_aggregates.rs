//! `PardosaFileEventStore::list_aggregates` — enumerates on-disk
//! aggregate ids in ascending order. Added for M1.3 adr-srv replay-
//! on-boot; tested in isolation here so the surface has unit-level
//! coverage independent of any downstream consumer.

use std::sync::Arc;

use cherry_pit_core::{CorrelationContext, DomainEvent, EventStore};
use cherry_pit_pardosa::PardosaFileEventStore;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Tick(u32);

impl DomainEvent for Tick {
    fn event_type(&self) -> &'static str {
        "Tick"
    }
}

impl pardosa_encoding::Encode for Tick {
    fn encode(&self, out: &mut Vec<u8>) {
        self.0.encode(out);
    }
}

impl pardosa_encoding::Decode for Tick {
    fn decode(d: &mut pardosa_encoding::Decoder<'_>) -> Result<Self, pardosa_encoding::EventError> {
        Ok(Self(u32::decode(d)?))
    }
}

fn ctx() -> CorrelationContext {
    CorrelationContext::none()
}

#[tokio::test]
async fn list_aggregates_returns_empty_for_fresh_store() {
    let dir = TempDir::new().expect("tempdir");
    let store: PardosaFileEventStore<Tick> =
        PardosaFileEventStore::open(dir.path()).expect("open store");
    let ids = store.list_aggregates().expect("list_aggregates");
    assert!(ids.is_empty(), "fresh store has no aggregates, got {ids:?}");
}

#[tokio::test]
async fn list_aggregates_returns_ids_in_ascending_order_after_create() {
    let dir = TempDir::new().expect("tempdir");
    let store: Arc<PardosaFileEventStore<Tick>> =
        Arc::new(PardosaFileEventStore::open(dir.path()).expect("open store"));

    // Create three aggregates; the store auto-assigns ids 1, 2, 3.
    let (id_a, _) = store.create(vec![Tick(1)], ctx()).await.expect("create a");
    let (id_b, _) = store.create(vec![Tick(2)], ctx()).await.expect("create b");
    let (id_c, _) = store.create(vec![Tick(3)], ctx()).await.expect("create c");

    let ids = store.list_aggregates().expect("list_aggregates");
    assert_eq!(
        ids,
        vec![id_a, id_b, id_c],
        "list_aggregates must return ascending ids: got {ids:?}"
    );
}

#[tokio::test]
async fn list_aggregates_persists_across_reopen() {
    let dir = TempDir::new().expect("tempdir");

    let assigned: Vec<_> = {
        let store: PardosaFileEventStore<Tick> =
            PardosaFileEventStore::open(dir.path()).expect("open store");
        let (a, _) = store.create(vec![Tick(1)], ctx()).await.expect("create a");
        let (b, _) = store.create(vec![Tick(2)], ctx()).await.expect("create b");
        vec![a, b]
    };

    // Re-open the same directory; list_aggregates must surface both.
    let reopened: PardosaFileEventStore<Tick> =
        PardosaFileEventStore::open(dir.path()).expect("reopen store");
    let ids = reopened.list_aggregates().expect("list_aggregates");
    assert_eq!(
        ids, assigned,
        "reopen must surface previously-persisted aggregates"
    );
}

#[tokio::test]
async fn list_aggregates_skips_foreign_files() {
    let dir = TempDir::new().expect("tempdir");
    let store: PardosaFileEventStore<Tick> =
        PardosaFileEventStore::open(dir.path()).expect("open store");

    let (real_id, _) = store
        .create(vec![Tick(1)], ctx())
        .await
        .expect("create one");

    // Drop a foreign file in the store directory. The `.lock` file
    // exists already (advisory flock); add an unrelated `.txt` and a
    // `.pardosa` file whose stem is not a u64.
    std::fs::write(dir.path().join("notes.txt"), b"foreign").expect("write notes");
    std::fs::write(dir.path().join("not-a-u64.pardosa"), b"").expect("write stub");

    let ids = store.list_aggregates().expect("list_aggregates");
    assert_eq!(
        ids,
        vec![real_id],
        "foreign files must be skipped; got {ids:?}"
    );
}
