//! Adopter-surface smoke test: every type appearing in public
//! `pardosa::store` signatures, plus the store-scoped migration
//! entry, is nameable through `pardosa::store::*` alone. No import
//! references a root-level `reader` / `writer` module — those
//! modules are demoted to `pub(crate)` under ADR-0018 Amendment 1,
//! and `pardosa::store` is the sole adopter-facing runtime API.
//!
//! This pins the additive re-export surface introduced for
//! ADR-0018 SM01 (store-only adopter API). Removing or renaming
//! any of the re-exported names below will fail this compile.
#![allow(dead_code, unused_imports)]
use pardosa::store::migrate::{MigrationError, MigrationReport, migrate_keep};
use pardosa::store::replay::{
    CheckedEventStream, CheckedReplayKind, Error as ReplayError, ValidatedEventStream,
    ValidatedReplayError, stream_checked, stream_validated,
};
use pardosa::store::{
    AppendReceipt, CausalChain, Decode, DetachReceipt, DetachedFiber, Encode, EnvelopeError, Event,
    EventId, EventStore, FiberHistory, FiberId, FiberState, GenomeSafe, HasEventSchemaSource,
    Index, LineCursor, LiveFiber, Lsn, PardosaError, Precursor, StoreReader, StoreWriter, Validate,
};
use pardosa_schema::Timestamp;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Payload {
    when: Timestamp,
    v: u64,
}
impl HasEventSchemaSource for Payload {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
fn handles_reachable(
    _store: &EventStore<Payload>,
    _writer: &StoreWriter<'_, Payload>,
    _reader: &StoreReader<'_, Payload>,
    _hist: &FiberHistory<'_, Payload>,
    _chain: &CausalChain<'_, Payload>,
    _cursor: &LineCursor<Payload>,
) {
}
fn event_primitives_reachable(
    _ev: &Event<Payload>,
    _id: EventId,
    _fid: FiberId,
    _lsn: Lsn,
    _precursor: Precursor,
    _index: Index,
    _envelope_err: &EnvelopeError,
) {
}
fn fiber_and_error_reachable(
    _err: &PardosaError,
    _live: &LiveFiber,
    _detached: &DetachedFiber,
    _ar: &AppendReceipt,
    _dr: &DetachReceipt,
    _state: FiberState,
) {
}
fn requires_encode<T: Encode>() {}
fn requires_decode<T: Decode>() {}
fn requires_genome_safe<T: GenomeSafe>() {}
fn requires_validate<T: Validate>() {}
fn requires_has_source<T: HasEventSchemaSource>() {}
fn public_replay_reachable<R: std::io::Read + std::io::Seek, V: Validate + Decode + GenomeSafe>(
    _checked: &CheckedEventStream<R, u64>,
    _validated: &ValidatedEventStream<R, V>,
    _checked_kind: &CheckedReplayKind,
    _replay_err: &ReplayError,
    _validated_err: &ValidatedReplayError<std::io::Error>,
) {
    let _ = stream_checked::<R, u64>;
    let _ = stream_validated::<R, V>;
}
fn migrate_signature_is_reachable<Old, New, E, F>(
    old_path: &std::path::Path,
    new_path: &std::path::Path,
    upcast: F,
) -> Result<MigrationReport<std::fs::File>, MigrationError<E>>
where
    Old: Decode + GenomeSafe,
    New: Encode + GenomeSafe,
    F: FnMut(Event<Old>) -> Result<New, E>,
{
    migrate_keep::<Old, New, E, F>(old_path, new_path, upcast)
}
#[test]
fn store_only_imports_compile_for_path_backed_adopter_flow() {
    let dir = tempdir_for_test();
    let path = dir.join("smoke.pgno");
    let mut store = EventStore::<Payload>::create(&path).expect("create path-backed EventStore");
    let receipt = store
        .writer()
        .begin(Payload {
            when: Timestamp::from_nanos(1).expect("nonzero"),
            v: 7,
        })
        .expect("begin");
    let _live: LiveFiber = receipt.fiber();
    let _acked: Option<Lsn> = store.writer().acked_lsn();
    let lsn: Lsn = store.writer().sync().expect("sync");
    assert!(lsn.value() > 0);
    let _reader: StoreReader<'_, Payload> = store.reader();
}
#[test]
fn migrate_keep_is_reachable_through_store_migrate() {
    let _ = migrate_signature_is_reachable::<
        u64,
        u64,
        std::convert::Infallible,
        fn(Event<u64>) -> Result<u64, std::convert::Infallible>,
    >;
}
fn tempdir_for_test() -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    p.push(format!("pardosa-store-smoke-{nanos}"));
    std::fs::create_dir_all(&p).expect("mkdir tempdir");
    p
}
fn type_witness<T>(_: T) {}
fn reader_apis_do_not_require_encode<T: Decode + GenomeSafe>() {
    fn fiber_history_nameable<U: Decode + GenomeSafe>(_h: &FiberHistory<'_, U>) {}
    fn causal_chain_nameable<U: Decode + GenomeSafe>(_c: &CausalChain<'_, U>) {}
    fn line_cursor_nameable<U: Decode + GenomeSafe>(_c: &LineCursor<U>) {}
    type_witness::<fn(&std::path::Path) -> Result<pardosa::store::StoreMetadata, PardosaError>>(
        EventStore::<T>::metadata,
    );
    type_witness::<fn(&FiberHistory<'_, T>)>(fiber_history_nameable::<T>);
    type_witness::<fn(&CausalChain<'_, T>)>(causal_chain_nameable::<T>);
    type_witness::<fn(&LineCursor<T>)>(line_cursor_nameable::<T>);
}
fn open_validated_does_not_require_encode<T: Decode + GenomeSafe + Validate>() {
    type OpenValidatedFn<T> =
        fn(&std::path::Path) -> Result<EventStore<T>, ValidatedReplayError<<T as Validate>::Error>>;
    type_witness::<OpenValidatedFn<T>>(EventStore::<T>::open_validated);
}
