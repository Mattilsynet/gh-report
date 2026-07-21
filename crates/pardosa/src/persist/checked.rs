use super::error::{CheckedReplayKind, Error};
use super::rehydrate::PrecursorCheckMode;
use crate::frontier::Frontier;
use crate::{Event, EventId, FiberId};
use pardosa_file::Reader;
use pardosa_schema::GenomeSafe;
use pardosa_wire::{Decode, from_bytes};
use std::io::{Read, Seek};
use std::iter::FusedIterator;
use std::marker::PhantomData;
/// Pure contiguity predicate shared by the streaming verify chain
/// and the dragline rebuild path.
pub(crate) fn event_id_matches_position(event_id: u64, position: u64) -> bool {
    event_id == position
}
/// Pure precursor-bounds check shared by the streaming verify chain
/// and the batch rebuild path.
pub(crate) fn precursor_bounds<T>(
    event: &Event<T>,
    position: usize,
    position_u64: u64,
) -> Result<Option<usize>, CheckedReplayKind> {
    let Some(precursor_idx) = event.precursor().as_index() else {
        return Ok(None);
    };
    match usize::try_from(precursor_idx) {
        Ok(pidx) if pidx < position => Ok(Some(pidx)),
        Ok(_) => Err(CheckedReplayKind::PrecursorOutOfBounds {
            event_id: event.event_id().value(),
            position: position_u64,
            precursor_index: precursor_idx.value(),
        }),
        Err(e) => Err(CheckedReplayKind::PrecursorOutOfBounds {
            event_id: event.event_id().value(),
            position: position_u64,
            precursor_index: e.0,
        }),
    }
}
/// Pure same-fiber + precursor-hash check shared by the streaming
/// verify chain and the batch rebuild path.
pub(crate) fn precursor_matches<T>(
    event: &Event<T>,
    precursor_index: u64,
    prior_fiber: FiberId,
    prior_hash: [u8; 32],
) -> Result<(), CheckedReplayKind> {
    if prior_fiber != event.fiber_id() {
        return Err(CheckedReplayKind::PrecursorFiberMismatch {
            event_id: event.event_id().value(),
            precursor_index,
            expected_fiber: event.fiber_id(),
            actual_fiber: prior_fiber,
        });
    }
    if event.precursor_hash() != prior_hash {
        return Err(CheckedReplayKind::PrecursorHashMismatch {
            event_id: event.event_id().value(),
            precursor_index,
            expected: prior_hash,
            actual: event.precursor_hash(),
        });
    }
    Ok(())
}
/// Emit the non-blocking `precursor_check_would_fail` warn. Never
/// logs the domain payload.
pub(crate) fn warn_precursor_would_fail<T>(
    event: &Event<T>,
    kind: &CheckedReplayKind,
    position: usize,
) {
    let check_kind = check_kind_name(kind);
    let position_u64 = u64::try_from(position).unwrap_or(u64::MAX);
    tracing::warn!(
        check_kind,
        fiber_id = event.fiber_id().value(),
        event_id = event.event_id().value(),
        position = position_u64,
        "precursor_check_would_fail"
    );
}
fn check_kind_name(kind: &CheckedReplayKind) -> &'static str {
    match kind {
        CheckedReplayKind::EventIdPositionMismatch { .. } => "EventIdPositionMismatch",
        CheckedReplayKind::PrecursorOutOfBounds { .. } => "PrecursorOutOfBounds",
        CheckedReplayKind::PrecursorFiberMismatch { .. } => "PrecursorFiberMismatch",
        CheckedReplayKind::PrecursorHashMismatch { .. } => "PrecursorHashMismatch",
    }
}
/// Checked-replay event stream (M3).
///
/// Returned by [`stream_checked`]. Walks every message — including
/// the `resume_after` prefix — and validates four invariants per
/// event: contiguity (`event_id == position`), precursor bounds,
/// same-fiber precursor, precursor hash. On violation the iterator
/// yields `Err(Error::CheckedReplay { kind })`. Exclusive resume
/// (ADR-0011 D2/D4).
///
/// # Memory shape (mvft-d)
///
/// No per-position cache; `Precursor::Of(pidx)` triggers a
/// `Reader::read_message(pidx)`. See bench `checked_replay_100`.
#[derive(Debug)]
pub struct CheckedEventStream<R: Read + Seek, T> {
    pub(super) reader: Reader<R>,
    next: usize,
    skip_until: Option<EventId>,
    /// Sticky terminal error: once a check fails the stream is
    /// poisoned and every subsequent `next()` returns `None` (mirrors
    /// `EventStream` behaviour on a deferred decode error — the
    /// iterator is fused).
    poisoned: bool,
    /// Rolling BLAKE3 frontier folded over raw persisted bytes in
    /// line order (ADR-0004 §1). Advanced once per event the reader
    /// returns successfully (before per-event structural checks).
    /// Surfaced to the rehydrate path via [`Self::frontier`] so the
    /// reader bound stays `T: Decode + GenomeSafe` (ADR-0020) —
    /// re-encoding decoded events to roll the frontier would
    /// reintroduce a writer-side `Encode` bound.
    frontier: Frontier,
    /// Precursor-check enforcement mode (adr-fmt-lutpd finding #2 /
    /// adr-fmt-ibi23). Governs only the three precursor checks
    /// (bounds, same-fiber, hash) inside [`Self::verify_precursor`];
    /// contiguity (`check_position`) stays unconditional in both
    /// modes, matching the dragline rebuild path.
    mode: PrecursorCheckMode,
    _t: PhantomData<fn() -> T>,
}
impl<R: Read + Seek, T> Iterator for CheckedEventStream<R, T>
where
    T: Decode + GenomeSafe,
{
    type Item = Result<Event<T>, Error>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.poisoned {
            return None;
        }
        loop {
            if self.next >= self.reader.index().len() {
                return None;
            }
            let position = self.next;
            self.next += 1;
            let bytes = match self.read_and_roll(position) {
                Ok(b) => b,
                Err(e) => return self.fail(e),
            };
            let event: Event<T> = match from_bytes(&bytes) {
                Ok(e) => e,
                Err(e) => return self.fail(Error::Decode(e)),
            };
            let position_u64 = u64::try_from(position).expect("64-bit target enforced at lib root");
            if let Err(e) = check_position(&event, position_u64) {
                return self.fail(e);
            }
            if let Err(e) = self.verify_precursor(&event, position, position_u64) {
                return self.fail(e);
            }
            if let Some(threshold) = self.skip_until
                && event.event_id().value() <= threshold.value()
            {
                continue;
            }
            return Some(Ok(event));
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.reader.index().len().saturating_sub(self.next);
        let lower = if self.skip_until.is_some() {
            0
        } else {
            remaining
        };
        (lower, Some(remaining))
    }
}
impl<R: Read + Seek, T> FusedIterator for CheckedEventStream<R, T> where T: Decode + GenomeSafe {}
impl<R: Read + Seek, T> CheckedEventStream<R, T>
where
    T: Decode + GenomeSafe,
{
    #[expect(
        clippy::unnecessary_wraps,
        reason = "`fail` always yields `Some(Err(..))` so that `Iterator::next` call sites can `return self.fail(e)` directly; the `Option` wrap matches `next`'s return type and is structural, not removable"
    )]
    fn fail(&mut self, error: Error) -> Option<Result<Event<T>, Error>> {
        self.poisoned = true;
        Some(Err(error))
    }
    fn read_and_roll(&mut self, position: usize) -> Result<Vec<u8>, Error> {
        let bytes = self.reader.read_message(position).map_err(Error::File)?;
        self.frontier = self.frontier.roll(&bytes);
        Ok(bytes)
    }
    fn verify_precursor(
        &mut self,
        event: &Event<T>,
        position: usize,
        position_u64: u64,
    ) -> Result<(), Error> {
        let pidx = match precursor_bounds(event, position, position_u64) {
            Ok(None) => return Ok(()),
            Ok(Some(pidx)) => pidx,
            Err(kind) => return self.handle_precursor_violation(event, kind, position),
        };
        let prior_bytes = self.reader.read_message(pidx).map_err(Error::File)?;
        let prior_event: Event<T> = from_bytes(&prior_bytes).map_err(Error::Decode)?;
        let prior_fid = prior_event.fiber_id();
        let prior_hash = pardosa_wire::precursor_hash_of(&prior_bytes);
        let precursor_index = event
            .precursor()
            .as_index()
            .expect("bounds check above confirmed a precursor index")
            .value();
        if let Err(kind) = precursor_matches(event, precursor_index, prior_fid, prior_hash) {
            return self.handle_precursor_violation(event, kind, position);
        }
        Ok(())
    }
    /// Dispatch a precursor-check violation per [`Self::mode`].
    fn handle_precursor_violation(
        &self,
        event: &Event<T>,
        kind: CheckedReplayKind,
        position: usize,
    ) -> Result<(), Error> {
        match self.mode {
            PrecursorCheckMode::Enforce => Err(Error::CheckedReplay { kind }),
            PrecursorCheckMode::ObserveOnly => {
                warn_precursor_would_fail(event, &kind, position);
                Ok(())
            }
        }
    }
}
fn check_position<T>(event: &Event<T>, position_u64: u64) -> Result<(), Error> {
    if event_id_matches_position(event.event_id().value(), position_u64) {
        Ok(())
    } else {
        Err(Error::CheckedReplay {
            kind: CheckedReplayKind::EventIdPositionMismatch {
                event_id: event.event_id().value(),
                position: position_u64,
            },
        })
    }
}
impl<R: Read + Seek, T> CheckedEventStream<R, T> {
    /// Consume the stream and return the underlying reader.
    /// `JournalCursor::tail` reclaims the reader between calls to
    /// honour the current `acked_offset`.
    pub fn into_inner(self) -> R {
        self.reader.into_inner()
    }
    /// Rolling BLAKE3 frontier folded over raw persisted bytes for
    /// every event the stream has yielded successfully so far
    /// (ADR-0004 §1). [`Frontier::GENESIS`] before the first
    /// successful read. Crate-internal: used by
    /// [`rehydrate_validated`] so the reader-side rebuild path can
    /// install the frontier without re-encoding decoded events
    /// (ADR-0020 reader bound).
    ///
    /// [`rehydrate_validated`]: super::rehydrate_validated
    pub(crate) fn frontier(&self) -> Frontier {
        self.frontier
    }
}
/// Open a `.pgno` source and return a [`CheckedEventStream<R, T>`]
/// (M3).
///
/// Performs container-header validation and exclusive `resume_after`,
/// plus four structural checks per event (see [`CheckedEventStream`]).
/// The prefix skipped by `resume_after` is validated before any tail
/// event is yielded.
///
/// # Errors
///
/// - [`Error::File`] from [`Reader::open`].
/// - [`Error::SchemaHashMismatch`] for header mismatch.
///
/// [`Error::CheckedReplay`] and per-event errors surface as `Err`
/// items on the iterator.
/// Open a `.pgno` source and return a [`CheckedEventStream<R, T>`]
/// (M3), always in [`PrecursorCheckMode::Enforce`] — the existing,
/// unconditional-reject contract used by `cursor.rs` and
/// `stream_validated`. Crate-internal callers that need
/// `ObserveOnly` use [`stream_checked_with_mode`] instead.
///
/// Performs container-header validation and exclusive `resume_after`,
/// plus four structural checks per event (see [`CheckedEventStream`]).
/// The prefix skipped by `resume_after` is validated before any tail
/// event is yielded.
///
/// # Errors
///
/// - [`Error::File`] from [`Reader::open`].
/// - [`Error::SchemaHashMismatch`] for header mismatch.
///
/// [`Error::CheckedReplay`] and per-event errors surface as `Err`
/// items on the iterator.
pub fn stream_checked<R, T>(
    source: R,
    resume_after: Option<EventId>,
) -> Result<CheckedEventStream<R, T>, Error>
where
    R: Read + Seek,
    T: Decode + GenomeSafe,
{
    stream_checked_with_mode(source, resume_after, PrecursorCheckMode::Enforce)
}
/// Mode-aware sibling of [`stream_checked`]. Crate-internal —
/// external callers get [`PrecursorCheckMode::Enforce`] via the
/// public [`stream_checked`].
///
/// # Errors
///
/// Same as [`stream_checked`].
pub(crate) fn stream_checked_with_mode<R, T>(
    source: R,
    resume_after: Option<EventId>,
    mode: PrecursorCheckMode,
) -> Result<CheckedEventStream<R, T>, Error>
where
    R: Read + Seek,
    T: Decode + GenomeSafe,
{
    let reader = Reader::open(source)?;
    let found = reader.schema_hash();
    let expected = Event::<T>::ENVELOPE_HASH;
    if found != expected {
        return Err(Error::SchemaHashMismatch { expected, found });
    }
    Ok(CheckedEventStream {
        reader,
        next: 0,
        skip_until: resume_after,
        poisoned: false,
        frontier: Frontier::GENESIS,
        mode,
        _t: PhantomData,
    })
}
