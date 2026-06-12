use super::error::{CheckedReplayKind, Error};
use crate::frontier::Frontier;
use crate::{Event, EventId};
use pardosa_file::Reader;
use pardosa_schema::GenomeSafe;
use pardosa_wire::{Decode, from_bytes};
use std::io::{Read, Seek};
use std::iter::FusedIterator;
use std::marker::PhantomData;
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
        let Some(precursor_idx) = event.precursor().as_index() else {
            return Ok(());
        };
        let pidx = usize::try_from(precursor_idx).map_err(|e| Error::CheckedReplay {
            kind: CheckedReplayKind::PrecursorOutOfBounds {
                event_id: event.event_id().value(),
                position: position_u64,
                precursor_index: e.0,
            },
        })?;
        if pidx >= position {
            return Err(Error::CheckedReplay {
                kind: CheckedReplayKind::PrecursorOutOfBounds {
                    event_id: event.event_id().value(),
                    position: position_u64,
                    precursor_index: precursor_idx.value(),
                },
            });
        }
        let prior_bytes = self.reader.read_message(pidx).map_err(Error::File)?;
        let prior_event: Event<T> = from_bytes(&prior_bytes).map_err(Error::Decode)?;
        let prior_fid = prior_event.fiber_id();
        let prior_hash = pardosa_wire::precursor_hash_of(&prior_bytes);
        if prior_fid != event.fiber_id() {
            return Err(Error::CheckedReplay {
                kind: CheckedReplayKind::PrecursorFiberMismatch {
                    event_id: event.event_id().value(),
                    precursor_index: precursor_idx.value(),
                    expected_fiber: event.fiber_id(),
                    actual_fiber: prior_fid,
                },
            });
        }
        if event.precursor_hash() != prior_hash {
            return Err(Error::CheckedReplay {
                kind: CheckedReplayKind::PrecursorHashMismatch {
                    event_id: event.event_id().value(),
                    precursor_index: precursor_idx.value(),
                    expected: prior_hash,
                    actual: event.precursor_hash(),
                },
            });
        }
        Ok(())
    }
}
fn check_position<T>(event: &Event<T>, position_u64: u64) -> Result<(), Error> {
    if event.event_id().value() == position_u64 {
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
pub fn stream_checked<R, T>(
    source: R,
    resume_after: Option<EventId>,
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
        _t: PhantomData,
    })
}
