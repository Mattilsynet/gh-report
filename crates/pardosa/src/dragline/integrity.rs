#[cfg(test)]
use super::state::Line;
#[cfg(test)]
use crate::error::{FiberInvariantKind, IntegrityKind, PardosaError};
#[cfg(test)]
use pardosa_wire::{Encode, precursor_hash_of, to_vec};
#[cfg(test)]
impl<T> Line<T> {
    /// Verify every cross-event invariant on the dragline: monotonic event ids, valid
    /// fiber indices, precursor-chain consistency, anchor-interval bounds, etc.
    ///
    /// # Errors
    /// Returns `PardosaError` on the first invariant violation; categories include
    /// `BrokenPrecursorChain`, `FiberInvariant(FiberInvariantKind::Integrity(_))`,
    /// and related structural-state failures.
    pub fn verify_invariants(&self) -> Result<(), PardosaError>
    where
        T: Encode,
    {
        self.verify_precursor_chains()?;
        for (position, event) in self.line.as_slice().iter().enumerate() {
            let position_u64 = u64::try_from(position).map_err(|_| {
                PardosaError::FiberInvariant(FiberInvariantKind::CounterSaturated {
                    counter: "line_position",
                })
            })?;
            if event.event_id().value() != position_u64 {
                return Err(PardosaError::FiberInvariant(FiberInvariantKind::Integrity(
                    IntegrityKind::EventIdPositionMismatch {
                        event_id: event.event_id().value(),
                        position: position_u64,
                    },
                )));
            }
        }
        for purged in &self.purged_ids {
            if self.lookup.contains_key(purged) {
                return Err(PardosaError::FiberInvariant(FiberInvariantKind::Integrity(
                    IntegrityKind::PurgedIdInLookup(*purged),
                )));
            }
        }
        let expected_next = match self.line.as_slice().last() {
            None => 0,
            Some(last) => {
                last.event_id()
                    .value()
                    .checked_add(1)
                    .ok_or(PardosaError::FiberInvariant(
                        FiberInvariantKind::CounterSaturated {
                            counter: "next_event_id",
                        },
                    ))?
            }
        };
        if self.next_event_id.value() != expected_next {
            return Err(PardosaError::FiberInvariant(FiberInvariantKind::Integrity(
                IntegrityKind::NextEventIdMismatch {
                    actual: self.next_event_id.value(),
                    expected: expected_next,
                    line_len: self.line.len(),
                },
            )));
        }
        let line_len = self.line.len();
        for (id, (fiber, _)) in &self.lookup {
            let cur = usize::try_from(fiber.current())?;
            if cur >= line_len {
                return Err(PardosaError::FiberInvariant(FiberInvariantKind::Integrity(
                    IntegrityKind::FiberCurrentOutOfBounds {
                        fiber_id: *id,
                        current: cur,
                        line_len,
                    },
                )));
            }
        }
        Ok(())
    }
    /// Verify the per-event precursor pointer + precursor hash for every event.
    ///
    /// # Errors
    /// Returns `PardosaError::BrokenPrecursorChain` on the first event whose stored
    /// precursor hash does not match the recomputed hash of the referenced event.
    pub fn verify_precursor_chains(&self) -> Result<(), PardosaError>
    where
        T: Encode,
    {
        for (i, event) in self.line.as_slice().iter().enumerate() {
            let precursor = event.precursor();
            let Some(precursor_idx) = precursor.as_index() else {
                continue;
            };
            let Ok(pidx_usize) = usize::try_from(precursor_idx) else {
                return Err(PardosaError::BrokenPrecursorChain {
                    event_id: event.event_id().value(),
                    precursor: precursor_idx,
                });
            };
            if pidx_usize >= i {
                return Err(PardosaError::BrokenPrecursorChain {
                    event_id: event.event_id().value(),
                    precursor: precursor_idx,
                });
            }
            let precursor_event = &self.line[pidx_usize];
            if precursor_event.fiber_id() != event.fiber_id() {
                return Err(PardosaError::BrokenPrecursorChain {
                    event_id: event.event_id().value(),
                    precursor: precursor_idx,
                });
            }
            let expected = precursor_hash_of(&to_vec(precursor_event));
            let actual = event.precursor_hash();
            if actual != expected {
                return Err(PardosaError::PrecursorHashMismatch {
                    event_id: event.event_id().value(),
                    expected,
                    actual,
                });
            }
        }
        Ok(())
    }
}
