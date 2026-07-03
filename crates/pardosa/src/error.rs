use crate::event::{FiberId, Index, IndexTooLargeForUsize};
use crate::fiber_state::{FiberAction, FiberState, LockedRescuePolicy};
use crate::frontier::Frontier;
/// Structured taxonomy of fiber invariant violations.
///
/// Each variant carries the actual structured data the violating site has
/// in hand at the moment of failure — no formatted strings, no debug dumps.
/// Solon §4.2: the type rejects illegal histories, not a free-form string.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FiberInvariantKind {
    /// Fiber length constraint violated.
    FiberLen(FiberLenReason),
    /// Index ordering invariant violated within a single fiber.
    IndexOrdering(IndexOrderingKind),
    /// `Linevec::append_validated` rejected an event.
    LinevecAppend(LinevecAppendKind),
    /// Dragline-wide integrity check failed.
    Integrity(IntegrityKind),
    /// A `u64` counter cannot advance because it is already at `u64::MAX`.
    /// `counter` names the saturated counter (e.g. `"next_event_id"`).
    CounterSaturated { counter: &'static str },
}
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FiberLenReason {
    /// `len` must be `>= 1`; got the contained value.
    Zero,
    /// `len.checked_add(1)` overflowed `u64`.
    Overflow,
}
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IndexOrderingKind {
    /// `Fiber::new`: `current < anchor`.
    CurrentBelowAnchor { anchor: Index, current: Index },
    /// `Fiber::check_advance`: `new_current <= current`.
    NewCurrentNotAfterCurrent { current: Index, new_current: Index },
}
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LinevecAppendKind {
    /// `event.event_id()` did not match the caller-supplied expected id.
    EventIdMismatch { actual: u64, expected: u64 },
    /// New `event_id` was not strictly greater than the last present in the line.
    EventIdNotMonotonic { event_id: u64, last_event_id: u64 },
    /// Precursor index referenced a position beyond `line.len()`.
    ///
    /// W3 (roadmap correctness 2026-05-24): `precursor_index` is the
    /// raw decoded `u64` (carries the value losslessly on 32-bit
    /// targets where the decoded index can exceed `usize::MAX`).
    PrecursorIndexOutOfBounds {
        precursor_index: u64,
        line_len: usize,
    },
}
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IntegrityKind {
    /// Adjacent `line[i].event_id() >= line[i+1].event_id()`.
    EventIdNotMonotonic { prev: u64, next: u64 },
    /// A `FiberId` appears in both `purged_ids` and `lookup`.
    PurgedIdInLookup(FiberId),
    /// `next_event_id` disagrees with the value derived from the line tail.
    NextEventIdMismatch {
        actual: u64,
        expected: u64,
        line_len: usize,
    },
    /// A live fiber's `current` index exceeds `line.len()`.
    FiberCurrentOutOfBounds {
        fiber_id: FiberId,
        current: usize,
        line_len: usize,
    },
    /// `line[position].event_id().value() != position`.
    ///
    /// Pre-M1, `verify_invariants` only checked strict monotonicity, so
    /// gapped (`[0, 2]`) or shifted (`[5, 6, 7]`) event-id sequences were
    /// accepted on raw replay. Per-fiber `event_id` is by construction
    /// dragline-global and contiguous from zero — replay state that
    /// violates that has been tampered or truncated upstream and must not
    /// rehydrate into a live `Dragline`.
    EventIdPositionMismatch { event_id: u64, position: u64 },
}
impl core::fmt::Display for FiberInvariantKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::FiberLen(FiberLenReason::Zero) => write!(f, "len must be >= 1"),
            Self::FiberLen(FiberLenReason::Overflow) => write!(f, "fiber len overflow"),
            Self::IndexOrdering(IndexOrderingKind::CurrentBelowAnchor { anchor, current }) => {
                write!(
                    f,
                    "current must be >= anchor (anchor={anchor:?}, current={current:?})"
                )
            }
            Self::IndexOrdering(IndexOrderingKind::NewCurrentNotAfterCurrent {
                current,
                new_current,
            }) => {
                write!(
                    f,
                    "new_current must be > current (current={current:?}, new_current={new_current:?})"
                )
            }
            Self::LinevecAppend(LinevecAppendKind::EventIdMismatch { actual, expected }) => {
                write!(
                    f,
                    "append_validated: event.event_id() {actual} != expected {expected}"
                )
            }
            Self::LinevecAppend(LinevecAppendKind::EventIdNotMonotonic {
                event_id,
                last_event_id,
            }) => {
                write!(
                    f,
                    "append_validated: event_id {event_id} not strictly greater than last {last_event_id}"
                )
            }
            Self::LinevecAppend(LinevecAppendKind::PrecursorIndexOutOfBounds {
                precursor_index,
                line_len,
            }) => {
                write!(
                    f,
                    "append_validated: precursor index {precursor_index} not strictly less than line.len() {line_len}"
                )
            }
            Self::Integrity(IntegrityKind::EventIdNotMonotonic { prev, next }) => {
                write!(
                    f,
                    "event_id not monotonic: line[i]={prev} >= line[i+1]={next}"
                )
            }
            Self::Integrity(IntegrityKind::PurgedIdInLookup(fiber_id)) => {
                write!(
                    f,
                    "fiber_id {fiber_id:?} is both purged and present in lookup"
                )
            }
            Self::Integrity(IntegrityKind::NextEventIdMismatch {
                actual,
                expected,
                line_len,
            }) => {
                write!(
                    f,
                    "next_event_id {actual} does not match expected {expected} (line.len()={line_len})"
                )
            }
            Self::Integrity(IntegrityKind::FiberCurrentOutOfBounds {
                fiber_id,
                current,
                line_len,
            }) => {
                write!(
                    f,
                    "fiber {fiber_id:?} current index {current} >= line.len() {line_len}"
                )
            }
            Self::Integrity(IntegrityKind::EventIdPositionMismatch { event_id, position }) => {
                write!(
                    f,
                    "event_id {event_id} at line position {position} (expected event_id == position for contiguous replay)"
                )
            }
            Self::CounterSaturated { counter } => {
                write!(f, "{counter} is u64::MAX — next value cannot be derived")
            }
        }
    }
}
impl core::error::Error for FiberInvariantKind {}
/// M4 (roadmap correctness 4): typed reasons `Dragline::from_raw_parts`
/// rejects parts that disagree with canonical state derivable from
/// `line`.
///
/// `from_raw_parts` derives canonical
/// `(lookup, next_id, next_event_id)` from the event line itself and
/// compares against the supplied values; any disagreement surfaces as
/// one of these variants. Also covers the two reconstructed-from-line
/// states that cannot persist via the event line (`migrating == true`
/// and non-empty `purged_ids`), mirroring `persist::UnpersistableKind`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FromRawPartsKind {
    /// `migrating == true` cannot be reconstructed from an event line
    /// (the migration flag is dragline-runtime state).
    #[error("supplied `migrating == true` cannot be reconstructed from event line")]
    Migrating,
    /// Non-empty `purged_ids` cannot be reconstructed from an event line.
    #[error("supplied `purged_ids` is non-empty; cannot be reconstructed from event line")]
    PurgedIdsNonEmpty,
    /// Canonical line implies a `FiberId` in `lookup` that the caller
    /// did not supply.
    #[error("lookup is missing canonical entry for fiber {fiber_id:?}")]
    LookupMissingFiber { fiber_id: FiberId },
    /// Caller supplied a `lookup` entry for a `FiberId` that does not
    /// appear in the event line.
    #[error("lookup has extra entry for fiber {fiber_id:?} not present in line")]
    LookupExtraFiber { fiber_id: FiberId },
    /// Supplied `FiberState` disagrees with the canonical state derived
    /// from the latest event's `detached` flag for that fiber.
    #[error(
        "fiber {fiber_id:?} state mismatch: supplied {supplied:?}, expected (from line) {expected:?}"
    )]
    FiberStateMismatch {
        fiber_id: FiberId,
        supplied: FiberState,
        expected: FiberState,
    },
    /// Supplied `Fiber::current` disagrees with the canonical index
    /// (the position of the latest event for that fiber).
    #[error("fiber {fiber_id:?} `current` mismatch: supplied {supplied:?}, expected {expected:?}")]
    FiberCurrentMismatch {
        fiber_id: FiberId,
        supplied: Index,
        expected: Index,
    },
    /// Supplied `next_id` (`FiberId` allocator) disagrees with
    /// `max(fiber_id in line) + 1` (or `0` for an empty line).
    #[error("next_id mismatch: supplied {supplied:?}, expected {expected:?}")]
    NextIdMismatch {
        supplied: FiberId,
        expected: FiberId,
    },
    /// Supplied `next_event_id` disagrees with `line.len()` (which
    /// equals `last.event_id() + 1` once contiguity holds).
    #[error("next_event_id mismatch: supplied {supplied:?}, expected {expected:?}")]
    NextEventIdMismatch {
        supplied: crate::event::EventId,
        expected: crate::event::EventId,
    },
}
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PardosaError {
    #[error("invalid transition: state {state:?} + action {action:?}")]
    InvalidTransition {
        state: FiberState,
        action: FiberAction,
    },
    /// `Dragline::rescue` was invoked with a `LockedRescuePolicy` /
    /// [`FiberState`] pair the substrate refuses to honour.
    ///
    /// Two arms surface here:
    ///
    /// * `LockedRescuePolicy::AcceptDataLoss` — rejected for every
    ///   state (no public data-loss rescue path; use
    ///   [`crate::migrate::migrate_keep`] instead).
    /// * `LockedRescuePolicy::PreserveAuditTrail` on
    ///   [`FiberState::Locked`] — rejected because resuming a locked
    ///   entry would sever the precursor chain.
    ///
    /// `policy` and `state` capture the rejected pair verbatim.
    #[error("locked-rescue policy {policy:?} not supported for fiber state {state:?}")]
    RescuePolicyUnsupported {
        policy: LockedRescuePolicy,
        state: FiberState,
    },
    #[error("fiber invariant violation: {0}")]
    FiberInvariant(FiberInvariantKind),
    #[error("fiber {0} is not in Purged state — cannot reuse")]
    IdNotPurged(FiberId),
    #[error("fiber {0} already exists")]
    IdAlreadyExists(FiberId),
    #[error("fiber not found for fiber id {0}")]
    FiberNotFound(FiberId),
    #[error("index overflow")]
    IndexOverflow,
    #[error("fiber id counter overflow")]
    FiberIdOverflow,
    #[error("event ID counter overflow")]
    EventIdOverflow,
    #[error("migration in progress — application operations rejected")]
    MigrationInProgress,
    #[error(
        "precursor chain broken at event_id {event_id}: precursor index {precursor:?} not found"
    )]
    BrokenPrecursorChain { event_id: u64, precursor: Index },
    #[error(
        "precursor hash mismatch at event_id {event_id}: expected {expected:?}, actual {actual:?}"
    )]
    PrecursorHashMismatch {
        event_id: u64,
        expected: [u8; 32],
        actual: [u8; 32],
    },
    /// `Dragline::from_raw_parts` was given a `frontier` value that
    /// disagrees with the value the runtime re-folds from `line`.
    /// The public constructor is verify-only (ADR-0004 §3); this
    /// gate fires only for in-process callers supplying an
    /// inconsistent `(line, frontier)` pair, not for open-time
    /// `.pgno` tamper detection (which requires an out-of-band
    /// trusted anchor per ADR-0004 § Security model).
    #[error("frontier mismatch in from_raw_parts: supplied {supplied:?} != computed {computed:?}")]
    FrontierMismatch {
        supplied: Frontier,
        computed: Frontier,
    },
    /// M4 (roadmap correctness 4): `Dragline::from_raw_parts` rejected
    /// supplied parts because they disagree with the canonical state
    /// derived from `line`. See `FromRawPartsKind` for the typed
    /// reason taxonomy.
    #[error("from_raw_parts rejected supplied state: {0}")]
    FromRawParts(FromRawPartsKind),
    /// The publish-anchor buffer reached its configured cap before
    /// `Dragline::sync_data_with_source` could drain it. Typed configuration
    /// error per ADR-0007.
    ///
    /// # No-op-on-`Err` contract
    ///
    /// The originating `commit_event` made no observable change to
    /// the in-memory event line: no append, no frontier
    /// roll, no `event_id` advance, no anchor loss. Drain via
    /// `Dragline::sync_data_with_source` and retry. Adopters raise the cap or
    /// sync more frequently.
    #[error("publish-anchor buffer overflow: cap {cap} reached before next sync_data")]
    AnchorBufferOverflow { cap: usize },
    /// A `pardosa::cursor::JournalCursor` failed to read or decode the
    /// underlying `.pgno` while opening or iterating. Wraps the
    /// typed `persist::Error` (file framing, schema-hash mismatch,
    /// decode failure, I/O) without duplicating its taxonomy.
    ///
    /// This is the only `PardosaError` variant a cursor can surface
    /// from reads of a persistent source; ADR-0011 D1 documents the
    /// choice of wrapping over an associated `type Error` GAT on the
    /// `Cursor` trait (chosen so the trait's `Iter::Item` stays
    /// uniformly `Result<Event<T>, PardosaError>` per the brief's
    /// locked decision).
    #[error("cursor read failed: {source}")]
    CursorRead {
        #[source]
        source: Box<crate::persist::Error>,
    },
    /// Backend detected a stale single-writer append conflict.
    #[error("concurrency conflict: {source}")]
    ConcurrencyConflict {
        #[source]
        source: Box<dyn core::error::Error + Send + Sync + 'static>,
    },
    /// `JournalCursor` sidecar I/O failed (read at `from_path`, or
    /// write/fsync during `commit_offset`).
    ///
    /// Scope is sidecar-only: the `.cursor` watermark file. Dragline-runtime
    /// `.pgno` open failures surface as
    /// [`PardosaError::CursorJournalOpen`]. Format is 8 LE bytes
    /// (`EventId`); other lengths surface here with
    /// `ErrorKind::InvalidData`. Recovery: delete the sidecar —
    /// cursor restarts from the journal beginning. ADR-0011 D5
    /// documents the durability tradeoff.
    #[error("cursor sidecar I/O failed: {source}")]
    CursorSidecar {
        #[source]
        source: Box<std::io::Error>,
    },
    /// `JournalCursor::from_path` could not open the underlying
    /// `.pgno` journal file (`NotFound`, permission denied, etc).
    ///
    /// Scope is journal-only; distinct from
    /// [`PardosaError::CursorSidecar`] (the `.cursor` watermark
    /// file) and [`PardosaError::CursorRead`] (post-open framing /
    /// decode). Recovery: verify path and permissions; do not
    /// delete the sidecar — it is a healthy watermark for a
    /// currently unreachable journal.
    #[error("cursor journal open failed: {source}")]
    CursorJournalOpen {
        #[source]
        source: Box<std::io::Error>,
    },
    /// `JournalCursor::tail` was called after a prior iterator
    /// surfaced a deferred construction error from `persist::stream`,
    /// which consumed the underlying reader. The cursor cannot retry
    /// against the same source; the caller must drop the cursor and
    /// reopen against a fresh reader (e.g. `from_path` again).
    /// ADR-0011 D6 documents the choice of a typed-error retry path
    /// over a panic.
    #[error("cursor reader was consumed by a prior failed tail; reopen the cursor")]
    CursorExhausted,
    /// A `Dragline` publish-watermark sidecar file operation failed
    /// (read at open, or write/fsync during the post-publish update).
    ///
    /// Scope is **sidecar-only**: the `<journal>.publish` 8-LE-byte
    /// watermark file owned by the writer (ADR-0016 §D5). A file
    /// present on disk that does not satisfy the fixed 8-byte length
    /// surfaces here with `ErrorKind::InvalidData`. Recovery: delete
    /// the sidecar — the watermark will reconstruct from `None` on
    /// next start and the writer will republish the full anchor
    /// superset (idempotent per ADR-0016 §D7).
    #[error("publish watermark sidecar I/O failed: {source}")]
    PublishWatermark {
        #[source]
        source: Box<std::io::Error>,
    },
    /// Conversion of an [`Index`] value to `usize` for slice
    /// indexing failed because the inner `u64` exceeds
    /// `usize::MAX`. Structurally unreachable on the only
    /// supported target (`lib.rs` gates a 64-bit
    /// `target_pointer_width`); the variant exists so substrate
    /// indexing sites stay panic-free via `?` propagation rather
    /// than `expect`, and so a 32-bit cross-compile would surface
    /// the failure as a typed error rather than a panic.
    #[error("Index value {0} does not fit in usize on this target")]
    IndexNotUsize(u64),
}
impl From<IndexTooLargeForUsize> for PardosaError {
    fn from(e: IndexTooLargeForUsize) -> Self {
        PardosaError::IndexNotUsize(e.0)
    }
}
#[cfg(test)]
mod pardosa_error_tests {
    use super::*;

