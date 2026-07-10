use crate::error::FiberInvariantKind;
use crate::event::Index;
use crate::{FiberId, PardosaError};
use pardosa_file::FileError;
use pardosa_wire::DecodeError;
/// Operation-scoped taxonomy of invariant violations reachable from
/// [`rehydrate_unchecked`] (ADR-0014, F2 closure).
///
/// Narrower than [`PardosaError`] so [`Error::InvariantViolation`]
/// cannot transitively wrap [`PardosaError::CursorRead`] — breaking
/// the cycle documented in ADR-0011 D6 at the type level. Variants
/// are a direct projection of [`PardosaError`]; see
/// `From<PardosaError>` for the mapping. `Unexpected` preserves `From`
/// totality and is unreachable on the rebuild path.
///
/// [`rehydrate_unchecked`]: super::rehydrate_unchecked
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RehydrateInvariant {
    #[error("fiber invariant violation: {0}")]
    FiberInvariant(FiberInvariantKind),
    #[error("index overflow during rehydrate")]
    IndexOverflow,
    #[error("fiber id counter overflow during rehydrate")]
    FiberIdOverflow,
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
    /// Unreachable on the rebuild path by construction; preserves
    /// `From<PardosaError>` totality without restoring the cycle.
    /// `error` carries the Debug-formatted offending `PardosaError`
    /// for post-mortem debugging. The field is an owned `String`, not
    /// a wrapped `PardosaError`, so the ADR-0011 D6 cycle stays broken
    /// at the type level.
    #[error("unexpected pardosa error reached rehydrate path: {error}")]
    Unexpected { error: String },
}
impl From<PardosaError> for RehydrateInvariant {
    fn from(err: PardosaError) -> Self {
        match err {
            PardosaError::FiberInvariant(k) => Self::FiberInvariant(k),
            PardosaError::IndexOverflow => Self::IndexOverflow,
            PardosaError::FiberIdOverflow => Self::FiberIdOverflow,
            PardosaError::BrokenPrecursorChain {
                event_id,
                precursor,
            } => Self::BrokenPrecursorChain {
                event_id,
                precursor,
            },
            PardosaError::PrecursorHashMismatch {
                event_id,
                expected,
                actual,
            } => Self::PrecursorHashMismatch {
                event_id,
                expected,
                actual,
            },
            other => {
                debug_assert!(
                    false,
                    "unexpected PardosaError variant on rehydrate path: {other:?}"
                );
                Self::Unexpected {
                    error: format!("{other:?}"),
                }
            }
        }
    }
}
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("schema hash mismatch: expected 0x{expected:032X}, found 0x{found:032X}")]
    SchemaHashMismatch { expected: u128, found: u128 },
    #[error("schema marker absent on populated stream: expected 0x{expected:032X}")]
    SchemaMarkerAbsent { expected: u128 },
    /// PGN-0021 R7/R9: adopter-supplied opaque epoch token disagrees
    /// between the stored marker and the caller-supplied expectation,
    /// raised fail-closed at open before any frame decode, parity
    /// with [`Error::SchemaHashMismatch`]. Fires whenever presence
    /// (`Some` vs `None`) or, when both are `Some`, byte content
    /// disagrees; never fires when both are `None` (PGN-0021 R9).
    #[error("semantic epoch mismatch: expected {expected:?}, found {found:?}")]
    SemanticEpochMismatch {
        expected: Option<Box<[u8]>>,
        found: Option<Box<[u8]>>,
    },
    /// Carries the operation-scoped `RehydrateInvariant` (F2 / ADR-0014):
    /// this variant deliberately does **not** wrap [`PardosaError`], so
    /// `PardosaError::CursorRead → persist::Error::InvariantViolation`
    /// no longer forms a cycle.
    #[error("rehydrate invariant check failed: {0}")]
    InvariantViolation(#[source] RehydrateInvariant),
    #[error("file error: {0}")]
    File(#[from] FileError),
    #[error("payload decode error: {0}")]
    Decode(#[source] DecodeError),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// M2: the `Line` carries state that cannot round-trip
    /// through a `.pgno` event line. Surfaced by `persist_with_source`
    /// before any byte hits the sink.
    ///
    /// Unrepresentable states: `migrating == true` (runtime-only),
    /// non-empty `purged_ids` (bookkeeping; not in event line),
    /// `FiberState::Locked` (no event-line entry; would rehydrate as
    /// `Defined`/`Detached`).
    #[error("unpersistable dragline state: {kind}")]
    UnpersistableState { kind: UnpersistableKind },
    /// M3 (roadmap correctness 3): checked replay rejected an event in
    /// the streamed `.pgno`. Surfaced from [`CheckedEventStream::next`]
    /// (returned by [`stream_checked`]) at the position the violation
    /// was detected — for skipped-prefix violations the error fires
    /// before any tail event is yielded.
    ///
    /// [`CheckedEventStream::next`]: super::CheckedEventStream::next
    /// [`stream_checked`]: super::stream_checked
    #[error("checked replay rejected event: {kind}")]
    CheckedReplay { kind: CheckedReplayKind },
    /// ADR-0016 §D6: writing or fsyncing the `<journal>.publish`
    /// watermark sidecar after a successful anchor publish failed.
    /// Local durability (the `.pgno` fsync) has already committed by
    /// the time this variant can surface — the publish-watermark
    /// state is decoupled from journal durability. Recovery is
    /// delete-the-sidecar and restart; reconstruction from `.pgno`
    /// is idempotent under at-least-once (ADR-0016 §D7).
    #[error("publish watermark sidecar i/o failed: {source}")]
    PublishWatermark {
        #[source]
        source: std::io::Error,
    },
}
impl Error {
    /// Whether this error implies persisted bytes have been tampered
    /// with or corrupted, as opposed to a schema/decode mismatch.
    ///
    /// W8: adopters can escalate corruption-suspicious failures
    /// without re-matching the full taxonomy. Same caveat as
    /// [`FileError::is_tamper_suspicious`]: `xxh64` is not a
    /// tamper-resistant MAC; this surfaces evidence, not proof.
    ///
    /// Returns `true` for [`Error::File`] (forwarded),
    /// [`Error::CheckedReplay`], and
    /// [`Error::InvariantViolation`] with
    /// `RehydrateInvariant::BrokenPrecursorChain` /
    /// `RehydrateInvariant::PrecursorHashMismatch`.
    /// Returns `false` for schema drift, decode, pre-write,
    /// I/O, watermark, and other invariant variants.
    #[must_use]
    pub fn is_tamper_suspicious(&self) -> bool {
        match self {
            Self::File(f) => f.is_tamper_suspicious(),
            Self::CheckedReplay { .. }
            | Self::InvariantViolation(
                RehydrateInvariant::BrokenPrecursorChain { .. }
                | RehydrateInvariant::PrecursorHashMismatch { .. },
            ) => true,
            Self::SchemaHashMismatch { .. }
            | Self::SchemaMarkerAbsent { .. }
            | Self::SemanticEpochMismatch { .. }
            | Self::InvariantViolation(_)
            | Self::Decode(_)
            | Self::Io(_)
            | Self::UnpersistableState { .. }
            | Self::PublishWatermark { .. } => false,
        }
    }
}
/// Reason a `Line` cannot be persisted via `persist_with_source` (M2,
/// roadmap correctness 2). See [`Error::UnpersistableState`] for the
/// full rationale per variant.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UnpersistableKind {
    #[error("dragline is migrating (migration flag is in-memory state, not event-line)")]
    Migrating,
    #[error("dragline has non-empty purged_ids set (set is in-memory state, not event-line)")]
    PurgedIdsNonEmpty,
    #[error(
        "lookup entry for fiber {fiber_id:?} is in FiberState::Locked (Locked has no event-line representation)"
    )]
    LockedLookupEntry { fiber_id: FiberId },
}
/// Reason a checked replay rejected an event (M3, roadmap correctness 3).
/// See [`Error::CheckedReplay`] for the surface variant.
///
/// All four checks mirror the corresponding `Line::verify_invariants`
/// / `Line::verify_precursor_chains` checks but run on a
/// **streaming** raw event line — no `Line` materialization, no
/// `Frontier` rebuild (per the M3 brief constraints).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CheckedReplayKind {
    /// Per-event `event_id` must equal the event's physical line
    /// position (M1 contiguity, applied here at stream time).
    #[error("event_id {event_id} != physical position {position} (expected event_id == position)")]
    EventIdPositionMismatch { event_id: u64, position: u64 },
    /// `Precursor::Of(idx)` must reference a strictly earlier line
    /// position than the event itself.
    #[error(
        "event {event_id} at position {position} references precursor index {precursor_index} which is out of bounds"
    )]
    PrecursorOutOfBounds {
        event_id: u64,
        position: u64,
        precursor_index: u64,
    },
    /// `Precursor::Of(idx)` must reference an event that belongs to
    /// the same `FiberId` as the referencing event.
    #[error(
        "event {event_id} precursor index {precursor_index} belongs to fiber {actual_fiber:?}, but referencing event belongs to {expected_fiber:?}"
    )]
    PrecursorFiberMismatch {
        event_id: u64,
        precursor_index: u64,
        expected_fiber: FiberId,
        actual_fiber: FiberId,
    },
    /// `event.precursor_hash()` must equal `precursor_hash_of(canonical
    /// bytes of the referenced prior event)`.
    #[error(
        "event {event_id} precursor hash mismatch (precursor index {precursor_index}): expected {expected:?}, actual {actual:?}"
    )]
    PrecursorHashMismatch {
        event_id: u64,
        precursor_index: u64,
        expected: [u8; 32],
        actual: [u8; 32],
    },
}
/// Validated-replay error envelope (o1ix.6, roadmap correctness 6).
///
/// Generic over the payload validator's error type `E = <T as
/// pardosa_wire::Validate>::Error`, so adopters surface their own
/// `DomainError` (or equivalent) end-to-end without round-tripping
/// through a stringified payload error.
///
/// Variants compose:
///   * [`ValidatedReplayError::Replay`] — any error already surfaced by
///     [`stream_checked`] (file/decode/schema-hash/checked-replay).
///   * [`ValidatedReplayError::Envelope`] — per-envelope structural
///     shape failure ([`crate::event::EnvelopeError`], o1ix.5/.15).
///   * [`ValidatedReplayError::Payload`] — the decoded `T` failed its
///     own `Validate::validate()` check (any open `Validate` impl).
///
/// [`stream_checked`]: super::stream_checked
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ValidatedReplayError<E> {
    #[error(transparent)]
    Replay(#[from] Error),
    #[error("envelope shape violation: {0}")]
    Envelope(#[source] crate::event::EnvelopeError),
    #[error("payload validation failed: {0}")]
    Payload(#[source] E),
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn semantic_epoch_mismatch_display_carries_both_fields() {
        let err = Error::SemanticEpochMismatch {
            expected: Some(Box::from(*b"16.0")),
            found: Some(Box::from(*b"15.0")),
        };
        let rendered = err.to_string();
        assert!(rendered.contains("semantic epoch mismatch"));
        assert!(rendered.contains("[49, 54, 46, 48]"));
        assert!(rendered.contains("[49, 53, 46, 48]"));
        let debugged = format!("{err:?}");
        assert!(debugged.contains("SemanticEpochMismatch"));
    }
    #[test]
    fn semantic_epoch_mismatch_none_vs_some_differ() {
        let none_err = Error::SemanticEpochMismatch {
            expected: None,
            found: None,
        };
        let some_err = Error::SemanticEpochMismatch {
            expected: None,
            found: Some(Box::from(*b"")),
        };
        assert_ne!(none_err.to_string(), some_err.to_string());
    }
}
