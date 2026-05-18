use crate::error::PardosaError;
use crate::event::Event;

/// Append-only event log with invariant enforcement at write time.
///
/// Newtype wrapper around `Vec<Event<T>>` whose sole production write path
/// is [`Linevec::append_validated`]. Future replay/migration/snapshot code
/// reaching for `Vec::push` is a compile error, forcing every append through
/// the validator regardless of whether the caller remembered to call
/// `verify_invariants` after the fact.
///
/// The accepted positional/relational invariants are documented on
/// [`Linevec::append_validated`]. Sentinel rejection on
/// [`crate::event::Index`] values
/// already happens at the outer `IndexRaw` deserialization layer (FH1), so
/// this layer focuses on positional facts that only the line can see.
#[derive(Debug)]
pub(crate) struct Linevec<T>(Vec<Event<T>>);

impl<T> Default for Linevec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Linevec<T> {
    /// Create an empty linevec.
    pub(crate) const fn new() -> Self {
        Linevec(Vec::new())
    }

    /// Append `event` after validating positional/relational invariants.
    ///
    /// Checks (all must hold):
    /// 1. `event.event_id() == expected_event_id` (caller-supplied monotonic
    ///    counter matches the event being appended).
    /// 2. `expected_event_id > last.event_id()` when the line is non-empty
    ///    (strict monotonicity across the line).
    /// 3. `event.precursor()` is either `Index::NONE`, or points strictly
    ///    backwards within the existing line (`precursor.as_usize() < self.0.len()`).
    /// 4. When the precursor is set, the referenced event has the same
    ///    `domain_id` as `event` (no cross-domain precursor splicing).
    ///
    /// On success the event is pushed and `Ok(())` is returned. On failure
    /// the line is unmodified and a [`PardosaError`] describes the violation.
    ///
    /// Sentinel rejection on the precursor `Index` value itself happens at
    /// the deserialization boundary (FH1, `IndexRaw`); this method assumes
    /// the `Index` inside `event` is well-formed and only checks the
    /// positional/relational facts that depend on the line's current state.
    ///
    /// # Errors
    ///
    /// Returns [`PardosaError::FiberInvariantViolation`] for monotonicity
    /// or precursor-bounds violations, or [`PardosaError::BrokenPrecursorChain`]
    /// for cross-domain precursors (matching the variant existing callers of
    /// `verify_precursor_chains` already pattern-match on).
    pub(crate) fn append_validated(
        &mut self,
        event: Event<T>,
        expected_event_id: u64,
    ) -> Result<(), PardosaError> {
        // (1) event_id matches caller's expected counter
        if event.event_id() != expected_event_id {
            return Err(PardosaError::FiberInvariantViolation(format!(
                "append_validated: event.event_id() {} != expected {}",
                event.event_id(),
                expected_event_id,
            )));
        }

        // (2) strict monotonicity vs last event
        if let Some(last) = self.0.last()
            && expected_event_id <= last.event_id()
        {
            return Err(PardosaError::FiberInvariantViolation(format!(
                "append_validated: event_id {} not strictly greater than last {}",
                expected_event_id,
                last.event_id(),
            )));
        }

        // (3) precursor in-bounds (strictly backwards) when set
        let precursor = event.precursor();
        if precursor.is_some() {
            let p = precursor.as_usize();
            if p >= self.0.len() {
                return Err(PardosaError::FiberInvariantViolation(format!(
                    "append_validated: precursor index {} not strictly less than line.len() {}",
                    p,
                    self.0.len(),
                )));
            }
            // (4) precursor refers to same-domain event
            if self.0[p].domain_id() != event.domain_id() {
                return Err(PardosaError::BrokenPrecursorChain {
                    event_id: event.event_id(),
                    precursor,
                });
            }
        }

        self.0.push(event);
        Ok(())
    }

    /// Number of events.
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    /// Most recent event, if any.
    pub(crate) fn last(&self) -> Option<&Event<T>> {
        self.0.last()
    }

    /// Iterate over events in append order.
    pub(crate) fn iter(&self) -> std::slice::Iter<'_, Event<T>> {
        self.0.iter()
    }

    /// Sliding windows over the events (read-only).
    pub(crate) fn windows(&self, size: usize) -> std::slice::Windows<'_, Event<T>> {
        self.0.windows(size)
    }

    /// Borrow the full slice (read-only).
    pub(crate) fn as_slice(&self) -> &[Event<T>] {
        &self.0
    }

    /// Test-only escape hatch: push without validation.
    ///
    /// Mirrors the `Index::new_unchecked` and FH2 `from_raw_parts` patterns —
    /// negative tests for invariant-checking code paths must be able to
    /// construct broken state. Hidden behind `cfg(test)` so production code
    /// cannot reach it.
    #[cfg(test)]
    pub(crate) fn force_push_unchecked(&mut self, event: Event<T>) {
        self.0.push(event);
    }

    /// Test/persistence-boundary helper: wrap a pre-existing `Vec` without
    /// validation. The caller is responsible for re-running
    /// `Dragline::verify_invariants` before exposing the dragline. Gated
    /// to `cfg(test)` for the same reason `Dragline::from_raw_parts` is.
    #[cfg(test)]
    pub(crate) fn from_raw_unchecked(events: Vec<Event<T>>) -> Self {
        Linevec(events)
    }
}

impl<T: Clone> Clone for Linevec<T> {
    fn clone(&self) -> Self {
        Linevec(self.0.clone())
    }
}

impl<T> std::ops::Index<usize> for Linevec<T> {
    type Output = Event<T>;
    fn index(&self, i: usize) -> &Event<T> {
        &self.0[i]
    }
}
