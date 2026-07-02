//! Adopter-facing typed event-log façade (ADR-0018).
//!
//! Single appliance [`EventStore<T>`] composed from runtime-ring
//! primitives; additive on ADR-0002.
//!
//! Path-backed `W = File` is the adopter form; public types
//! default `W = File`. Three distinct read views (§D3/§D5):
//!
//! * `StoreReader::fiber` → [`FiberHistory`]: per-fiber,
//!   in-memory; `FiberId` dragline-local (ADR-0003 §1).
//! * `StoreReader::causal_chain` → [`CausalChain`]: same-fiber
//!   precursor walk; cross-fiber causality is payload-level (§D6).
//! * `StoreReader::cursor` → [`LineCursor`]: global ACK/resume,
//!   sidecar-backed (ADR-0011 §D2/§D5; Amendment 2).
//!
//! Submodules: [`replay`], [`migrate`]. [`migrate::migrate_keep`]
//! is the only public migration path until ADR-0019 lands.
/// Sealed marker trait identifying a substrate eligible to back
/// an [`EventStore`] via `EventStore::<T>::open_with_backend`
/// (ADR-0022 §D1, §D11, §D12).
///
/// Sealed via private supertrait; in-crate impls only.
/// Method-less — substrate behaviour lives on [`BackendSink`].
/// First in-crate impl: [`PgnoBackend`]. External-impl posture
/// pinned by `tests/ui/no_external_authoritative_backend_impl.rs`.
pub use crate::authoritative::AuthoritativeBackend;
/// JetStream-backed [`AuthoritativeBackend`] handle — the
/// second in-crate sealed admission accepted by
/// `EventStore::<T>::open_with_backend` (ADR-0022 §D11,
/// §D1; mission `event-storage-dual-backend-20260606`).
///
/// Adopters obtain a `JetStreamBackend` from
/// [`JetStreamBackend::open`] wrapping a
/// [`pardosa_nats::JetStreamHandle`] minted by
/// [`pardosa_nats::JetStreamBackend::open`], then feed it
/// into `EventStore::<T>::open_with_backend`. The wrapped
/// substrate adapter is `pub(crate)`: the
/// [`crate::backend::BackendSink`] surface (append/sync) on the
/// `JetStream` substrate stays sealed at the adopter boundary, so
/// no `JetStream` reader/cursor API is exposed.
pub use crate::authoritative::JetStreamBackend;
/// `.pgno` path-backed [`AuthoritativeBackend`] handle — the
/// first in-crate sealed impl per ADR-0022 §D11.
///
/// Adopters obtain a `PgnoBackend` from
/// [`PgnoBackend::open`] and feed it into
/// `EventStore::<T>::open_with_backend`. The wrapped path is
/// not part of the public surface (only the runtime crate
/// reaches it via a `pub(crate)` accessor). Construction does
/// not touch the filesystem; framing, schema-hash, and
/// contiguity checks happen at the
/// `open_with_backend` call site against the same `.pgno`
/// rehydrate pipeline used by `EventStore::open`.
pub use crate::authoritative::PgnoBackend;
/// Sealed substrate contract a backend implements to participate
/// in [`EventStore`] construction (ADR-0022 §D2, §D11). Sealed via
/// private supertrait; in-crate impls only. First impl:
/// [`PgnoFileSink`] over `.pgno`/[`std::fs::File`].
///
/// Adopters using path constructors never name `BackendSink`
/// directly. Re-exported for backend-author crates and callers of
/// `open_with_backend` (ADR-0022 §D12).
pub use crate::backend::BackendSink;
/// `.pgno`/[`std::fs::File`]-backed [`BackendSink`] adapter —
/// the first in-crate sealed impl per ADR-0022 §D11. Wraps any
/// [`pardosa_file::Syncable`] + [`std::io::Seek`] sink (default
/// `W = std::fs::File`); preserves ADR-0006 byte layout
/// unchanged because the underlying writer is the same
/// [`pardosa_file::Syncable`] used on the existing journal sync
/// path.
pub use crate::backend::PgnoFileSink;
/// Re-exported so backend-author crates (`pardosa-nats` per
/// ADR-0022 §D10, plus the in-crate `.pgno` adapter) can name the
/// opaque positional primitive the sealed `BackendSink` returns
/// from `append` / `sync` (ADR-0022 §D2) directly from
/// `pardosa::store`, without crossing a `pub(crate)` boundary.
///
/// `AckPosition` is **position metadata**, not durability evidence
/// on its own; durability is earned by `sync` returning the
/// position at which all preceding bytes are stable (ADR-0022 §D2).
/// See [`AckPosition`] for the position-vs-durability contract and
/// the per-instance ordering guarantee.
pub use crate::durability::AckPosition;
/// Re-exports of the typed-event primitives that appear in public
/// `pardosa::store` signatures (ADR-0018 § Naming). Adopters can
/// satisfy every signature of [`EventStore`], [`StoreReader`],
/// [`StoreWriter`], [`FiberHistory`], [`CausalChain`], and
/// [`LineCursor`] using only names exported from `pardosa::store`,
/// `pardosa_schema`, and `pardosa_wire`. `pardosa::store` is the
/// sole adopter-facing module; no public root-level alternative
/// exists (ADR-0018 Amendment 1).
pub use crate::durability::Lsn;
/// Re-exported so adopters can name the error type returned from
/// `EventStore::<T>::create`, `EventStore::<T>::open_validated`,
/// `StoreWriter` verbs, and `StoreReader::cursor` directly from
/// `pardosa::store` (ADR-0018 § Naming, ADR-0018 Amendment 1).
pub use crate::error::PardosaError;
/// Re-exported so backend-author crates can name the failure
/// taxonomy returned from the substrate's append / sync / watermark
/// path (ADR-0022 §D11). The enum is `#[non_exhaustive]` per
/// ADR-0007; the initial variants pinned at acceptance were
/// `Timeout`, `RuntimeFailure`, `Publish`, and `PublisherBacklog`
/// (ADR-0022 §D7, §D8), with later additive backend-projection
/// variants permitted by that non-exhaustive contract.
///
/// Adopters who only consume an `EventStore<T>` do not see
/// `BackendError`: the runtime ring translates substrate failures
/// to [`PardosaError`] at the public boundary. These re-exports
/// exist for adopters writing a `BackendSink` impl in a sibling
/// substrate crate.
pub use crate::error::{BackendError, BackendOp, PublisherBacklogKind, RuntimeFailureKind};
pub use crate::event::{EnvelopeError, Event, EventId, FiberId, Index, Precursor};
/// Opaque live-fiber state token returned by
/// `StoreWriter::begin`, `StoreWriter::append`, and
/// `StoreWriter::resume`, and round-tripped via
/// [`AppendReceipt::fiber`].
///
/// Holding a `LiveFiber` is the only public way to append a
/// continuation or to detach. The wrapped [`FiberId`] is
/// observable; the constructor is private — a fabricated
/// [`FiberId`] cannot be upgraded into write authority
/// (ADR-0017 §D1). `resume` does not accept a `LiveFiber` —
/// rescue requires a [`DetachedFiber`].
///
/// `PartialEq`/`Eq`/`Hash` deliberately not derived: two
/// `LiveFiber` values for one [`FiberId`] cannot legitimately
/// coexist. Key collections by `.fiber_id()`.
#[derive(Debug)]
pub struct LiveFiber(pub(crate) FiberId);
impl LiveFiber {
    /// The underlying [`FiberId`] observed via the public
    /// reader / writer facades. Borrow-based — observing the
    /// [`FiberId`] does not consume the linear token; the
    /// transition methods (`StoreWriter::append`,
    /// `StoreWriter::detach`) remain the only paths that move
    /// the token by value. Read-only — there is no
    /// `LiveFiber::from(_: FiberId)` entry.
    #[must_use]
    pub fn fiber_id(&self) -> FiberId {
        self.0
    }
}
/// Opaque detached-fiber state token returned by
/// `StoreWriter::detach` and carried by [`DetachReceipt::fiber`].
///
/// Holding a `DetachedFiber` is the only public way to rescue a
/// fiber via `StoreWriter::resume`. As with [`LiveFiber`], the
/// wrapped [`FiberId`] is observable; the constructor is private:
///
/// - `resume(DetachedFiber, _)` — typechecks.
/// - `append(DetachedFiber, _)` — does not compile.
///
/// Structural equality is not derived; key by `.fiber_id()`.
#[derive(Debug)]
pub struct DetachedFiber(pub(crate) FiberId);
impl DetachedFiber {
    /// The underlying [`FiberId`] observed via the public
    /// reader / writer facades. Borrow-based — observing the
    /// [`FiberId`] does not consume the linear token;
    /// `StoreWriter::resume` remains the only path that moves
    /// the token by value. Read-only — there is no
    /// `DetachedFiber::from(_: FiberId)` entry.
    #[must_use]
    pub fn fiber_id(&self) -> FiberId {
        self.0
    }
}
/// Receipt returned by the fiber-authoring writer methods that
/// produce or preserve a [`LiveFiber`] state — i.e.
/// `StoreWriter::begin`, `StoreWriter::append`, and
/// `StoreWriter::resume`.
///
/// Carries the assigned [`EventId`] and the [`LiveFiber`] token
/// that authorises further appends on the same fiber. Pair the
/// receipt with `StoreWriter::sync` to drive durability;
/// `event_id` is in-memory only until `sync` succeeds (ADR-0010).
#[derive(Debug)]
#[must_use]
pub struct AppendReceipt {
    pub(crate) event_id: EventId,
    pub(crate) fiber: LiveFiber,
}
impl AppendReceipt {
    /// The newly minted [`EventId`]. Borrow-based — observing the
    /// id does not consume the receipt; [`AppendReceipt::fiber`]
    /// remains the only path that moves the linear token out.
    #[must_use]
    pub fn event_id(&self) -> EventId {
        self.event_id
    }
    /// [`LiveFiber`] state token for the receipt's fiber.
    /// Pass back into `StoreWriter::append` to continue the
    /// fiber, or into `StoreWriter::detach` to detach it.
    #[must_use]
    pub fn fiber(self) -> LiveFiber {
        self.fiber
    }
}
/// Receipt returned by `StoreWriter::detach`.
///
/// Carries the assigned [`EventId`] of the detach event plus the
/// [`DetachedFiber`] token that authorises subsequent
/// `StoreWriter::resume` calls on the same fiber. As with
/// [`AppendReceipt`], `event_id` is in-memory only until
/// `StoreWriter::sync` succeeds (ADR-0010).
#[derive(Debug)]
#[must_use]
pub struct DetachReceipt {
    pub(crate) event_id: EventId,
    pub(crate) fiber: DetachedFiber,
}
impl DetachReceipt {
    /// The newly minted [`EventId`] of the detach event.
    /// Borrow-based — observing the id does not consume the
    /// receipt; [`DetachReceipt::fiber`] remains the only path
    /// that moves the linear token out.
    #[must_use]
    pub fn event_id(&self) -> EventId {
        self.event_id
    }
    /// [`DetachedFiber`] state token for the receipt's fiber.
    /// Pass back into `StoreWriter::resume` to return the
    /// fiber to `Defined`.
    #[must_use]
    pub fn fiber(self) -> DetachedFiber {
        self.fiber
    }
}
/// Re-exports of the in-memory `FiberIndex<K>` routing accelerator
/// (ADR-0023 D5 default-public). Adopters opt in by constructing
/// an index via `StoreReader::fiber_index`;
/// a journal opened without that call pays no per-event indexing
/// cost (D5). `K` is application-owned and opaque to pardosa (D6);
/// lookups return one of the three typed shapes in
/// [`FiberLookup`] (D4); the index is in-memory only and
/// log-derived (D1, D2, D5).
pub use crate::fiber_index::{ExtractError, FiberIndex, FiberLookup};
/// Declarative lifecycle state of a fiber, as observed by
/// `FiberHistory::state` (ADR-0018 §D3 (a), §D8).
///
/// Re-exported from the substrate `fiber_state` module so that
/// adopters of `pardosa::store` can name the return type of
/// `FiberHistory::state` without crossing a `pub(crate)`
/// boundary. The variants and transitions are defined by the
/// substrate (see `fiber_state.rs` `TRANSITIONS` table); the
/// re-export here is identity — same type, same variants, same
/// `non_exhaustive` discipline.
pub use crate::fiber_state::FiberState;
/// Re-exported so adopters can name the BLAKE3 chain-frontier type
/// returned from `StoreReader::frontier` directly from
/// `pardosa::store` (ADR-0018 § Naming; ADR-0004 § Security model).
///
/// `Frontier` is a tamper-evident rolling digest over the persisted
/// event line — useful for cross-replica comparison or feeding a
/// downstream attestor. Identity re-export of
/// [`crate::frontier::Frontier`]; same type, same `GENESIS`
/// constant, same `roll` / `as_bytes` accessors.
pub use crate::frontier::Frontier;
/// Re-exported so adopters can name the error type returned from
/// `EventStore::<T>::open_validated` directly from `pardosa::store`
/// (ADR-0018 § Naming; Fiber-semantics correctness goal 6).
pub use crate::persist::ValidatedReplayError;
/// Re-exported so adopters constructing a path-backed
/// `EventStore::<T>::create` can satisfy its
/// `T: HasEventSchemaSource` bound directly from
/// `pardosa::store` (ADR-0018 § Naming, ADR-0018 Amendment 1).
pub use crate::typed::HasEventSchemaSource;
use pardosa_file::Syncable;
pub use pardosa_file::manifest::{RecoveryError, RecoveryOutcome, RecoveryReaderErrorKind};
pub use pardosa_schema::{Decode, Encode, GenomeSafe, Validate};
use std::path::{Path, PathBuf};
/// Adopter-facing replay surface (ADR-0018 § Naming).
///
/// Streaming, no-rehydrate primitives so adopters can audit a
/// `.pgno` without materialising an `EventStore<T>`.
///
/// `stream_checked` yields `Result<Event<T>, Error>` and stops on
/// the first error (poison-on-error). [`replay::CheckedReplayKind`]
/// enumerates structural rejections;
/// [`replay::Error::is_tamper_suspicious`] separates tamper-shaped
/// failures (checksum, precursor hash, broken chain) from
/// schema/decode/IO drift. `stream_validated` adds per-event
/// payload validation via [`ValidatedReplayError`].
pub mod replay {
    pub use crate::persist::{
        CheckedEventStream, CheckedReplayKind, Error, ValidatedEventStream, ValidatedReplayError,
        stream_checked, stream_validated,
    };
}
/// Store-scoped public migration surface (ADR-0018 §D7, ADR-0019).
///
/// At v0 the only public migration entry is the startup-blocking
/// out-of-band [`migrate::migrate_keep`]; `MigrationPolicy` and
/// any in-place `open_with_migration` surface remain out of scope
/// until ADR-0019 is Accepted. `pardosa::store::migrate` is the
/// sole public path to `migrate_keep` (ADR-0018 Amendment 1).
pub mod migrate {
    pub use crate::migrate::{MigrationError, MigrationReport, migrate_keep};
}
/// NATS operational diagnostics owned by the `pardosa` adapter ring.
pub mod diagnostics {
    pub use crate::backend::diagnostics::{
        NatsFailureClass, classify_nats_failure, emit_nats_connect_diagnostics, error_chain_json,
        nats_failure_remediation, redact_nats_credentials,
    };
}
/// Store-scoped adopter-facing test affordances (ADR-0022 §D11).
///
/// Cfg-gated re-export of the in-memory `AuthoritativeBackend` /
/// `BackendSink` fake. Reachable from adopter tests under the
/// `test-support` feature and from in-tree tests under
/// `cfg(test)`; not part of the production-build public surface.
///
/// Placed inside `store.rs` (mirroring [`replay`] / [`migrate`])
/// rather than at the lib-root so the ADR-0018 sole-interface
/// audit's `pub mod` allowlist on `lib.rs` stays closed at
/// `store | prelude | __derive_support` (§D1 audit script
/// section 1).
#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    pub use crate::authoritative::fake::InMemoryBackend;
    pub use crate::backend::test_support_jetstream_recovery::{
        JetStreamRecoveryJournal, JetStreamRecoveryJournalRehydrateError,
        JetStreamRecoveryJournalSyncError, jetstream_recovery_journal,
    };
}
/// Store-scoped public publisher seam (ADR-0018 §12 bullet 3,
/// ADR-0015, ADR-0016 §§D5–D8).
///
/// Re-exports the substrate publisher contract so adopters can
/// supply a `Box<dyn FrontierPublisher>` to
/// `EventStore::<T>::open_with_publisher` without crossing a
/// `pub(crate)` boundary. The trait is the same instance the
/// substrate dispatches against — there is no façade-side
/// adapter — keeping ADR-0015 D2/D4 (publish-after-durable,
/// no-propagate) and ADR-0016 §D6 (durable-watermark recovery)
/// observable through the public surface.
pub use crate::error::PublishError;
pub use crate::frontier::FrontierPublisher;
/// Narrow JetStream-backed [`FrontierPublisher`] adapter
/// (mission `nats-phase5-publisher-soak-01`).
///
/// Re-exported so adopters running the Phase 5 publisher
/// failure/recovery soak can supply a JetStream-backed
/// publisher to `EventStore::<T>::open_with_publisher`
/// without crossing a `pub(crate)` boundary. Constructed from a
/// [`pardosa_nats::JetStreamHandle`]; no other public
/// configuration surface is added (ADR-0015/ADR-0016 publisher
/// seam preserved, ADR-0022 §D11 sibling-crate adapter pattern).
pub use crate::frontier::JetStreamFrontierPublisher;
/// Adopter-facing path-backed appliance (ADR-0018 § Naming).
///
/// Sole shape adopters can name on the runtime crate: the
/// generic-`W` form lives in the crate-internal `inner::EventStore`
/// under a `pub(crate)` module that is unreachable from downstream
/// crates. `W = std::fs::File` is fixed by this alias; the in-memory
/// generic-`W` substrate remains available to in-tree tests via
/// the `pub(crate)` path (`crate::store::inner::*`).
pub type EventStore<T> = inner::EventStore<T, std::fs::File>;
/// Adopter-facing writer handle borrowed from [`EventStore<T>`].
pub type StoreWriter<'a, T> = inner::StoreWriter<'a, T, std::fs::File>;
/// Adopter-facing reader handle borrowed from [`EventStore<T>`].
pub type StoreReader<'a, T> = inner::StoreReader<'a, T, std::fs::File>;
/// Adopter-facing per-fiber history view returned by
/// `StoreReader::fiber`.
pub type FiberHistory<'a, T> = inner::FiberHistory<'a, T, std::fs::File>;
/// Adopter-facing same-fiber causal-replay walk returned by
/// `StoreReader::causal_chain`.
pub type CausalChain<'a, T> = inner::CausalChain<'a, T, std::fs::File>;
/// W-independent items re-exported from the crate-internal `inner`
/// module so the public `pardosa::store::*` surface stays
/// single-path (ADR-0018 § Naming, Amendment 1).
pub use inner::{
    CausalChainError, CausalChainIter, CausalChainStrictIter, FiberHistoryIter, HistoryStream,
    LineCursor, OfflineRecoveryPlan, OfflineRecoveryStatus, StoreMetadata,
    plan_offline_pgno_recovery, recover_offline_pgno,
};
/// Generic-`W` shapes retained for the in-memory test substrate
/// (ADR-0018 § Naming permits sealing the generic form). Adopters
/// cannot name this module; the only public spelling of the appliance
/// is the `W = File`-specialised type aliases above.
pub(crate) mod inner;
