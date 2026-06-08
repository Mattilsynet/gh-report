use super::state::Line;
use crate::error::PardosaError;
use crate::event::{Event, FiberId};
#[cfg(test)]
use crate::fiber_state::FiberState;
impl<T> Line<T> {
    /// Read the latest event for a live fiber.
    ///
    /// # Errors
    /// Returns `PardosaError::FiberNotFound` if the fiber id is unknown or if its
    /// state is not `FiberState::Defined` (detached / migrated fibers are hidden).
    #[cfg(test)]
    pub fn read(&self, fiber_id: FiberId) -> Result<&Event<T>, PardosaError> {
        let (fiber, state) = self
            .lookup
            .get(&fiber_id)
            .ok_or(PardosaError::FiberNotFound(fiber_id))?;
        if *state != FiberState::Defined {
            return Err(PardosaError::FiberNotFound(fiber_id));
        }
        Ok(&self.line[usize::try_from(fiber.current())?])
    }
    /// Read the latest event for a fiber including detached / migrated states.
    ///
    /// # Errors
    /// Returns `PardosaError::FiberNotFound` if the fiber id is unknown.
    #[cfg(test)]
    pub fn read_with_deleted(&self, fiber_id: FiberId) -> Result<&Event<T>, PardosaError> {
        let (fiber, _) = self
            .lookup
            .get(&fiber_id)
            .ok_or(PardosaError::FiberNotFound(fiber_id))?;
        Ok(&self.line[usize::try_from(fiber.current())?])
    }
    #[must_use]
    #[cfg(test)]
    pub fn list(&self) -> Vec<FiberId> {
        self.lookup
            .iter()
            .filter(|(_, (_, state))| *state == FiberState::Defined)
            .map(|(id, _)| *id)
            .collect()
    }
    #[must_use]
    #[cfg(test)]
    pub fn list_with_deleted(&self) -> Vec<FiberId> {
        self.lookup.keys().copied().collect()
    }
    /// Walk the precursor chain for `fiber_id`, returning events in oldest-first order.
    ///
    /// # Errors
    /// Returns `PardosaError::FiberNotFound` if the fiber id is unknown.
    pub fn history(&self, fiber_id: FiberId) -> Result<Vec<&Event<T>>, PardosaError> {
        let (fiber, _) = self
            .lookup
            .get(&fiber_id)
            .ok_or(PardosaError::FiberNotFound(fiber_id))?;
        let capacity = usize::try_from(fiber.len()).unwrap_or(usize::MAX);
        let mut events = Vec::with_capacity(capacity);
        let mut cursor = Some(fiber.current());
        while let Some(idx) = cursor {
            let event = &self.line[usize::try_from(idx)?];
            events.push(event);
            cursor = event.precursor().as_index();
        }
        events.reverse();
        Ok(events)
    }
    #[must_use]
    pub fn read_line(&self) -> &[Event<T>] {
        self.line.as_slice()
    }
    /// Stream `fiber_id`'s history newest-first as a zero-allocation
    /// reverse-chronological walk of the precursor chain (ADR-0018
    /// §11 bullet 1).
    ///
    /// Builds a [`crate::store::HistoryStream`] over the dragline's
    /// event slice; no `Vec` is materialised.
    ///
    /// # Errors
    /// Returns `PardosaError::FiberNotFound` if the fiber id is unknown.
    pub fn history_stream(
        &self,
        fiber_id: FiberId,
    ) -> Result<crate::store::HistoryStream<'_, T>, PardosaError> {
        let (fiber, _) = self
            .lookup
            .get(&fiber_id)
            .ok_or(PardosaError::FiberNotFound(fiber_id))?;
        let remaining = usize::try_from(fiber.len()).unwrap_or(usize::MAX);
        Ok(crate::store::HistoryStream::new(
            self.line.as_slice(),
            Some(fiber.current()),
            remaining,
        ))
    }
}
