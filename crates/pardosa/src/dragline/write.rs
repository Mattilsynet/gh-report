use super::commit::{LookupOp, PreparedCommit};
use super::state::{AppendResult, Line};
use crate::error::PardosaError;
use crate::event::{Event, FiberId, Precursor};
use crate::fiber::Fiber;
use crate::fiber_state::{
    FiberAction, FiberMigrationPolicy, FiberState, LockedRescuePolicy, transition,
};
use pardosa_wire::{Encode, precursor_hash_of, to_vec};
impl<T> Line<T> {
    /// Create a new fiber with a fresh id, appending a single creation event.
    ///
    /// # Errors
    /// Returns `PardosaError` if id allocation, fiber-state transition, event-id
    /// assignment, or commit fails (`IdAlreadyExists`, `InvalidTransition`,
    /// `EventIdOverflow`, etc.).
    pub fn create(&mut self, domain_event: T) -> Result<AppendResult, PardosaError>
    where
        T: Encode,
    {
        self.commit_atomic(|s| {
            let mut fiber_id = s.next_id;
            while s.purged_ids.contains(&fiber_id) {
                fiber_id = fiber_id.checked_next()?;
            }
            if s.lookup.contains_key(&fiber_id) {
                return Err(PardosaError::IdAlreadyExists(fiber_id));
            }
            let new_state = transition(FiberState::Undefined, FiberAction::Create)?;
            let event_id = s.peek_event_id()?;
            let index = s.next_index()?;
            let next_fiber_id = fiber_id.checked_next()?;
            let fiber = Fiber::new(index, 1, index)?;
            let event = Event::new_unchecked(
                event_id,
                fiber_id,
                false,
                Precursor::Genesis,
                [0u8; 32],
                domain_event,
            );
            Ok(PreparedCommit {
                event,
                event_id,
                fiber_id,
                lookup_op: LookupOp::Insert {
                    fiber,
                    state: new_state,
                },
                next_id_advance: Some(next_fiber_id),
            })
        })
    }
    /// Append an update event to an existing fiber, advancing its current pointer.
    ///
    /// # Errors
    /// Returns `PardosaError::FiberNotFound` if `fiber_id` is unknown, or any
    /// commit-pipeline error (`InvalidTransition`, `EventIdOverflow`, …).
    pub fn update(
        &mut self,
        fiber_id: FiberId,
        domain_event: T,
    ) -> Result<AppendResult, PardosaError>
    where
        T: Encode,
    {
        self.commit_atomic(|s| {
            let (fiber, state) = s
                .lookup
                .get(&fiber_id)
                .ok_or(PardosaError::FiberNotFound(fiber_id))?;
            let _new_state = transition(*state, FiberAction::Update)?;
            let event_id = s.peek_event_id()?;
            let index = s.next_index()?;
            let precursor_idx = fiber.current();
            fiber.check_advance(index)?;
            let precursor = Precursor::Of(precursor_idx);
            let precursor_pos = usize::try_from(precursor_idx)?;
            let event = Event::new_unchecked(
                event_id,
                fiber_id,
                false,
                precursor,
                precursor_hash_of(&to_vec(&s.line[precursor_pos])),
                domain_event,
            );
            Ok(PreparedCommit {
                event,
                event_id,
                fiber_id,
                lookup_op: LookupOp::AdvanceFiber {
                    new_current: index,
                    new_state: None,
                },
                next_id_advance: None,
            })
        })
    }
    /// Append a detach event marking the fiber as `Detached`.
    ///
    /// # Errors
    /// Returns `PardosaError::FiberNotFound` if `fiber_id` is unknown, or any
    /// commit-pipeline error (`InvalidTransition`, `EventIdOverflow`, …).
    pub fn detach(
        &mut self,
        fiber_id: FiberId,
        domain_event: T,
    ) -> Result<AppendResult, PardosaError>
    where
        T: Encode,
    {
        self.commit_atomic(|s| {
            let (fiber, state) = s
                .lookup
                .get(&fiber_id)
                .ok_or(PardosaError::FiberNotFound(fiber_id))?;
            let new_state = transition(*state, FiberAction::Detach)?;
            let event_id = s.peek_event_id()?;
            let index = s.next_index()?;
            let precursor_idx = fiber.current();
            fiber.check_advance(index)?;
            let precursor = Precursor::Of(precursor_idx);
            let precursor_pos = usize::try_from(precursor_idx)?;
            let event = Event::new_unchecked(
                event_id,
                fiber_id,
                true,
                precursor,
                precursor_hash_of(&to_vec(&s.line[precursor_pos])),
                domain_event,
            );
            Ok(PreparedCommit {
                event,
                event_id,
                fiber_id,
                lookup_op: LookupOp::AdvanceFiber {
                    new_current: index,
                    new_state: Some(new_state),
                },
                next_id_advance: None,
            })
        })
    }
    /// Rescue a `Detached` fiber by appending a rescue event that
    /// continues the precursor chain.
    ///
    /// [`LockedRescuePolicy`] is honoured at the substrate
    /// boundary:
    ///
    /// * `PreserveAuditTrail` on `Detached` — the only supported
    ///   pair; produces "continue the chain".
    /// * `PreserveAuditTrail` on `Locked` →
    ///   [`PardosaError::RescuePolicyUnsupported`].
    /// * `AcceptDataLoss` → rejected for every state. Use
    ///   [`crate::migrate::migrate_keep`] out-of-band instead.
    ///
    /// # Errors
    ///
    /// `RescuePolicyUnsupported` for the rejected pairs above,
    /// `FiberNotFound` if `fiber_id` is unknown,
    /// `InvalidTransition` for other states, or any
    /// commit-pipeline error.
    pub fn rescue(
        &mut self,
        fiber_id: FiberId,
        policy: LockedRescuePolicy,
        domain_event: T,
    ) -> Result<AppendResult, PardosaError>
    where
        T: Encode,
    {
        if matches!(policy, LockedRescuePolicy::AcceptDataLoss) {
            let state = self
                .lookup
                .get(&fiber_id)
                .map_or(FiberState::Undefined, |(_, s)| *s);
            return Err(PardosaError::RescuePolicyUnsupported { policy, state });
        }
        self.commit_atomic(|s| {
            let (fiber, state) = s
                .lookup
                .get(&fiber_id)
                .ok_or(PardosaError::FiberNotFound(fiber_id))?;
            let current_state = *state;
            let new_state = transition(current_state, FiberAction::Rescue)?;
            let event_id = s.peek_event_id()?;
            let index = s.next_index()?;
            let (precursor, lookup_op) = match current_state {
                FiberState::Locked => {
                    return Err(PardosaError::RescuePolicyUnsupported {
                        policy,
                        state: current_state,
                    });
                }
                FiberState::Detached => {
                    fiber.check_advance(index)?;
                    (
                        Precursor::Of(fiber.current()),
                        LookupOp::AdvanceFiber {
                            new_current: index,
                            new_state: Some(new_state),
                        },
                    )
                }
                other => {
                    return Err(PardosaError::InvalidTransition {
                        state: other,
                        action: FiberAction::Rescue,
                    });
                }
            };
            let event = Event::new_unchecked(
                event_id,
                fiber_id,
                false,
                precursor,
                match precursor.as_index() {
                    None => [0u8; 32],
                    Some(idx) => precursor_hash_of(&to_vec(&s.line[usize::try_from(idx)?])),
                },
                domain_event,
            );
            Ok(PreparedCommit {
                event,
                event_id,
                fiber_id,
                lookup_op,
                next_id_advance: None,
            })
        })
    }
    /// Apply a migration policy to a fiber, transitioning its state and updating the
    /// purged-id set as appropriate.
    ///
    /// # Errors
    /// Returns `PardosaError::FiberNotFound` if `fiber_id` is unknown, or
    /// `PardosaError::InvalidTransition` if the fiber state does not admit the
    /// requested `FiberMigrationPolicy`.
    ///
    /// # Panics
    /// The `lookup.get_mut(&fiber_id).unwrap()` is unreachable because the preceding
    /// `lookup.get(&fiber_id)` succeeded and the borrow is dropped before the
    /// `get_mut` — the entry is structurally present.
    #[allow(
        dead_code,
        reason = "per-fiber Migrate(FiberMigrationPolicy) typestate gate; no public per-fiber Migrate API in ADR-0018, and migrate_keep is whole-stream Keep that replays via commit_event/update/detach/rescue rather than invoking migrate_fiber — retention rationale recorded in closed bead rescue-pardosa-bnbu (Port 02 Branch A follow-up)"
    )]
    pub fn migrate_fiber(
        &mut self,
        fiber_id: FiberId,
        policy: FiberMigrationPolicy,
    ) -> Result<(), PardosaError> {
        let (_, state) = self
            .lookup
            .get(&fiber_id)
            .ok_or(PardosaError::FiberNotFound(fiber_id))?;
        let new_state = transition(*state, FiberAction::Migrate(policy))?;
        match new_state {
            FiberState::Purged => {
                self.lookup.remove(&fiber_id);
                self.purged_ids.insert(fiber_id);
            }
            _ => {
                self.lookup
                    .get_mut(&fiber_id)
                    .expect("entry present: preceding lookup.get succeeded for this fiber_id")
                    .1 = new_state;
            }
        }
        Ok(())
    }
    #[allow(
        dead_code,
        reason = "paired with migrate_fiber; commit-rejection flag for the per-fiber Migrate typestate gate. No public per-fiber Migrate API in ADR-0018; retention rationale recorded in closed bead rescue-pardosa-bnbu (Port 02 Branch A follow-up)"
    )]
    pub fn set_migrating(&mut self, migrating: bool) {
        self.migrating = migrating;
    }
    #[must_use]
    #[allow(
        dead_code,
        reason = "paired with migrate_fiber; reads the commit-rejection flag for the per-fiber Migrate typestate gate. No public per-fiber Migrate API in ADR-0018; retention rationale recorded in closed bead rescue-pardosa-bnbu (Port 02 Branch A follow-up)"
    )]
    pub fn is_migrating(&self) -> bool {
        self.migrating
    }
}
