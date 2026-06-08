//! Integration-level MIRROR ratchets for `FiberIndex<K>`
//! (ADR-0023 D1–D6).
//!
//! Covers ADR-0023 §Verification items #2 (no implicit
//! construction), #4 (explicit construction), #5 (per-journal
//! scope), #9 (rebuild determinism), #10 (log-replay
//! equivalence), #11 (Empty/Unique/Diverged shape), #12
//! (append-side divergence observable, never refused), #17
//! (cross-journal isolation), and #20 (K is opaque — a
//! panic-on-`Debug` K never causes pardosa to panic). Item #1
//! is pinned by the unchanged
//! `tests/event_store_negative_gates.rs`.
use pardosa::prelude::*;
use pardosa::store::FiberLookup;
use tempfile::TempDir;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Order {
    customer_id: u64,
    total_cents: u64,
}
impl HasEventSchemaSource for Order {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
impl Validate for Order {
    type Error = core::convert::Infallible;
    fn validate(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}
fn extract_customer(e: &Event<Order>) -> std::iter::Once<u64> {
    std::iter::once(e.domain_event().customer_id)
}
fn new_path(td: &TempDir, label: &str) -> std::path::PathBuf {
    td.path().join(format!("{label}.pgno"))
}
fn open_store(path: &std::path::Path) -> EventStore<Order> {
    EventStore::create(path).expect("create")
}
#[test]
fn empty_lookup_yields_empty_shape() {
    let td = TempDir::new().unwrap();
    let store = open_store(&new_path(&td, "empty"));
    let idx = store.reader().fiber_index(extract_customer);
    assert_eq!(idx.lookup(&42), FiberLookup::Empty);
    assert_eq!(idx.key_count(), 0);
}
#[test]
fn unique_lookup_after_single_begin() {
    let td = TempDir::new().unwrap();
    let mut store = open_store(&new_path(&td, "unique"));
    let r = store
        .writer()
        .begin(Order {
            customer_id: 7,
            total_cents: 100,
        })
        .expect("begin");
    let fid = r.fiber().fiber_id();
    let _ = store.writer().sync().expect("sync");
    let idx = store.reader().fiber_index(extract_customer);
    assert_eq!(idx.lookup(&7), FiberLookup::Unique(fid));
}
#[test]
fn divergence_when_two_fibers_share_key() {
    let td = TempDir::new().unwrap();
    let mut store = open_store(&new_path(&td, "diverged"));
    let _ = store
        .writer()
        .begin(Order {
            customer_id: 9,
            total_cents: 1,
        })
        .expect("begin a");
    let _ = store
        .writer()
        .begin(Order {
            customer_id: 9,
            total_cents: 2,
        })
        .expect("begin b");
    let _ = store.writer().sync().expect("sync");
    let idx = store.reader().fiber_index(extract_customer);
    match idx.lookup(&9) {
        FiberLookup::Diverged { fibers } => {
            assert_eq!(fibers.len(), 2);
        }
        other => panic!("expected Diverged, got {other:?}"),
    }
}
#[test]
fn append_succeeds_when_extractor_emits_already_seen_key() {
    let td = TempDir::new().unwrap();
    let mut store = open_store(&new_path(&td, "no_refusal"));
    let _ = store
        .writer()
        .begin(Order {
            customer_id: 13,
            total_cents: 100,
        })
        .expect("first begin");
    let third = store
        .writer()
        .begin(Order {
            customer_id: 13,
            total_cents: 200,
        })
        .expect("second begin must not refuse on key divergence");
    let _ = third.event_id();
    let _ = store.writer().sync().expect("sync");
    let idx = store.reader().fiber_index(extract_customer);
    match idx.lookup(&13) {
        FiberLookup::Diverged { fibers } => assert_eq!(fibers.len(), 2),
        other => panic!("expected Diverged, got {other:?}"),
    }
}
#[test]
fn two_fiber_index_calls_yield_logically_equivalent_indices() {
    let td = TempDir::new().unwrap();
    let mut store = open_store(&new_path(&td, "deterministic"));
    for c in [11u64, 22, 33, 11] {
        let _ = store
            .writer()
            .begin(Order {
                customer_id: c,
                total_cents: 1,
            })
            .expect("begin");
    }
    let _ = store.writer().sync().expect("sync");
    let a = store.reader().fiber_index(extract_customer);
    let b = store.reader().fiber_index(extract_customer);
    for c in [11u64, 22, 33, 44] {
        assert_eq!(a.lookup(&c), b.lookup(&c));
    }
    assert_eq!(a.key_count(), b.key_count());
}
#[test]
fn cursor_replay_yields_the_same_mapping_the_index_exposes() {
    let td = TempDir::new().unwrap();
    let store_path = new_path(&td, "replay");
    let sidecar = td.path().join("replay.cur");
    let mut store: EventStore<Order> = open_store(&store_path);
    for c in [1u64, 2, 2, 3] {
        let _ = store
            .writer()
            .begin(Order {
                customer_id: c,
                total_cents: 1,
            })
            .expect("begin");
    }
    let _ = store.writer().sync().expect("sync");
    let idx = store.reader().fiber_index(extract_customer);
    let mut by_cursor: FiberIndex<u64> = FiberIndex::empty();
    let mut cursor: LineCursor<Order> = store.reader().cursor(&sidecar).expect("cursor");
    for ev in cursor.tail() {
        let ev = ev.expect("tail event");
        by_cursor.observe(&ev, extract_customer);
    }
    for c in [1u64, 2, 3, 4] {
        assert_eq!(idx.lookup(&c), by_cursor.lookup(&c), "mismatch at K={c}");
    }
}
#[test]
fn separate_indices_per_key_family_are_independent() {
    let td = TempDir::new().unwrap();
    let mut store = open_store(&new_path(&td, "families"));
    let _ = store
        .writer()
        .begin(Order {
            customer_id: 5,
            total_cents: 200,
        })
        .expect("begin");
    let _ = store.writer().sync().expect("sync");
    let by_customer = store.reader().fiber_index(extract_customer);
    let by_total = store
        .reader()
        .fiber_index(|e: &Event<Order>| std::iter::once(e.domain_event().total_cents));
    assert!(matches!(by_customer.lookup(&5), FiberLookup::Unique(_)));
    assert!(matches!(by_total.lookup(&200), FiberLookup::Unique(_)));
    assert_eq!(by_customer.lookup(&200), FiberLookup::Empty);
    assert_eq!(by_total.lookup(&5), FiberLookup::Empty);
}
#[test]
fn two_journals_do_not_share_index_state() {
    let td = TempDir::new().unwrap();
    let mut store_a = open_store(&new_path(&td, "journal_a"));
    let mut store_b = open_store(&new_path(&td, "journal_b"));
    let _ = store_a
        .writer()
        .begin(Order {
            customer_id: 100,
            total_cents: 1,
        })
        .expect("begin a");
    let _ = store_b
        .writer()
        .begin(Order {
            customer_id: 100,
            total_cents: 1,
        })
        .expect("begin b");
    let _ = store_a.writer().sync().expect("sync a");
    let _ = store_b.writer().sync().expect("sync b");
    let idx_a = store_a.reader().fiber_index(extract_customer);
    let idx_b = store_b.reader().fiber_index(extract_customer);
    assert_eq!(idx_a.key_count(), 1);
    assert_eq!(idx_b.key_count(), 1);
    let fid_a = match idx_a.lookup(&100) {
        FiberLookup::Unique(f) => f,
        other => panic!("expected Unique on A, got {other:?}"),
    };
    let fid_b = match idx_b.lookup(&100) {
        FiberLookup::Unique(f) => f,
        other => panic!("expected Unique on B, got {other:?}"),
    };
    let _ = fid_a;
    let _ = fid_b;
}
#[test]
fn empty_store_opens_and_operates_without_naming_fiber_index() {
    let td = TempDir::new().unwrap();
    let store: EventStore<Order> = open_store(&new_path(&td, "no_index_path"));
    let reader = store.reader();
    let _ = reader.frontier();
    let _ = reader.publish_watermark();
}
#[test]
fn pardosa_never_debug_prints_application_owned_keys() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static PANIC_K: AtomicBool = AtomicBool::new(false);
    #[derive(Clone, PartialEq, Eq, Hash)]
    struct PanickyKey(u64);
    impl std::fmt::Debug for PanickyKey {
        fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            PANIC_K.store(true, Ordering::SeqCst);
            panic!("pardosa must never debug-print K (ADR-0023 D6)");
        }
    }
    let td = TempDir::new().unwrap();
    let mut store = open_store(&new_path(&td, "opaque_k"));
    for c in [1u64, 2] {
        let _ = store
            .writer()
            .begin(Order {
                customer_id: c,
                total_cents: 1,
            })
            .expect("begin");
    }
    let _ = store.writer().sync().expect("sync");
    let idx: FiberIndex<PanickyKey> = store
        .reader()
        .fiber_index(|e: &Event<Order>| std::iter::once(PanickyKey(e.domain_event().customer_id)));
    assert_eq!(idx.key_count(), 2);
    let _ = idx.lookup(&PanickyKey(1));
    assert!(
        !PANIC_K.load(Ordering::SeqCst),
        "pardosa called Debug on the application-owned K (ADR-0023 D6 violation)"
    );
}
