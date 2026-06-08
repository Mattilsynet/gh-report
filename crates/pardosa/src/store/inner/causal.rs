use super::{Event, EventId, FiberId, Precursor, Syncable};
use crate::dragline::Dragline;
use crate::event::{IndexTooLargeForUsize, event_id_to_line_position};
use std::io::Seek;
/// Same-fiber causal-replay walk returned by
/// [`super::StoreReader::causal_chain`] (ADR-0018 §D3 (b)).
///
/// Walks precursor pointers within a single fiber in the current
/// dragline. A `Precursor::Genesis` or an out-of-fiber /
/// out-of-dragline precursor terminates the chain without error
/// (ADR-0018 §D6).
///
/// Adopter code names this as `CausalChain<'_, T>` — the sink
/// parameter defaults to [`std::fs::File`] (ADR-0018 § Naming).
pub struct CausalChain<'a, T, W: Syncable + Seek = std::fs::File> {
    pub(super) log: &'a Dragline<T, W>,
    pub(super) head: EventId,
}
impl<'a, T, W> CausalChain<'a, T, W>
where
    W: Syncable + Seek,
{
    /// Iterate the chain in **head-first** order: starts at the
    /// head event and walks precursor pointers backwards within
    /// the same fiber.
    ///
    /// Terminates when the precursor is [`Precursor::Genesis`] or
    /// when it points outside the current dragline / outside the
    /// head's fiber (the substrate validates precursor refs at
    /// commit time but does not enforce a strong DAG — ADR-0003
    /// §3).
    #[must_use]
    pub fn iter(&self) -> CausalChainIter<'a, T> {
        let view = self.log.reader_view();
        let line = view.read_line();
        CausalChainIter {
            line,
            next_id: Some(self.head),
            fiber_id: None,
        }
    }
    /// Strict variant of [`Self::iter`]: surfaces abnormal
    /// terminations as typed [`CausalChainError`] instead of
    /// ending the iterator silently.
    ///
    /// Normal terminations ([`Precursor::Genesis`]) end without an
    /// error item. Abnormal (index out of range; precursor on a
    /// different fiber than the head; `u64`→`usize` failure on
    /// non-64-bit targets) yield one [`Err`] item and end.
    ///
    /// Default callers use [`Self::iter`]; reach for `iter_strict`
    /// when validating a recovered dragline.
    #[must_use]
    pub fn iter_strict(&self) -> CausalChainStrictIter<'a, T> {
        let view = self.log.reader_view();
        let line = view.read_line();
        CausalChainStrictIter {
            line,
            next_id: Some(self.head),
            fiber_id: None,
            done: false,
        }
    }
}
impl<'a, T, W> IntoIterator for &CausalChain<'a, T, W>
where
    W: Syncable + Seek,
{
    type Item = &'a Event<T>;
    type IntoIter = CausalChainIter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
/// Iterator yielded by `CausalChain::iter`.
///
/// Each `next` resolves the current `EventId` against the line,
/// emits the borrowed [`Event<T>`], and advances `next_id` to the
/// event's precursor. Termination conditions (head-first): the
/// precursor is [`Precursor::Genesis`]; the precursor's resolved
/// event is on a different fiber than the head; or the precursor's
/// index is out of range of the current dragline.
pub struct CausalChainIter<'a, T> {
    line: &'a [Event<T>],
    next_id: Option<EventId>,
    fiber_id: Option<FiberId>,
}
impl<'a, T> Iterator for CausalChainIter<'a, T> {
    type Item = &'a Event<T>;
    fn next(&mut self) -> Option<Self::Item> {
        let id = self.next_id.take()?;
        let idx = event_id_to_line_position(id).ok()?;
        let ev = self.line.get(idx)?;
        match self.fiber_id {
            None => self.fiber_id = Some(ev.fiber_id()),
            Some(expected) if expected == ev.fiber_id() => {}
            Some(_) => return None,
        }
        self.next_id = match ev.precursor() {
            Precursor::Of(prev_idx) => Some(EventId::from_decoded(prev_idx.value())),
            Precursor::Genesis => None,
        };
        Some(ev)
    }
}
/// Abnormal-termination causes surfaced by
/// `CausalChain::iter_strict`.
///
/// `iter_strict` yields exactly one [`Err`] variant when the
/// walk cannot continue for a reason other than the natural
/// [`Precursor::Genesis`] terminus, then ends the iterator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CausalChainError {
    /// A precursor pointer resolves outside the current
    /// dragline's `read_line()` slice.
    PrecursorOutOfRange {
        /// The offending `EventId` (i.e. the line index that
        /// was missing).
        event_id: EventId,
    },
    /// A precursor pointer resolved to an event on a different
    /// fiber than the head of the chain.
    PrecursorWrongFiber {
        /// The fiber the head established for this walk.
        head_fiber: FiberId,
        /// The fiber the resolved event actually carries.
        found_fiber: FiberId,
        /// The `EventId` whose resolved fiber mismatched.
        event_id: EventId,
    },
    /// A precursor `EventId` value exceeds `usize::MAX` on the
    /// current target. Cannot occur on 64-bit targets — the
    /// crate already refuses to compile on non-64-bit targets
    /// (see `crates/pardosa/src/lib.rs:2`), but the variant is
    /// kept so the typed error path is exhaustive.
    IndexTooLargeForUsize(IndexTooLargeForUsize),
}
impl From<IndexTooLargeForUsize> for CausalChainError {
    fn from(e: IndexTooLargeForUsize) -> Self {
        Self::IndexTooLargeForUsize(e)
    }
}
impl std::fmt::Display for CausalChainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PrecursorOutOfRange { event_id } => {
                write!(
                    f,
                    "causal_chain: precursor {event_id:?} out of dragline range"
                )
            }
            Self::PrecursorWrongFiber {
                head_fiber,
                found_fiber,
                event_id,
            } => {
                write!(
                    f,
                    "causal_chain: precursor {event_id:?} resolves to fiber \
                 {found_fiber:?} but head is on fiber {head_fiber:?}"
                )
            }
            Self::IndexTooLargeForUsize(e) => write!(f, "causal_chain: {e}"),
        }
    }
}
impl std::error::Error for CausalChainError {}
/// Strict iterator yielded by `CausalChain::iter_strict`.
///
/// Each `next` resolves the current `EventId` against the line
/// and emits `Ok(&Event<T>)`, or yields a single
/// [`CausalChainError`] and then terminates. Natural
/// [`Precursor::Genesis`] termination ends the iterator without
/// an `Err` item.
pub struct CausalChainStrictIter<'a, T> {
    line: &'a [Event<T>],
    next_id: Option<EventId>,
    fiber_id: Option<FiberId>,
    done: bool,
}
impl<'a, T> Iterator for CausalChainStrictIter<'a, T> {
    type Item = Result<&'a Event<T>, CausalChainError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let id = self.next_id.take()?;
        let idx = match event_id_to_line_position(id) {
            Ok(i) => i,
            Err(e) => {
                self.done = true;
                return Some(Err(e.into()));
            }
        };
        let Some(ev) = self.line.get(idx) else {
            self.done = true;
            return Some(Err(CausalChainError::PrecursorOutOfRange { event_id: id }));
        };
        match self.fiber_id {
            None => self.fiber_id = Some(ev.fiber_id()),
            Some(expected) if expected == ev.fiber_id() => {}
            Some(expected) => {
                self.done = true;
                return Some(Err(CausalChainError::PrecursorWrongFiber {
                    head_fiber: expected,
                    found_fiber: ev.fiber_id(),
                    event_id: id,
                }));
            }
        }
        self.next_id = match ev.precursor() {
            Precursor::Of(prev_idx) => Some(EventId::from_decoded(prev_idx.value())),
            Precursor::Genesis => None,
        };
        Some(Ok(ev))
    }
}
#[cfg(test)]
mod tests {
    use super::{CausalChainError, CausalChainStrictIter};
    use crate::event::{Event, EventId, FiberId, Index, Precursor};
    fn ev(event_id: u64, fiber: u64, precursor: Precursor) -> Event<u64> {
        Event::new_unchecked(
            EventId::new(event_id),
            FiberId::new(fiber),
            false,
            precursor,
            [0u8; 32],
            event_id,
        )
    }
    fn strict(line: &[Event<u64>], head: EventId) -> CausalChainStrictIter<'_, u64> {
        CausalChainStrictIter {
            line,
            next_id: Some(head),
            fiber_id: None,
            done: false,
        }
    }
    #[test]
    fn strict_iter_terminates_cleanly_on_genesis() {
        let line = vec![
            ev(0, 0, Precursor::Genesis),
            ev(1, 0, Precursor::Of(Index::from_decoded(0))),
            ev(2, 0, Precursor::Of(Index::from_decoded(1))),
        ];
        let items: Vec<_> = strict(&line, EventId::new(2)).collect();
        assert_eq!(items.len(), 3);
        assert!(items.iter().all(Result::is_ok));
        assert!(
            items
                .iter()
                .all(|r| r.as_ref().unwrap().fiber_id() == FiberId::new(0))
        );
    }
    #[test]
    fn strict_iter_surfaces_out_of_range_precursor() {
        let line = vec![
            ev(0, 0, Precursor::Genesis),
            ev(1, 0, Precursor::Of(Index::from_decoded(999))),
        ];
        let items: Vec<_> = strict(&line, EventId::new(1)).collect();
        assert_eq!(items.len(), 2);
        assert!(items[0].is_ok());
        match items[1] {
            Err(CausalChainError::PrecursorOutOfRange { event_id }) => {
                assert_eq!(event_id, EventId::new(999));
            }
            other => panic!("expected PrecursorOutOfRange, got {other:?}"),
        }
        let again: Vec<_> = strict(&line, EventId::new(1)).skip(2).collect();
        assert!(again.is_empty(), "iterator must terminate after Err");
    }
    #[test]
    fn strict_iter_surfaces_wrong_fiber_precursor() {
        let line = vec![
            ev(0, 7, Precursor::Genesis),
            ev(1, 0, Precursor::Of(Index::from_decoded(0))),
        ];
        let items: Vec<_> = strict(&line, EventId::new(1)).collect();
        assert_eq!(items.len(), 2);
        assert!(items[0].is_ok());
        match items[1] {
            Err(CausalChainError::PrecursorWrongFiber {
                head_fiber,
                found_fiber,
                event_id,
            }) => {
                assert_eq!(head_fiber, FiberId::new(0));
                assert_eq!(found_fiber, FiberId::new(7));
                assert_eq!(event_id, EventId::new(0));
            }
            other => panic!("expected PrecursorWrongFiber, got {other:?}"),
        }
    }
}
