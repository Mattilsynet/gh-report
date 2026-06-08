//! Conformance harness for the cfg-gated in-memory authoritative
//! backend (ADR-0022 §D11) — second in-crate impl proving the
//! sealed surface admits non-File substrates without widening
//! either sealing supertrait.
//!
//! `InMemoryBackend` satisfies both [`AuthoritativeBackend`] (§D1)
//! and [`BackendSink`] (§D2) on a single type. The split-adapter
//! §D11 pattern is reserved for the first real cross-crate backend.
//!
//! Positional only — `EventStore::open_*` not involved. §D12
//! restricts the audit allowlist to `open_with_backend`, which
//! takes a concrete `PgnoBackend`.
use pardosa::store::test_support::InMemoryBackend;
use pardosa::store::{AuthoritativeBackend, BackendSink};
#[test]
fn in_memory_backend_satisfies_authoritative_backend_marker() {
    fn requires_authoritative_backend<B: AuthoritativeBackend>(_: &B) {}
    let backend = InMemoryBackend::new();
    requires_authoritative_backend(&backend);
}
#[test]
fn in_memory_backend_satisfies_backend_sink_trait() {
    fn requires_backend_sink<S: BackendSink>(_: &S) {}
    let backend = InMemoryBackend::new();
    requires_backend_sink(&backend);
}
#[test]
fn append_returns_post_write_position_and_extends_storage() {
    let mut backend = InMemoryBackend::new();
    let p1 = backend.append(b"hello").expect("append 1");
    assert_eq!(p1.as_u64(), 5, "first append moves position to byte 5");
    assert_eq!(backend.bytes(), b"hello", "storage carries staged bytes");
    let p2 = backend.append(b" world").expect("append 2");
    assert_eq!(p2.as_u64(), 11, "second append moves position to byte 11");
    assert!(p1 < p2, "AckPosition monotonic within a single backend");
    assert_eq!(
        backend.bytes(),
        b"hello world",
        "storage extends across appends"
    );
}
#[test]
fn sync_advances_durability_floor_to_current_length() {
    let mut backend = InMemoryBackend::new();
    let appended = backend.append(b"durable-payload").expect("append");
    let synced = backend.sync().expect("sync");
    assert_eq!(
        synced, appended,
        "sync returns position equal to last append on in-memory backend",
    );
    assert_eq!(
        synced.as_u64(),
        "durable-payload".len() as u64,
        "sync floor advances to staged byte length",
    );
}
#[test]
fn sync_without_intervening_append_is_idempotent() {
    let mut backend = InMemoryBackend::new();
    let _ = backend.append(b"once").expect("append");
    let first = backend.sync().expect("first sync");
    let second = backend.sync().expect("second sync");
    assert_eq!(first, second, "no-op sync is idempotent");
}
#[test]
fn ack_position_chain_is_strictly_monotonic_across_mixed_ops() {
    let mut backend = InMemoryBackend::new();
    let a = backend.append(b"a").expect("a");
    let b = backend.append(b"bb").expect("bb");
    let c = backend.append(b"ccc").expect("ccc");
    let d = backend.sync().expect("sync");
    assert!(a < b, "monotonic across appends (a < b)");
    assert!(b < c, "monotonic across appends (b < c)");
    assert_eq!(c, d, "sync returns the same position as the last append");
    assert_eq!(a.as_u64(), 1);
    assert_eq!(b.as_u64(), 3);
    assert_eq!(c.as_u64(), 6);
}
