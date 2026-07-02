#![forbid(unsafe_code)]
#[cfg(not(target_pointer_width = "64"))]
compile_error!("pardosa requires a 64-bit target (usize must be at least 8 bytes)");
extern crate self as pardosa;
/// Workspace auto-trait policy macro (mission rescue-pardosa-59y0).
///
/// Uses **stable built-in `Send`/`Sync`** only — no `auto trait` item, no
/// `negative_impls`. See `pardosa-schema/src/lib.rs` for full doctrine.
/// Buckets: `SendSync { T, ... }` verifies `T: Send + Sync`;
/// `SendOnly { T, ... }` verifies `T: Send` (rationale lives at the type's
/// site, e.g. ADR-0014 §F5 for `Dragline`); `NotSend { T, ... }` is
/// documentation-only (stable Rust cannot assert `!Send`).
macro_rules! assert_auto_traits {
    (
        $(SendSync { $($ss:ty),* $(,)? })? $(SendOnly { $($so:ty),* $(,)? })? $(NotSend {
        $($ns:ty),* $(,)? })?
    ) => {
        const _ : fn () = || { fn __assert_send_sync < T : Send + Sync > () {} fn
        __assert_send < T : Send > () {} $($(__assert_send_sync::<$ss > ();)*)?
        $($(__assert_send::<$so > ();)*)? $($(let _ = ::core::marker::PhantomData::<$ns
        >;)*)? };
    };
}
#[doc(hidden)]
pub mod __derive_support;
pub(crate) mod authoritative;
pub(crate) mod backend;
pub(crate) mod cursor;
pub(crate) mod dragline;
pub(crate) mod durability;
pub(crate) mod error;
pub(crate) mod event;
pub(crate) mod fiber;
pub(crate) mod fiber_index;
pub(crate) mod fiber_state;
pub(crate) mod frontier;
pub(crate) mod migrate;
pub(crate) mod persist;
pub mod prelude;
pub mod store;
pub(crate) mod typed;
pub(crate) use dragline::AppendResult;
pub(crate) use error::PardosaError;
pub(crate) use event::{Event, EventId, FiberId, Index};
pub(crate) use fiber::Fiber;
pub(crate) use fiber_index::{ExtractError, FiberIndex};
pub(crate) use fiber_state::FiberState;
// AUTO-TRAIT-POLICY-BEGIN
assert_auto_traits! {
    SendSync { durability::Lsn, durability::AckPosition, event::EnvelopeError,
    event::Index, event::IndexTooLargeForUsize, event::Precursor, event::FiberId,
    EventId, Event < u64 >, error::FiberInvariantKind, error::FiberLenReason,
    error::IndexOrderingKind, error::LinevecAppendKind, error::IntegrityKind,
    error::FromRawPartsKind, backend::diagnostics::NatsFailureClass, PardosaError, fiber_state::FiberState,
    fiber_state::FiberMigrationPolicy, fiber_state::LockedRescuePolicy,
    fiber_state::FiberAction, frontier::Frontier, error::PublishError,
    error::BackendError, error::BackendOp, error::RuntimeFailureKind,
    error::PublisherBacklogKind, frontier::JetStreamFrontierPublisher,
    backend::PgnoFileSink < std::fs::File >, backend::PgnoFileSink < std::io::Cursor <
    std::vec::Vec < u8 >>>, authoritative::PgnoBackend, authoritative::JetStreamBackend,
    persist::RehydrateInvariant, persist::Error, persist::UnpersistableKind,
    persist::CheckedReplayKind, persist::CheckedEventStream < std::io::Cursor <
    std::vec::Vec < u8 >>, u64 >, persist::ValidatedReplayError < std::io::Error >,
    persist::ValidatedEventStream < std::io::Cursor < std::vec::Vec < u8 >>, u64 >,
    store::LiveFiber, store::DetachedFiber, store::AppendReceipt, store::DetachReceipt,
    migrate::MigrationReport < std::io::Cursor < std::vec::Vec < u8 >>>,
    migrate::MigrationError < std::io::Error >, store::LineCursor < u64 >,
    store::StoreMetadata, store::OfflineRecoveryPlan, store::OfflineRecoveryStatus,
    store::CausalChainError, fiber_index::FiberIndex < u64 >, fiber_index::FiberLookup <
    FiberId >, fiber_index::ExtractError, } SendOnly {
    store::inner::EventStore < u64, std::io::Cursor < std::vec::Vec < u8 >>>,
    store::inner::EventStore < u64, std::fs::File >, store::inner::StoreWriter <'static,
    u64, std::io::Cursor < std::vec::Vec < u8 >>>, store::FiberHistoryIter <'static, u64
    >, store::CausalChainIter <'static, u64 >, store::CausalChainStrictIter <'static, u64
    >, store::HistoryStream <'static, u64 >, } NotSend { store::inner::StoreReader
    <'static, u64, std::io::Cursor < std::vec::Vec < u8 >>>, store::inner::FiberHistory
    <'static, u64, std::io::Cursor < std::vec::Vec < u8 >>>, store::inner::CausalChain
    <'static, u64, std::io::Cursor < std::vec::Vec < u8 >>>, }
}
#[cfg(any(test, feature = "test-support"))]
assert_auto_traits! {
    SendSync { authoritative::fake::InMemoryBackend,
    backend::test_support_jetstream_recovery::JetStreamRecoveryJournal < u64 >,
    backend::test_support_jetstream_recovery::JetStreamRecoveryJournalSyncError,
    backend::test_support_jetstream_recovery::JetStreamRecoveryJournalRehydrateError, }
}
// AUTO-TRAIT-POLICY-END