    #[test]
    fn concurrency_conflict_carries_source_chain() {
        let inner: Box<dyn core::error::Error + Send + Sync + 'static> =
            Box::new(std::io::Error::other("backend conflict"));
        let err = PardosaError::ConcurrencyConflict { source: inner };
        let src = core::error::Error::source(&err).expect("source attached");
        assert!(src.to_string().contains("backend conflict"));
    }
}
/// Operation tag carried by [`BackendError::Timeout`] (ADR-0022
/// §D7). Names the backend operation whose per-operation timeout
/// fired. Adopters discriminate timeout-on-append from
/// timeout-on-sync from timeout-on-cursor-advance from
/// timeout-on-open for retry / alerting logic.
///
/// `#[non_exhaustive]`: ADR-0007 default. Backends may surface
/// additional operations in later phases without breaking
/// adopter pattern-matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BackendOp {
    /// `BackendSink::append` — single-event append (ADR-0022 §D2).
    Append,
    /// `BackendSink::sync` — durability fence (ADR-0022 §D2).
    Sync,
    /// Cursor advance (`next` on the substrate cursor iterator,
    /// ADR-0022 §D7's third per-op-timeout call site).
    CursorNext,
    /// Open-gate read: stream-info / marker fetch
    /// (`read_stream_description`) and replay-based rehydrate
    /// fetch (`replay_all` / `fetch_durable_bytes`) — reads at
    /// connect/rehydrate time, distinct from the append-time
    /// [`Self::Sync`] durability fence.
    Open,
}
impl core::fmt::Display for BackendOp {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Append => f.write_str("append"),
            Self::Sync => f.write_str("sync"),
            Self::CursorNext => f.write_str("cursor next"),
            Self::Open => f.write_str("open"),
        }
    }
}
/// Non-recoverable backend runtime failure kind carried by
/// [`BackendError::RuntimeFailure`] (ADR-0022 §D7).
///
/// Backend-internal runtime errors (worker panic, runtime shutdown
/// mid-operation) surface here. Distinct from
/// [`BackendError::Timeout`] (per-op deadline exceeded, retryable)
/// and [`BackendError::Publish`] (transient publisher failure,
/// recoverable per ADR-0015).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeFailureKind {
    /// The backend's internal worker / blocking-bridge thread
    /// panicked. The backend instance is poisoned; recovery
    /// requires a fresh constructor call.
    WorkerPanic,
    /// The backend's internal async runtime shut down
    /// mid-operation. The backend instance is no longer usable.
    RuntimeShutdown,
}
impl core::fmt::Display for RuntimeFailureKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WorkerPanic => f.write_str("worker panic"),
            Self::RuntimeShutdown => f.write_str("runtime shutdown mid-operation"),
        }
    }
}
/// Bounded-buffer overflow kind carried by
/// [`BackendError::PublisherBacklog`] (ADR-0022 §D8).
///
/// A backend providing its own publish behaviour (collapsed
/// `JetStream` case — the authoritative log IS the downstream
/// transport, per ADR-0022 §D8) MUST document its bounded-buffer
/// shape and surface overflow as this variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PublisherBacklogKind {
    /// The backend's bounded publish buffer reached its configured
    /// cap before the next sync drained it. Adopter raises the
    /// cap or syncs more frequently.
    CapExceeded,
}
impl core::fmt::Display for PublisherBacklogKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::CapExceeded => f.write_str("bounded buffer cap exceeded"),
        }
    }
}
/// Backend-substrate failure taxonomy (ADR-0022 §D11).
///
/// Returned by `BackendSink::append` / `sync`. Variants at
/// acceptance:
///
/// - [`BackendError::Timeout`] (§D7) — per-op deadline exceeded.
/// - [`BackendError::RuntimeFailure`] (§D7) — non-recoverable.
/// - [`BackendError::Publish`] (§D7) — transient downstream
///   failure; recovery via ADR-0015 rebuffer.
/// - [`BackendError::Connect`] — backend connection failure.
/// - [`BackendError::Replay`] — backend replay failure.
/// - [`BackendError::PublisherBacklog`] (§D8) — bounded buffer
///   cap exceeded.
///
/// `#[non_exhaustive]` per ADR-0007.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BackendError {
    /// A per-operation timeout fired before the backend completed
    /// the named operation. ADR-0022 §D7 binds the public timeout
    /// surface to `append`, `sync`, and cursor `next`.
    #[error("backend operation `{op}` timed out: elapsed {elapsed:?} > configured {configured:?}")]
    Timeout {
        op: BackendOp,
        elapsed: std::time::Duration,
        configured: std::time::Duration,
    },
    /// A non-recoverable internal runtime failure. The backend
    /// instance is poisoned; callers must reconstruct.
    #[error("backend runtime failure: {kind}")]
    RuntimeFailure { kind: RuntimeFailureKind },
    /// A transient downstream publisher failure. Recovery is the
    /// publisher trait's rebuffer policy (ADR-0015 §D3); the
    /// adopter does not retry directly.
    #[error("backend publish failure during `{op}`: {source}")]
    Publish {
        op: BackendOp,
        #[source]
        source: Box<dyn core::error::Error + Send + Sync + 'static>,
    },
    /// Backend detected a stale single-writer append conflict.
    #[error("backend concurrency conflict: {source}")]
    ConcurrencyConflict {
        #[source]
        source: Box<dyn core::error::Error + Send + Sync + 'static>,
    },
    /// A downstream backend connection failure.
    #[error("backend connect failure during `{op}`: {source}")]
    Connect {
        op: BackendOp,
        #[source]
        source: Box<dyn core::error::Error + Send + Sync + 'static>,
    },
    /// A downstream backend replay failure.
    #[error("backend replay failure during `{op}`: {source}")]
    Replay {
        op: BackendOp,
        #[source]
        source: Box<dyn core::error::Error + Send + Sync + 'static>,
    },
    /// Bounded publish buffer reached its configured cap in the
    /// collapsed-publisher case (ADR-0022 §D8). The backend's own
    /// bounded-buffer documentation describes the cap and drain
    /// semantics.
    #[error("backend publisher backlog: {kind}")]
    PublisherBacklog { kind: PublisherBacklogKind },
}
#[cfg(test)]
mod backend_error_tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn timeout_display_names_op_and_durations() {
        let err = BackendError::Timeout {
            op: BackendOp::Append,
            elapsed: Duration::from_millis(750),
            configured: Duration::from_millis(500),
        };
        let rendered = err.to_string();
        assert!(rendered.contains("append"), "render: {rendered}");
        assert!(rendered.contains("750"), "render: {rendered}");
        assert!(rendered.contains("500"), "render: {rendered}");
    }
    #[test]
    fn runtime_failure_display_names_kind() {
        let err = BackendError::RuntimeFailure {
            kind: RuntimeFailureKind::WorkerPanic,
        };
        assert!(err.to_string().contains("worker panic"));
    }
    #[test]
    fn publish_carries_source_chain() {
        let inner: Box<dyn core::error::Error + Send + Sync + 'static> =
            Box::new(std::io::Error::other("downstream link reset"));
        let err = BackendError::Publish {
            op: BackendOp::Append,
            source: inner,
        };
        let src = core::error::Error::source(&err).expect("source attached");
        assert!(src.to_string().contains("downstream link reset"));
    }
    #[test]
    fn concurrency_conflict_carries_source_chain() {
        let inner: Box<dyn core::error::Error + Send + Sync + 'static> =
            Box::new(std::io::Error::other("wrong last sequence"));
        let err = BackendError::ConcurrencyConflict { source: inner };
        let src = core::error::Error::source(&err).expect("source attached");
        assert!(src.to_string().contains("wrong last sequence"));
    }
    #[test]
    fn publisher_backlog_display_names_kind() {
        let err = BackendError::PublisherBacklog {
            kind: PublisherBacklogKind::CapExceeded,
        };
        assert!(err.to_string().contains("cap exceeded"));
    }
    #[test]
    fn backend_op_display_round_trips_each_variant() {
        assert_eq!(BackendOp::Append.to_string(), "append");
        assert_eq!(BackendOp::Sync.to_string(), "sync");
        assert_eq!(BackendOp::CursorNext.to_string(), "cursor next");
        assert_eq!(BackendOp::Open.to_string(), "open");
    }
}
/// Transport-level failure surfaced by a [`FrontierPublisher`].
///
/// Adopter-facing typed error per ADR-0007. Inspected by
/// `Dragline::sync_data_with_source` for success-vs-failure only (ADR-0015 D2).
/// On failure, the offending anchor plus all later-in-order anchors
/// re-buffer for retry on the next `sync_data` (D3). `sync_data` is
/// never poisoned by a `PublishError`: durability commits before
/// publish, and failures do not propagate (D4). Panicking publishers
/// are substrate-contract violations; return `Err` instead.
///
/// Wrap transport-specific causes in `Custom`; the runtime does not
/// introspect the boxed cause.
///
/// [`FrontierPublisher`]: crate::frontier::FrontierPublisher
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PublishError {
    /// The underlying transport reported a delivery failure (network
    /// error, broker rejection, timeout). Retry is implicit via
    /// `Dragline::sync_data_with_source` re-buffer (ADR-0015 D3).
    #[error("publisher transport failure")]
    Transport,
    /// The publisher has been closed (graceful shutdown or remote-side
    /// closure). Subsequent `publish` calls will continue to fail
    /// until the publisher is replaced.
    #[error("publisher closed")]
    Closed,
    /// Adopter-defined cause carried as a boxed `std::error::Error`.
    /// The runtime does not inspect the inner error; it exists so
    /// adopter implementations need not map every transport variant
    /// to one of the above.
    #[error("publisher error: {source}")]
    Custom {
        #[source]
        source: Box<dyn core::error::Error + Send + Sync + 'static>,
    },
}
