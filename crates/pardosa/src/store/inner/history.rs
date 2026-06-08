use super::{Event, EventId, FiberId, FiberState, Index, PardosaError, Syncable};
use crate::dragline::Dragline;
use std::io::Seek;
/// Per-fiber history view returned by [`super::StoreReader::fiber`]
/// (ADR-0018 §D3 (a)).
///
/// Walks the fiber's precursor chain within the in-memory dragline.
/// `FiberId` is dragline-local; no sidecar, no I/O.
///
/// Adopter code names this as `FiberHistory<'_, T>` — the sink
/// parameter defaults to [`std::fs::File`] (ADR-0018 § Naming).
pub struct FiberHistory<'a, T, W: Syncable + Seek = std::fs::File> {
    pub(super) log: &'a Dragline<T, W>,
    pub(super) id: FiberId,
}
impl<'a, T, W> FiberHistory<'a, T, W>
where
    W: Syncable + Seek,
{
    /// Declarative state of the fiber. Returns
    /// [`FiberState::Undefined`] for ids never observed and
    /// [`FiberState::Purged`] for ids that were created, migrated,
    /// and reclaimed.
    #[must_use]
    pub fn state(&self) -> FiberState {
        self.log.reader_view().fiber_state(self.id)
    }
    /// Iterate the fiber's events in commit order (oldest first).
    ///
    /// # Errors
    ///
    /// Returns [`PardosaError::FiberNotFound`] if the fiber id is
    /// unknown to the dragline.
    #[allow(clippy::should_implement_trait, clippy::iter_not_returning_iterator)]
    pub fn iter(&self) -> Result<FiberHistoryIter<'a, T>, PardosaError> {
        let events = self.log.reader_view().history(self.id)?;
        Ok(FiberHistoryIter { events, next: 0 })
    }
    /// Iterate the half-open `[from, to)` window of the fiber's
    /// commit-ordered history (ADR-0018 §D3 partial fiber reads).
    ///
    /// `from > to` and `from == to` both yield an empty iterator
    /// without error. Events whose [`EventId`] lies outside `[from, to)`
    /// are skipped — never errored on — preserving the per-fiber,
    /// no-I/O capability surface.
    ///
    /// # Errors
    ///
    /// Returns [`PardosaError::FiberNotFound`] if the fiber id is
    /// unknown to the dragline.
    pub fn range(
        &self,
        from: EventId,
        to: EventId,
    ) -> Result<FiberHistoryIter<'a, T>, PardosaError> {
        let all = self.log.reader_view().history(self.id)?;
        let events = if from >= to {
            Vec::new()
        } else {
            all.into_iter()
                .filter(|e| {
                    let id = e.event_id();
                    id >= from && id < to
                })
                .collect()
        };
        Ok(FiberHistoryIter { events, next: 0 })
    }
    /// Iterate the suffix of the fiber's history starting at the
    /// first event whose [`EventId`] is `>= from` (ADR-0018 §D3
    /// partial fiber reads).
    ///
    /// When `from` precedes every event on the fiber, the whole
    /// history is yielded; when `from` exceeds every event, the
    /// iterator is empty.
    ///
    /// # Errors
    ///
    /// Returns [`PardosaError::FiberNotFound`] if the fiber id is
    /// unknown to the dragline.
    #[allow(clippy::wrong_self_convention)]
    pub fn from_event_id(&self, from: EventId) -> Result<FiberHistoryIter<'a, T>, PardosaError> {
        let events = self
            .log
            .reader_view()
            .history(self.id)?
            .into_iter()
            .filter(|e| e.event_id() >= from)
            .collect();
        Ok(FiberHistoryIter { events, next: 0 })
    }
    /// Iterate the first `n` events of the fiber's commit-ordered
    /// history (ADR-0018 §D3 partial fiber reads). Saturates when
    /// `n` exceeds the history length; `n == 0` yields an empty
    /// iterator.
    ///
    /// # Errors
    ///
    /// Returns [`PardosaError::FiberNotFound`] if the fiber id is
    /// unknown to the dragline.
    pub fn take(&self, n: usize) -> Result<FiberHistoryIter<'a, T>, PardosaError> {
        let events = self
            .log
            .reader_view()
            .history(self.id)?
            .into_iter()
            .take(n)
            .collect();
        Ok(FiberHistoryIter { events, next: 0 })
    }
    /// Stream the fiber's events in reverse-chronological order
    /// (newest first) without per-call allocation
    /// (ADR-0018 §11 bullet 1).
    ///
    /// Walks the precursor chain directly and yields each
    /// [`Event<T>`] lazily. Prefer over `iter()?.rev()` (which
    /// pays the chronological `Vec` allocation up front).
    ///
    /// # Errors
    ///
    /// [`PardosaError::FiberNotFound`] if the fiber id is unknown
    /// to the dragline.
    pub fn iter_rev(&self) -> Result<HistoryStream<'a, T>, PardosaError> {
        self.log.reader_view().history_stream(self.id)
    }
}
/// Iterator yielded by `FiberHistory::iter`.
pub struct FiberHistoryIter<'a, T> {
    events: Vec<&'a Event<T>>,
    next: usize,
}
impl<'a, T> Iterator for FiberHistoryIter<'a, T> {
    type Item = &'a Event<T>;
    fn next(&mut self) -> Option<Self::Item> {
        let e = self.events.get(self.next)?;
        self.next += 1;
        Some(*e)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.events.len().saturating_sub(self.next);
        (remaining, Some(remaining))
    }
}
/// Streaming, zero-allocation reverse-chronological iterator over
/// a fiber's history (newest first). Returned by
/// `FiberHistory::iter_rev` (ADR-0018 §11 bullet 1).
///
/// Walks the precursor chain backwards within the in-memory
/// dragline; each [`Iterator::next`] performs one slice index
/// and one precursor decode. Terminates at [`crate::event::Precursor::Genesis`]
/// or when the precursor index falls outside the slice.
///
/// Implements [`core::iter::FusedIterator`]; `size_hint` is exact.
pub struct HistoryStream<'a, T> {
    line: &'a [Event<T>],
    next: Option<Index>,
    remaining: usize,
}
impl<'a, T> HistoryStream<'a, T> {
    pub(crate) fn new(line: &'a [Event<T>], head: Option<Index>, remaining: usize) -> Self {
        Self {
            line,
            next: head,
            remaining,
        }
    }
}
impl<'a, T> Iterator for HistoryStream<'a, T> {
    type Item = &'a Event<T>;
    fn next(&mut self) -> Option<Self::Item> {
        let idx = self.next.take()?;
        let event = self.line.get(usize::try_from(idx).ok()?)?;
        self.next = event.precursor().as_index();
        self.remaining = self.remaining.saturating_sub(1);
        Some(event)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl<T> core::iter::FusedIterator for HistoryStream<'_, T> {}
