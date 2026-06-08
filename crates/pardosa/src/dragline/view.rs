//! `DraglineView<'a, T>` — read-only capability borrow of a
//! [`Line<T>`](super::state::Line).
//!
//! Per ADR-0016 §D2, the typed read-capability surface a reader
//! takes by reference. Hands out the read accessors on
//! `Line<T>` that the `StoreReader` path reaches:
//! `fiber_state`, `history`, `history_stream`, `read_line`.
//! Omits mutators per ADR-0016 §D2 binding negative list.
//!
//! `Copy`, zero-cost: `&'a Line<T>` under the hood. Acquire via
//! [`Dragline::reader_view`](crate::dragline::Dragline::reader_view) (canonical)
//! or [`DraglineView::from`] (test reborrow).
use super::state::Line;
use crate::error::PardosaError;
use crate::event::{Event, FiberId};
use crate::fiber_state::FiberState;
/// Read-only capability borrow of a [`Line<T>`].
///
/// `Copy`, zero-cost, lifetime-bound. See module docs for the binding
/// capability contract (ADR-0016 §D2).
#[derive(Debug)]
pub(crate) struct DraglineView<'a, T> {
    inner: &'a Line<T>,
}
impl<T> Clone for DraglineView<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for DraglineView<'_, T> {}
impl<'a, T> From<&'a Line<T>> for DraglineView<'a, T> {
    fn from(inner: &'a Line<T>) -> Self {
        Self { inner }
    }
}
impl<'a, T> DraglineView<'a, T> {
    /// Construct a view over an existing line borrow.
    ///
    /// Equivalent to [`DraglineView::from`]; provided for call sites
    /// that prefer the explicit constructor spelling.
    #[must_use]
    pub fn new(line: &'a Line<T>) -> Self {
        Self { inner: line }
    }
    /// Walk the precursor chain for `fiber_id`, returning events in
    /// oldest-first order.
    ///
    /// # Errors
    /// Returns [`PardosaError::FiberNotFound`] if the fiber id is
    /// unknown.
    pub fn history(self, fiber_id: FiberId) -> Result<Vec<&'a Event<T>>, PardosaError> {
        self.inner.history(fiber_id)
    }
    /// Stream `fiber_id`'s history newest-first without per-call
    /// allocation (ADR-0018 §11 bullet 1).
    ///
    /// Companion to [`Self::history`]: same view, reverse-chronological
    /// order, no `Vec` materialisation. See
    /// [`crate::store::FiberHistory::iter_rev`] for the public adopter
    /// entry point.
    ///
    /// # Errors
    /// Returns [`PardosaError::FiberNotFound`] if the fiber id is
    /// unknown.
    pub fn history_stream(
        self,
        fiber_id: FiberId,
    ) -> Result<crate::store::HistoryStream<'a, T>, PardosaError> {
        self.inner.history_stream(fiber_id)
    }
    /// Borrow the full event line slice in commit order.
    #[must_use]
    pub fn read_line(self) -> &'a [Event<T>] {
        self.inner.read_line()
    }
    /// Declarative state of `fiber_id`.
    ///
    /// Returns [`FiberState::Undefined`] for ids never observed and
    /// [`FiberState::Purged`] for ids that were created, migrated, and
    /// reclaimed.
    #[must_use]
    pub fn fiber_state(self, fiber_id: FiberId) -> FiberState {
        self.inner.fiber_state(fiber_id)
    }
}
