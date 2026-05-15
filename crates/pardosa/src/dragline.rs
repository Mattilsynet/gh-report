use std::collections::{HashMap, HashSet};

use crate::error::PardosaError;
use crate::event::{DomainId, Event, Index};
use crate::fiber::Fiber;
use crate::fiber_state::{
    FiberAction, FiberState, LockedRescuePolicy, MigrationPolicy, transition,
};

/// Result of a successful append operation.
#[derive(Debug, Clone, Copy)]
pub struct AppendResult {
    /// The domain ID of the affected fiber.
    pub domain_id: DomainId,
    /// The globally monotonic event ID assigned to this event.
    pub event_id: u64,
    /// The position of this event in the line.
    pub index: Index,
}

/// The core append-only log with fiber lookup.
///
/// Contains the event line, fiber index, and bookkeeping state.
/// Not thread-safe — wrap in `tokio::sync::RwLock` for concurrent access (Phase 3).
///
/// # Invariants
///
/// - `line` is append-only. Events are never removed or modified.
/// - `lookup` maps each active domain ID to its fiber position and state.
/// - `purged_ids` tracks domain IDs in the Purged state (removed from lookup).
/// - `next_event_id` is globally monotonic, never decreases, even across generations.
/// - `next_id` auto-increments for new fiber creation.
/// - When `migrating` is true, application writes are rejected.
#[derive(Debug)]
pub struct Dragline<T> {
    line: Vec<Event<T>>,
    lookup: HashMap<DomainId, (Fiber, FiberState)>,
    purged_ids: HashSet<DomainId>,
    next_id: DomainId,
    next_event_id: u64,
    migrating: bool,
}

impl<T> Default for Dragline<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Dragline<T> {
    /// Create a new empty dragline.
    #[must_use]
    pub fn new() -> Self {
        Dragline {
            line: Vec::new(),
            lookup: HashMap::new(),
            purged_ids: HashSet::new(),
            next_id: DomainId::new(0),
            next_event_id: 0,
            migrating: false,
        }
    }

    // ── Write operations ──────────────────────────────────────────────

    /// Create a new fiber with an auto-assigned domain ID.
    ///
    /// Transition: Undefined → Defined.
    /// Appends a creation event to the line and registers the fiber.
    ///
    /// # Errors
    ///
    /// Returns an error if a migration is in progress, the event ID or
    /// domain ID counter overflows, or the line index exceeds capacity.
    pub fn create(
        &mut self,
        timestamp: i64,
        domain_event: T,
    ) -> Result<AppendResult, PardosaError> {
        self.reject_if_migrating()?;

        let domain_id = self.next_id;

        // Domain ID should be fresh — auto-assigned from monotonic counter.
        // Defensive check against impossible collisions.
        if self.lookup.contains_key(&domain_id) || self.purged_ids.contains(&domain_id) {
            return Err(PardosaError::IdAlreadyExists(domain_id));
        }

        // Validate state machine: Undefined → Create → Defined
        let new_state = transition(FiberState::Undefined, FiberAction::Create)?;

        // Pre-validate all computations before mutating
        let event_id = self.peek_event_id()?;
        let index = self.next_index()?;
        let next_domain_id = domain_id.checked_next()?;
        let fiber = Fiber::new(index, 1, index)?;

        // All checks passed — commit state changes
        let event = Event::new(
            event_id,
            timestamp,
            domain_id,
            false,
            Index::NONE,
            domain_event,
        );
        self.line.push(event);
        self.lookup.insert(domain_id, (fiber, new_state));
        self.next_event_id = event_id + 1;
        self.next_id = next_domain_id;

        Ok(AppendResult {
            domain_id,
            event_id,
            index,
        })
    }

    /// Create a fiber reusing a previously purged domain ID.
    ///
    /// Transition: Purged → Defined.
    /// The domain ID counter is NOT advanced — this reuses an existing ID.
    ///
    /// # Errors
    ///
    /// Returns an error if a migration is in progress, the domain ID is
    /// not in the purged set, or internal counters overflow.
    pub fn create_reuse(
        &mut self,
        domain_id: DomainId,
        timestamp: i64,
        domain_event: T,
    ) -> Result<AppendResult, PardosaError> {
        self.reject_if_migrating()?;

        if !self.purged_ids.contains(&domain_id) {
            return Err(PardosaError::IdNotPurged(domain_id));
        }

        // Validate state machine: Purged → Create → Defined
        let new_state = transition(FiberState::Purged, FiberAction::Create)?;

        let event_id = self.peek_event_id()?;
        let index = self.next_index()?;
        let fiber = Fiber::new(index, 1, index)?;

        let event = Event::new(
            event_id,
            timestamp,
            domain_id,
            false,
            Index::NONE,
            domain_event,
        );
        self.line.push(event);
        self.purged_ids.remove(&domain_id);
        self.lookup.insert(domain_id, (fiber, new_state));
        self.next_event_id = event_id + 1;

        Ok(AppendResult {
            domain_id,
            event_id,
            index,
        })
    }

    /// Append an update event to an existing fiber.
    ///
    /// Transition: Defined → Defined.
    ///
    /// # Errors
    ///
    /// Returns an error if a migration is in progress, the fiber is not
    /// found or not in the `Defined` state, or internal counters overflow.
    ///
    /// # Panics
    ///
    /// Panics if the fiber disappears from the lookup between the
    /// read-check and the mutable update — impossible under single-threaded
    /// access (the only supported mode).
    pub fn update(
        &mut self,
        domain_id: DomainId,
        timestamp: i64,
        domain_event: T,
    ) -> Result<AppendResult, PardosaError> {
        self.reject_if_migrating()?;

        let (fiber, state) = self
            .lookup
            .get(&domain_id)
            .ok_or(PardosaError::FiberNotFound(domain_id))?;

        let _new_state = transition(*state, FiberAction::Update)?;

        let event_id = self.peek_event_id()?;
        let index = self.next_index()?;
        let precursor = fiber.current();

        let event = Event::new(
            event_id,
            timestamp,
            domain_id,
            false,
            precursor,
            domain_event,
        );
        self.line.push(event);

        let (fiber, _) = self.lookup.get_mut(&domain_id).unwrap();
        fiber.advance(index)?;
        self.next_event_id = event_id + 1;

        Ok(AppendResult {
            domain_id,
            event_id,
            index,
        })
    }

    /// Soft-delete a fiber by appending a detach event.
    ///
    /// Transition: Defined → Detached.
    ///
    /// # Errors
    ///
    /// Returns an error if a migration is in progress, the fiber is not
    /// found or not in the `Defined` state, or internal counters overflow.
    ///
    /// # Panics
    ///
    /// Panics if the fiber disappears from the lookup between the
    /// read-check and the mutable update — impossible under single-threaded
    /// access.
    pub fn detach(
        &mut self,
        domain_id: DomainId,
        timestamp: i64,
        domain_event: T,
    ) -> Result<AppendResult, PardosaError> {
        self.reject_if_migrating()?;

        let (fiber, state) = self
            .lookup
            .get(&domain_id)
            .ok_or(PardosaError::FiberNotFound(domain_id))?;

        let new_state = transition(*state, FiberAction::Detach)?;

        let event_id = self.peek_event_id()?;
        let index = self.next_index()?;
        let precursor = fiber.current();

        let event = Event::new(
            event_id,
            timestamp,
            domain_id,
            true,
            precursor,
            domain_event,
        );
        self.line.push(event);

        let (fiber, state) = self.lookup.get_mut(&domain_id).unwrap();
        fiber.advance(index)?;
        *state = new_state;
        self.next_event_id = event_id + 1;

        Ok(AppendResult {
            domain_id,
            event_id,
            index,
        })
    }

    /// Rescue a detached or locked fiber.
    ///
    /// Transitions: Detached → Defined, Locked → Defined.
    ///
    /// For Locked fibers, history is lost — the new event starts with
    /// `precursor = Index::NONE` and the fiber is replaced with a fresh one.
    /// The `policy` parameter communicates whether the audit trail is preserved
    /// (old stream in grace period) or destroyed (old stream expired).
    ///
    /// For Detached fibers, `policy` is ignored — events remain in the
    /// current stream and the precursor chain continues.
    ///
    /// # Errors
    ///
    /// Returns an error if a migration is in progress, the fiber is not
    /// found, the transition is invalid, or internal counters overflow.
    ///
    /// # Panics
    ///
    /// Panics if the fiber disappears from the lookup between the
    /// read-check and the mutable update — impossible under single-threaded
    /// access.
    pub fn rescue(
        &mut self,
        domain_id: DomainId,
        _policy: LockedRescuePolicy,
        timestamp: i64,
        domain_event: T,
    ) -> Result<AppendResult, PardosaError> {
        self.reject_if_migrating()?;

        let (fiber, state) = self
            .lookup
            .get(&domain_id)
            .ok_or(PardosaError::FiberNotFound(domain_id))?;

        let current_state = *state;
        let new_state = transition(current_state, FiberAction::Rescue)?;

        let event_id = self.peek_event_id()?;
        let index = self.next_index()?;

        // Locked → Defined: fresh start, no precursor, new fiber.
        // Detached → Defined: continue the chain, advance existing fiber.
        let (precursor, replace_fiber) = match current_state {
            FiberState::Locked => (Index::NONE, Some(Fiber::new(index, 1, index)?)),
            FiberState::Detached => (fiber.current(), None),
            // transition() above only succeeds for Detached and Locked.
            // If the state machine gains new rescuable states, this arm
            // surfaces the gap as an explicit error rather than a panic.
            other => {
                return Err(PardosaError::InvalidTransition {
                    state: other,
                    action: FiberAction::Rescue,
                });
            }
        };

        let event = Event::new(
            event_id,
            timestamp,
            domain_id,
            false,
            precursor,
            domain_event,
        );
        self.line.push(event);

        let (fiber, state) = self.lookup.get_mut(&domain_id).unwrap();
        if let Some(new_fiber) = replace_fiber {
            *fiber = new_fiber;
        } else {
            fiber.advance(index)?;
        }
        *state = new_state;
        self.next_event_id = event_id + 1;

        Ok(AppendResult {
            domain_id,
            event_id,
            index,
        })
    }

    // ── Migration operations ──────────────────────────────────────────

    /// Apply a migration policy to a single fiber.
    ///
    /// Used by the migration lifecycle (Phase 4) and tests.
    /// Only valid for fibers in Detached or Locked state (per the state machine).
    ///
    /// - `Purge`: removes fiber from lookup, adds domain ID to `purged_ids`.
    /// - `LockAndPrune`: changes state to Locked (events remain in line;
    ///   actual pruning occurs during the new-stream migration pass in Phase 4).
    /// - `Keep`: state remains Detached.
    ///
    /// Not gated by `reject_if_migrating` — this IS a migration operation,
    /// invoked while the migration flag is active.
    ///
    /// # Errors
    ///
    /// Returns an error if the fiber is not found or the transition from
    /// the fiber's current state with the given policy is invalid.
    ///
    /// # Panics
    ///
    /// Panics if the fiber disappears from the lookup between the
    /// read-check and the mutable update — impossible under single-threaded
    /// access.
    pub fn migrate_fiber(
        &mut self,
        domain_id: DomainId,
        policy: MigrationPolicy,
    ) -> Result<(), PardosaError> {
        let (_, state) = self
            .lookup
            .get(&domain_id)
            .ok_or(PardosaError::FiberNotFound(domain_id))?;

        let new_state = transition(*state, FiberAction::Migrate(policy))?;

        match new_state {
            FiberState::Purged => {
                self.lookup.remove(&domain_id);
                self.purged_ids.insert(domain_id);
            }
            _ => {
                self.lookup.get_mut(&domain_id).unwrap().1 = new_state;
            }
        }

        Ok(())
    }

    /// Set the migration flag. When true, application writes are rejected.
    pub fn set_migrating(&mut self, migrating: bool) {
        self.migrating = migrating;
    }

    /// Returns true if a migration is in progress.
    #[must_use]
    pub fn is_migrating(&self) -> bool {
        self.migrating
    }

    // ── Read operations ───────────────────────────────────────────────

    /// Read the current (head) event of a Defined fiber.
    ///
    /// Returns `FiberNotFound` if the fiber doesn't exist or is not Defined.
    ///
    /// # Errors
    ///
    /// Returns [`PardosaError::FiberNotFound`] if the fiber doesn't exist
    /// or is not in the `Defined` state.
    pub fn read(&self, domain_id: DomainId) -> Result<&Event<T>, PardosaError> {
        let (fiber, state) = self
            .lookup
            .get(&domain_id)
            .ok_or(PardosaError::FiberNotFound(domain_id))?;

        if *state != FiberState::Defined {
            return Err(PardosaError::FiberNotFound(domain_id));
        }

        Ok(&self.line[fiber.current().as_usize()])
    }

    /// Read the current (head) event of a fiber, including soft-deleted fibers.
    ///
    /// Returns the event for Defined, Detached, and Locked fibers.
    /// Returns `FiberNotFound` if the fiber is Purged or doesn't exist.
    ///
    /// # Errors
    ///
    /// Returns [`PardosaError::FiberNotFound`] if the fiber is purged or
    /// doesn't exist.
    pub fn read_with_deleted(&self, domain_id: DomainId) -> Result<&Event<T>, PardosaError> {
        let (fiber, _) = self
            .lookup
            .get(&domain_id)
            .ok_or(PardosaError::FiberNotFound(domain_id))?;

        Ok(&self.line[fiber.current().as_usize()])
    }

    /// List all domain IDs with Defined state.
    ///
    /// Order is non-deterministic (`HashMap` iteration). Callers must not
    /// rely on stable ordering across calls.
    #[must_use]
    pub fn list(&self) -> Vec<DomainId> {
        self.lookup
            .iter()
            .filter(|(_, (_, state))| *state == FiberState::Defined)
            .map(|(id, _)| *id)
            .collect()
    }

    /// List all domain IDs that are not Purged (Defined + Detached + Locked).
    ///
    /// Order is non-deterministic (`HashMap` iteration). Callers must not
    /// rely on stable ordering across calls.
    #[must_use]
    pub fn list_with_deleted(&self) -> Vec<DomainId> {
        self.lookup.keys().copied().collect()
    }

    /// Return all events in a fiber's history, from oldest to newest.
    ///
    /// Walks the precursor chain from the head event backwards, then
    /// reverses to chronological order.
    ///
    /// Returns `FiberNotFound` if the fiber is Purged or doesn't exist.
    ///
    /// # Errors
    ///
    /// Returns [`PardosaError::FiberNotFound`] if the fiber is purged or
    /// doesn't exist.
    pub fn history(&self, domain_id: DomainId) -> Result<Vec<&Event<T>>, PardosaError> {
        let (fiber, _) = self
            .lookup
            .get(&domain_id)
            .ok_or(PardosaError::FiberNotFound(domain_id))?;

        let capacity = usize::try_from(fiber.len()).unwrap_or(usize::MAX);
        let mut events = Vec::with_capacity(capacity);
        let mut idx = fiber.current();

        while idx.is_some() {
            let event = &self.line[idx.as_usize()];
            events.push(event);
            idx = event.precursor();
        }

        events.reverse();
        Ok(events)
    }

    /// Return the entire line (all events in append order).
    #[must_use]
    pub fn read_line(&self) -> &[Event<T>] {
        &self.line
    }

    // ── Integrity ─────────────────────────────────────────────────────

    /// Verify all precursor chains are valid.
    ///
    /// Each event's precursor (when not `Index::NONE`) must:
    /// 1. Point to a valid earlier position in the line.
    /// 2. Reference an event with the same `domain_id`.
    ///
    /// O(n) time. Called on startup after replay.
    ///
    /// # Errors
    ///
    /// Returns [`PardosaError::BrokenPrecursorChain`] if any event's
    /// precursor references a forward position or a different domain ID.
    pub fn verify_precursor_chains(&self) -> Result<(), PardosaError> {
        for (i, event) in self.line.iter().enumerate() {
            let precursor = event.precursor();
            if precursor.is_none() {
                continue;
            }

            if precursor.as_usize() >= i {
                return Err(PardosaError::BrokenPrecursorChain {
                    event_id: event.event_id(),
                    precursor,
                });
            }

            let precursor_event = &self.line[precursor.as_usize()];
            if precursor_event.domain_id() != event.domain_id() {
                return Err(PardosaError::BrokenPrecursorChain {
                    event_id: event.event_id(),
                    precursor,
                });
            }
        }

        Ok(())
    }

    // ── Accessors ─────────────────────────────────────────────────────

    /// The next event ID that will be assigned.
    #[must_use]
    pub fn next_event_id(&self) -> u64 {
        self.next_event_id
    }

    /// The next domain ID that will be auto-assigned by `create()`.
    #[must_use]
    pub fn next_domain_id(&self) -> DomainId {
        self.next_id
    }

    /// Number of events in the line.
    #[must_use]
    pub fn line_len(&self) -> usize {
        self.line.len()
    }

    /// Resolve the state of a domain ID.
    ///
    /// Returns `Undefined` if the domain ID has never existed.
    /// Returns `Purged` if the domain ID was purged.
    /// Otherwise returns the current fiber state.
    #[must_use]
    pub fn fiber_state(&self, domain_id: DomainId) -> FiberState {
        if let Some((_, state)) = self.lookup.get(&domain_id) {
            *state
        } else if self.purged_ids.contains(&domain_id) {
            FiberState::Purged
        } else {
            FiberState::Undefined
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────

    fn reject_if_migrating(&self) -> Result<(), PardosaError> {
        if self.migrating {
            Err(PardosaError::MigrationInProgress)
        } else {
            Ok(())
        }
    }

    fn peek_event_id(&self) -> Result<u64, PardosaError> {
        if self.next_event_id == u64::MAX {
            Err(PardosaError::EventIdOverflow)
        } else {
            Ok(self.next_event_id)
        }
    }

    fn next_index(&self) -> Result<Index, PardosaError> {
        let len = u64::try_from(self.line.len()).map_err(|_| PardosaError::IndexOverflow)?;
        // u64::MAX is reserved for Index::NONE. Reject if next index would be the sentinel.
        if len == u64::MAX {
            return Err(PardosaError::IndexOverflow);
        }
        Ok(Index::new(len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fiber_state::MigrationPolicy;

    // ── Create ────────────────────────────────────────────────────────

    #[test]
    fn create_assigns_monotonic_event_id() {
        let mut d = Dragline::new();
        let r1 = d.create(1000, "first").unwrap();
        let r2 = d.create(1001, "second").unwrap();
        let r3 = d.create(1002, "third").unwrap();

        assert_eq!(r1.event_id, 0);
        assert_eq!(r2.event_id, 1);
        assert_eq!(r3.event_id, 2);
    }

    #[test]
    fn create_assigns_monotonic_domain_id() {
        let mut d = Dragline::new();
        let r1 = d.create(1000, "first").unwrap();
        let r2 = d.create(1001, "second").unwrap();

        assert_eq!(r1.domain_id, DomainId::new(0));
        assert_eq!(r2.domain_id, DomainId::new(1));
    }

    #[test]
    fn create_assigns_sequential_indices() {
        let mut d = Dragline::new();
        let r1 = d.create(1000, "first").unwrap();
        let r2 = d.create(1001, "second").unwrap();

        assert_eq!(r1.index, Index::new(0));
        assert_eq!(r2.index, Index::new(1));
    }

    #[test]
    fn create_sets_state_to_defined() {
        let mut d = Dragline::new();
        let r = d.create(1000, "first").unwrap();

        assert_eq!(d.fiber_state(r.domain_id), FiberState::Defined);
    }

    #[test]
    fn create_event_has_none_precursor() {
        let mut d = Dragline::new();
        let r = d.create(1000, "first").unwrap();
        let event = &d.read_line()[r.index.as_usize()];

        assert!(event.precursor().is_none());
    }

    // ── Create → Update → Detach lifecycle ────────────────────────────

    #[test]
    fn create_update_detach_lifecycle_event_ids() {
        let mut d = Dragline::new();

        let r1 = d.create(1000, "created").unwrap();
        let domain_id = r1.domain_id;

        let r2 = d.update(domain_id, 1001, "updated").unwrap();
        assert_eq!(d.fiber_state(domain_id), FiberState::Defined);

        let r3 = d.detach(domain_id, 1002, "detached").unwrap();
        assert_eq!(d.fiber_state(domain_id), FiberState::Detached);

        // All event_ids are monotonically increasing
        assert!(r1.event_id < r2.event_id);
        assert!(r2.event_id < r3.event_id);
    }

    #[test]
    fn update_sets_precursor_to_previous_event() {
        let mut d = Dragline::new();
        let r1 = d.create(1000, "created").unwrap();
        let r2 = d.update(r1.domain_id, 1001, "updated").unwrap();

        let event = &d.read_line()[r2.index.as_usize()];
        assert_eq!(event.precursor(), r1.index);
    }

    #[test]
    fn detach_sets_detached_flag() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();
        let r2 = d.detach(r.domain_id, 1001, "detached").unwrap();

        let event = &d.read_line()[r2.index.as_usize()];
        assert!(event.detached());
    }

    // ── Rescue ────────────────────────────────────────────────────────

    #[test]
    fn rescue_from_detached_continues_chain() {
        let mut d = Dragline::new();
        let r1 = d.create(1000, "created").unwrap();
        let r2 = d.detach(r1.domain_id, 1001, "detached").unwrap();

        let r3 = d
            .rescue(
                r1.domain_id,
                LockedRescuePolicy::PreserveAuditTrail,
                1002,
                "rescued",
            )
            .unwrap();

        assert_eq!(d.fiber_state(r1.domain_id), FiberState::Defined);

        // Precursor continues from the detach event
        let event = &d.read_line()[r3.index.as_usize()];
        assert_eq!(event.precursor(), r2.index);
        assert!(!event.detached());
    }

    #[test]
    fn rescue_from_locked_starts_fresh() {
        let mut d = Dragline::new();
        let r1 = d.create(1000, "created").unwrap();
        d.detach(r1.domain_id, 1001, "detached").unwrap();
        d.migrate_fiber(r1.domain_id, MigrationPolicy::LockAndPrune)
            .unwrap();

        assert_eq!(d.fiber_state(r1.domain_id), FiberState::Locked);

        let r3 = d
            .rescue(
                r1.domain_id,
                LockedRescuePolicy::AcceptDataLoss,
                1002,
                "rescued",
            )
            .unwrap();

        assert_eq!(d.fiber_state(r1.domain_id), FiberState::Defined);

        // Precursor is NONE — history lost
        let event = &d.read_line()[r3.index.as_usize()];
        assert!(event.precursor().is_none());
    }

    #[test]
    fn rescue_from_undefined_fails() {
        let mut d = Dragline::<&str>::new();
        let err = d
            .rescue(
                DomainId::new(99),
                LockedRescuePolicy::PreserveAuditTrail,
                1000,
                "nope",
            )
            .unwrap_err();

        assert!(
            matches!(err, PardosaError::FiberNotFound(_)),
            "expected FiberNotFound, got: {err}"
        );
    }

    // ── Purged-ID reuse ───────────────────────────────────────────────

    #[test]
    fn purged_id_reuse() {
        let mut d = Dragline::new();
        let r1 = d.create(1000, "created").unwrap();
        let domain_id = r1.domain_id;

        d.detach(domain_id, 1001, "detached").unwrap();
        d.migrate_fiber(domain_id, MigrationPolicy::Purge).unwrap();

        assert_eq!(d.fiber_state(domain_id), FiberState::Purged);

        let r2 = d.create_reuse(domain_id, 1002, "reused").unwrap();
        assert_eq!(r2.domain_id, domain_id);
        assert_eq!(d.fiber_state(domain_id), FiberState::Defined);

        // New fiber starts fresh
        let event = &d.read_line()[r2.index.as_usize()];
        assert!(event.precursor().is_none());
    }

    #[test]
    fn create_reuse_non_purged_fails() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();

        let err = d.create_reuse(r.domain_id, 1001, "nope").unwrap_err();
        assert!(
            matches!(err, PardosaError::IdNotPurged(_)),
            "expected IdNotPurged, got: {err}"
        );
    }

    #[test]
    fn create_reuse_unknown_id_fails() {
        let mut d = Dragline::<&str>::new();
        let err = d.create_reuse(DomainId::new(99), 1000, "nope").unwrap_err();

        assert!(
            matches!(err, PardosaError::IdNotPurged(_)),
            "expected IdNotPurged, got: {err}"
        );
    }

    #[test]
    fn purged_create_detach_purge_create_multi_cycle() {
        let mut d = Dragline::new();

        // Cycle 1: Create → Detach → Purge
        let r1 = d.create(1000, "c1").unwrap();
        let id = r1.domain_id;
        d.detach(id, 1001, "d1").unwrap();
        d.migrate_fiber(id, MigrationPolicy::Purge).unwrap();
        assert_eq!(d.fiber_state(id), FiberState::Purged);

        // Cycle 2: Reuse → Detach → Purge
        d.create_reuse(id, 1002, "c2").unwrap();
        d.detach(id, 1003, "d2").unwrap();
        d.migrate_fiber(id, MigrationPolicy::Purge).unwrap();
        assert_eq!(d.fiber_state(id), FiberState::Purged);

        // Cycle 3: Reuse again
        let r3 = d.create_reuse(id, 1004, "c3").unwrap();
        assert_eq!(d.fiber_state(id), FiberState::Defined);
        assert_eq!(r3.domain_id, id);
    }

    // ── Precursor chain verification ──────────────────────────────────

    #[test]
    fn verify_precursor_chains_valid() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();
        d.update(r.domain_id, 1001, "u1").unwrap();
        d.update(r.domain_id, 1002, "u2").unwrap();

        assert!(d.verify_precursor_chains().is_ok());
    }

    #[test]
    fn verify_precursor_chains_multi_fiber_valid() {
        let mut d = Dragline::new();

        let r1 = d.create(1000, "a-create").unwrap();
        let r2 = d.create(1001, "b-create").unwrap();

        d.update(r1.domain_id, 1002, "a-update").unwrap();
        d.update(r2.domain_id, 1003, "b-update").unwrap();
        d.update(r1.domain_id, 1004, "a-update2").unwrap();

        assert!(d.verify_precursor_chains().is_ok());
    }

    #[test]
    fn verify_precursor_chains_broken_wrong_domain_id() {
        let mut d = Dragline::new();
        d.create(1000, "a-create").unwrap();
        d.create(1001, "b-create").unwrap();

        // Manually inject event with precursor pointing to wrong domain_id.
        // Event at index 0 is domain_id=0, so this event (domain_id=99)
        // with precursor=0 has a cross-domain precursor — broken chain.
        let bad_event = Event::new(99, 2000, DomainId::new(99), false, Index::new(0), "broken");
        d.line.push(bad_event);

        let err = d.verify_precursor_chains().unwrap_err();
        assert!(
            matches!(err, PardosaError::BrokenPrecursorChain { .. }),
            "expected BrokenPrecursorChain, got: {err}"
        );
    }

    #[test]
    fn verify_precursor_chains_broken_forward_reference() {
        let mut d = Dragline::new();
        d.create(1000, "created").unwrap();

        // Precursor points forward (index 5 > current position 1) — broken
        let bad_event = Event::new(99, 2000, DomainId::new(0), false, Index::new(5), "broken");
        d.line.push(bad_event);

        let err = d.verify_precursor_chains().unwrap_err();
        assert!(
            matches!(err, PardosaError::BrokenPrecursorChain { .. }),
            "expected BrokenPrecursorChain, got: {err}"
        );
    }

    #[test]
    fn verify_precursor_chains_broken_self_reference() {
        let mut d = Dragline::new();
        d.create(1000, "created").unwrap();

        // Precursor points to self (index 1 at position 1) — broken
        let bad_event = Event::new(99, 2000, DomainId::new(0), false, Index::new(1), "broken");
        d.line.push(bad_event);

        let err = d.verify_precursor_chains().unwrap_err();
        assert!(
            matches!(err, PardosaError::BrokenPrecursorChain { .. }),
            "expected BrokenPrecursorChain, got: {err}"
        );
    }

    // ── Read operations ───────────────────────────────────────────────

    #[test]
    fn read_defined_fiber() {
        let mut d = Dragline::new();
        let r = d.create(1000, "hello").unwrap();

        let event = d.read(r.domain_id).unwrap();
        assert_eq!(*event.domain_event(), "hello");
        assert_eq!(event.event_id(), r.event_id);
    }

    #[test]
    fn read_returns_latest_event() {
        let mut d = Dragline::new();
        let r = d.create(1000, "v1").unwrap();
        d.update(r.domain_id, 1001, "v2").unwrap();
        d.update(r.domain_id, 1002, "v3").unwrap();

        let event = d.read(r.domain_id).unwrap();
        assert_eq!(*event.domain_event(), "v3");
    }

    #[test]
    fn read_detached_fiber_fails() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();
        d.detach(r.domain_id, 1001, "detached").unwrap();

        assert!(d.read(r.domain_id).is_err());
    }

    #[test]
    fn read_unknown_domain_id_fails() {
        let d = Dragline::<&str>::new();
        assert!(d.read(DomainId::new(0)).is_err());
    }

    #[test]
    fn read_with_deleted_returns_detached() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();
        d.detach(r.domain_id, 1001, "detached").unwrap();

        let event = d.read_with_deleted(r.domain_id).unwrap();
        assert!(event.detached());
        assert_eq!(*event.domain_event(), "detached");
    }

    #[test]
    fn read_with_deleted_returns_locked() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();
        d.detach(r.domain_id, 1001, "detached").unwrap();
        d.migrate_fiber(r.domain_id, MigrationPolicy::LockAndPrune)
            .unwrap();

        let event = d.read_with_deleted(r.domain_id).unwrap();
        assert!(event.detached());
    }

    #[test]
    fn read_with_deleted_purged_fails() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();
        d.detach(r.domain_id, 1001, "detached").unwrap();
        d.migrate_fiber(r.domain_id, MigrationPolicy::Purge)
            .unwrap();

        assert!(d.read_with_deleted(r.domain_id).is_err());
    }

    // ── List operations ───────────────────────────────────────────────

    #[test]
    fn list_only_defined() {
        let mut d = Dragline::new();
        let r1 = d.create(1000, "a").unwrap();
        let r2 = d.create(1001, "b").unwrap();
        let _r3 = d.create(1002, "c").unwrap();

        d.detach(r2.domain_id, 1003, "detached").unwrap();

        let listed = d.list();
        assert_eq!(listed.len(), 2);
        assert!(listed.contains(&r1.domain_id));
        assert!(!listed.contains(&r2.domain_id));
    }

    #[test]
    fn list_with_deleted_includes_detached_and_locked() {
        let mut d = Dragline::new();
        let r1 = d.create(1000, "a").unwrap();
        let r2 = d.create(1001, "b").unwrap();
        let r3 = d.create(1002, "c").unwrap();

        d.detach(r2.domain_id, 1003, "detached-b").unwrap();
        d.detach(r3.domain_id, 1004, "detached-c").unwrap();
        d.migrate_fiber(r3.domain_id, MigrationPolicy::LockAndPrune)
            .unwrap();

        let listed = d.list_with_deleted();
        assert_eq!(listed.len(), 3);
        assert!(listed.contains(&r1.domain_id));
        assert!(listed.contains(&r2.domain_id));
        assert!(listed.contains(&r3.domain_id));
    }

    #[test]
    fn list_with_deleted_excludes_purged() {
        let mut d = Dragline::new();
        let r1 = d.create(1000, "a").unwrap();
        let r2 = d.create(1001, "b").unwrap();

        d.detach(r2.domain_id, 1002, "detached").unwrap();
        d.migrate_fiber(r2.domain_id, MigrationPolicy::Purge)
            .unwrap();

        let listed = d.list_with_deleted();
        assert_eq!(listed.len(), 1);
        assert!(listed.contains(&r1.domain_id));
    }

    #[test]
    fn list_empty_dragline() {
        let d = Dragline::<&str>::new();
        assert!(d.list().is_empty());
        assert!(d.list_with_deleted().is_empty());
    }

    // ── History ───────────────────────────────────────────────────────

    #[test]
    fn history_returns_chronological_order() {
        let mut d = Dragline::new();
        let r = d.create(1000, "v1").unwrap();
        d.update(r.domain_id, 1001, "v2").unwrap();
        d.update(r.domain_id, 1002, "v3").unwrap();

        let hist = d.history(r.domain_id).unwrap();
        assert_eq!(hist.len(), 3);
        assert_eq!(*hist[0].domain_event(), "v1");
        assert_eq!(*hist[1].domain_event(), "v2");
        assert_eq!(*hist[2].domain_event(), "v3");
    }

    #[test]
    fn history_single_event() {
        let mut d = Dragline::new();
        let r = d.create(1000, "only").unwrap();

        let hist = d.history(r.domain_id).unwrap();
        assert_eq!(hist.len(), 1);
        assert_eq!(*hist[0].domain_event(), "only");
    }

    #[test]
    fn history_includes_detach_event() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();
        d.update(r.domain_id, 1001, "updated").unwrap();
        d.detach(r.domain_id, 1002, "detached").unwrap();

        let hist = d.history(r.domain_id).unwrap();
        assert_eq!(hist.len(), 3);
        assert!(hist[2].detached());
    }

    #[test]
    fn history_after_rescue_from_locked_shows_only_new_event() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();
        d.update(r.domain_id, 1001, "updated").unwrap();
        d.detach(r.domain_id, 1002, "detached").unwrap();
        d.migrate_fiber(r.domain_id, MigrationPolicy::LockAndPrune)
            .unwrap();

        d.rescue(
            r.domain_id,
            LockedRescuePolicy::AcceptDataLoss,
            1003,
            "rescued",
        )
        .unwrap();

        let hist = d.history(r.domain_id).unwrap();
        assert_eq!(hist.len(), 1);
        assert_eq!(*hist[0].domain_event(), "rescued");
    }

    #[test]
    fn history_purged_fiber_fails() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();
        d.detach(r.domain_id, 1001, "detached").unwrap();
        d.migrate_fiber(r.domain_id, MigrationPolicy::Purge)
            .unwrap();

        assert!(d.history(r.domain_id).is_err());
    }

    // ── Read line ─────────────────────────────────────────────────────

    #[test]
    fn read_line_returns_all_events() {
        let mut d = Dragline::new();
        let r1 = d.create(1000, "a").unwrap();
        let r2 = d.create(1001, "b").unwrap();
        d.update(r1.domain_id, 1002, "a-update").unwrap();

        let line = d.read_line();
        assert_eq!(line.len(), 3);
        assert_eq!(line[0].domain_id(), r1.domain_id);
        assert_eq!(line[1].domain_id(), r2.domain_id);
        assert_eq!(line[2].domain_id(), r1.domain_id);
    }

    // ── Migration flag ────────────────────────────────────────────────

    #[test]
    fn migration_in_progress_rejects_create() {
        let mut d = Dragline::new();
        d.set_migrating(true);

        assert!(matches!(
            d.create(1000, "should fail"),
            Err(PardosaError::MigrationInProgress)
        ));
    }

    #[test]
    fn migration_in_progress_rejects_update() {
        let mut d = Dragline::new();
        let r = d.create(1000, "ok").unwrap();
        d.set_migrating(true);

        assert!(matches!(
            d.update(r.domain_id, 1001, "should fail"),
            Err(PardosaError::MigrationInProgress)
        ));
    }

    #[test]
    fn migration_in_progress_rejects_detach() {
        let mut d = Dragline::new();
        let r = d.create(1000, "ok").unwrap();
        d.set_migrating(true);

        assert!(matches!(
            d.detach(r.domain_id, 1001, "should fail"),
            Err(PardosaError::MigrationInProgress)
        ));
    }

    #[test]
    fn migration_in_progress_rejects_rescue() {
        let mut d = Dragline::new();
        let r = d.create(1000, "ok").unwrap();
        d.detach(r.domain_id, 1001, "detach").unwrap();
        d.set_migrating(true);

        assert!(matches!(
            d.rescue(
                r.domain_id,
                LockedRescuePolicy::PreserveAuditTrail,
                1002,
                "should fail"
            ),
            Err(PardosaError::MigrationInProgress)
        ));
    }

    #[test]
    fn migration_in_progress_rejects_create_reuse() {
        let mut d = Dragline::new();
        let r = d.create(1000, "ok").unwrap();
        d.detach(r.domain_id, 1001, "detach").unwrap();
        d.migrate_fiber(r.domain_id, MigrationPolicy::Purge)
            .unwrap();
        d.set_migrating(true);

        assert!(matches!(
            d.create_reuse(r.domain_id, 1002, "should fail"),
            Err(PardosaError::MigrationInProgress)
        ));
    }

    #[test]
    fn reads_work_during_migration() {
        let mut d = Dragline::new();
        let r = d.create(1000, "ok").unwrap();
        d.set_migrating(true);

        // Reads should still work
        assert!(d.read(r.domain_id).is_ok());
        assert!(!d.list().is_empty());
        assert!(!d.list_with_deleted().is_empty());
        assert!(d.history(r.domain_id).is_ok());
        assert!(!d.read_line().is_empty());
    }

    // ── Invalid transitions ───────────────────────────────────────────

    #[test]
    fn update_on_detached_fails() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();
        d.detach(r.domain_id, 1001, "detached").unwrap();

        assert!(matches!(
            d.update(r.domain_id, 1002, "nope"),
            Err(PardosaError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn detach_on_detached_fails() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();
        d.detach(r.domain_id, 1001, "detached").unwrap();

        assert!(matches!(
            d.detach(r.domain_id, 1002, "nope"),
            Err(PardosaError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn update_on_unknown_fails() {
        let mut d = Dragline::<&str>::new();
        assert!(matches!(
            d.update(DomainId::new(0), 1000, "nope"),
            Err(PardosaError::FiberNotFound(_))
        ));
    }

    // ── Overflow tests ────────────────────────────────────────────────

    #[test]
    fn event_id_overflow() {
        let mut d = Dragline::new();
        d.next_event_id = u64::MAX;

        assert!(matches!(
            d.create(1000, "overflow"),
            Err(PardosaError::EventIdOverflow)
        ));
    }

    #[test]
    fn domain_id_overflow() {
        let mut d = Dragline::new();
        d.next_id = DomainId::new(u64::MAX);

        // peek_event_id succeeds, next_index succeeds, but
        // domain_id.checked_next() overflows
        assert!(matches!(
            d.create(1000, "overflow"),
            Err(PardosaError::DomainIdOverflow)
        ));
    }

    // ── Migrate fiber ─────────────────────────────────────────────────

    #[test]
    fn migrate_keep_preserves_detached() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();
        d.detach(r.domain_id, 1001, "detached").unwrap();
        d.migrate_fiber(r.domain_id, MigrationPolicy::Keep).unwrap();

        assert_eq!(d.fiber_state(r.domain_id), FiberState::Detached);
    }

    #[test]
    fn migrate_purge_removes_from_lookup() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();
        d.detach(r.domain_id, 1001, "detached").unwrap();
        d.migrate_fiber(r.domain_id, MigrationPolicy::Purge)
            .unwrap();

        assert_eq!(d.fiber_state(r.domain_id), FiberState::Purged);
    }

    #[test]
    fn migrate_lock_and_prune_sets_locked() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();
        d.detach(r.domain_id, 1001, "detached").unwrap();
        d.migrate_fiber(r.domain_id, MigrationPolicy::LockAndPrune)
            .unwrap();

        assert_eq!(d.fiber_state(r.domain_id), FiberState::Locked);
    }

    #[test]
    fn migrate_defined_fiber_fails() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();

        assert!(matches!(
            d.migrate_fiber(r.domain_id, MigrationPolicy::Keep),
            Err(PardosaError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn migrate_locked_purge_escalation() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();
        d.detach(r.domain_id, 1001, "detached").unwrap();
        d.migrate_fiber(r.domain_id, MigrationPolicy::LockAndPrune)
            .unwrap();

        // Locked → Migrate(Purge) → Purged (escalation)
        d.migrate_fiber(r.domain_id, MigrationPolicy::Purge)
            .unwrap();
        assert_eq!(d.fiber_state(r.domain_id), FiberState::Purged);
    }

    // ── Accessors ─────────────────────────────────────────────────────

    #[test]
    fn default_creates_empty_dragline() {
        let d = Dragline::<String>::default();
        assert_eq!(d.line_len(), 0);
        assert_eq!(d.next_event_id(), 0);
        assert_eq!(d.next_domain_id(), DomainId::new(0));
        assert!(!d.is_migrating());
    }

    #[test]
    fn fiber_state_reports_undefined() {
        let d = Dragline::<&str>::new();
        assert_eq!(d.fiber_state(DomainId::new(0)), FiberState::Undefined);
    }

    // ── Additional coverage (rigormortis findings) ────────────────────

    #[test]
    fn read_with_deleted_on_defined_fiber() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();

        let event = d.read_with_deleted(r.domain_id).unwrap();
        assert_eq!(*event.domain_event(), "created");
    }

    #[test]
    fn history_through_detach_and_rescue() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();
        d.update(r.domain_id, 1001, "updated").unwrap();
        d.detach(r.domain_id, 1002, "detached").unwrap();

        d.rescue(
            r.domain_id,
            LockedRescuePolicy::PreserveAuditTrail,
            1003,
            "rescued",
        )
        .unwrap();
        d.update(r.domain_id, 1004, "post-rescue").unwrap();

        let hist = d.history(r.domain_id).unwrap();
        // Full chain: created → updated → detached → rescued → post-rescue
        assert_eq!(hist.len(), 5);
        assert_eq!(*hist[0].domain_event(), "created");
        assert_eq!(*hist[3].domain_event(), "rescued");
        assert_eq!(*hist[4].domain_event(), "post-rescue");
    }

    #[test]
    fn migrate_fiber_unknown_domain_id_fails() {
        let mut d = Dragline::<&str>::new();

        assert!(matches!(
            d.migrate_fiber(DomainId::new(99), MigrationPolicy::Purge),
            Err(PardosaError::FiberNotFound(_))
        ));
    }

    #[test]
    fn migrate_fiber_purged_domain_id_fails() {
        let mut d = Dragline::new();
        let r = d.create(1000, "created").unwrap();
        d.detach(r.domain_id, 1001, "detached").unwrap();
        d.migrate_fiber(r.domain_id, MigrationPolicy::Purge)
            .unwrap();

        // Already purged — not in lookup anymore
        assert!(matches!(
            d.migrate_fiber(r.domain_id, MigrationPolicy::Purge),
            Err(PardosaError::FiberNotFound(_))
        ));
    }

    // ── proptest ──────────────────────────────────────────────────────

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        #[derive(Debug, Clone)]
        enum TestAction {
            Create,
            UpdateFirst,
            DetachFirst,
            RescueFirst,
            MigrateFirstPurge,
            MigrateFirstLockAndPrune,
            CreateReusePurged,
        }

        fn arb_action() -> impl Strategy<Value = TestAction> {
            prop_oneof![
                3 => Just(TestAction::Create),
                2 => Just(TestAction::UpdateFirst),
                1 => Just(TestAction::DetachFirst),
                1 => Just(TestAction::RescueFirst),
                1 => Just(TestAction::MigrateFirstPurge),
                1 => Just(TestAction::MigrateFirstLockAndPrune),
                1 => Just(TestAction::CreateReusePurged),
            ]
        }

        proptest! {
            #[test]
            fn arbitrary_sequences_preserve_precursor_chains(
                actions in prop::collection::vec(arb_action(), 1..100)
            ) {
                let mut d = Dragline::<String>::new();
                let mut defined: Vec<DomainId> = Vec::new();
                let mut detached: Vec<DomainId> = Vec::new();
                let mut locked: Vec<DomainId> = Vec::new();
                let mut purged: Vec<DomainId> = Vec::new();
                let mut ts = 0i64;

                for action in &actions {
                    ts += 1;
                    match action {
                        TestAction::Create => {
                            let r = d.create(ts, format!("c{ts}")).unwrap();
                            defined.push(r.domain_id);
                        }
                        TestAction::UpdateFirst => {
                            if let Some(&id) = defined.first() {
                                let _ = d.update(id, ts, format!("u{ts}"));
                            }
                        }
                        TestAction::DetachFirst => {
                            if let Some(id) = defined.pop() {
                                if d.detach(id, ts, format!("d{ts}")).is_ok() {
                                    detached.push(id);
                                } else {
                                    defined.push(id);
                                }
                            }
                        }
                        TestAction::RescueFirst => {
                            if let Some(id) = detached.pop() {
                                if d.rescue(id, LockedRescuePolicy::PreserveAuditTrail, ts, format!("r{ts}")).is_ok() {
                                    defined.push(id);
                                } else {
                                    detached.push(id);
                                }
                            } else if let Some(id) = locked.pop() {
                                if d.rescue(id, LockedRescuePolicy::AcceptDataLoss, ts, format!("r{ts}")).is_ok() {
                                    defined.push(id);
                                } else {
                                    locked.push(id);
                                }
                            }
                        }
                        TestAction::MigrateFirstPurge => {
                            if let Some(id) = detached.pop() {
                                if d.migrate_fiber(id, MigrationPolicy::Purge).is_ok() {
                                    purged.push(id);
                                } else {
                                    detached.push(id);
                                }
                            } else if let Some(id) = locked.pop() {
                                if d.migrate_fiber(id, MigrationPolicy::Purge).is_ok() {
                                    purged.push(id);
                                } else {
                                    locked.push(id);
                                }
                            }
                        }
                        TestAction::MigrateFirstLockAndPrune => {
                            if let Some(id) = detached.pop() {
                                if d.migrate_fiber(id, MigrationPolicy::LockAndPrune).is_ok() {
                                    locked.push(id);
                                } else {
                                    detached.push(id);
                                }
                            }
                        }
                        TestAction::CreateReusePurged => {
                            if let Some(id) = purged.pop() {
                                if d.create_reuse(id, ts, format!("reuse{ts}")).is_ok() {
                                    defined.push(id);
                                } else {
                                    purged.push(id);
                                }
                            }
                        }
                    }
                }

                // Core invariant: precursor chains valid after all operations
                prop_assert!(d.verify_precursor_chains().is_ok());

                // event_id should equal number of events in line
                prop_assert_eq!(usize::try_from(d.next_event_id()).unwrap(), d.line_len());

                // Every event_id in the line is unique and sequential
                for (i, event) in d.read_line().iter().enumerate() {
                    prop_assert_eq!(event.event_id(), u64::try_from(i).unwrap());
                }
            }

            #[test]
            fn monotonic_event_ids_across_creates(count in 1..50usize) {
                let mut d = Dragline::<String>::new();
                let mut prev_event_id = None;

                for i in 0..count {
                    let r = d.create(i64::try_from(i).unwrap(), format!("e{i}")).unwrap();

                    if let Some(prev) = prev_event_id {
                        prop_assert!(r.event_id > prev, "event_id not monotonic: {} <= {}", r.event_id, prev);
                    }
                    prev_event_id = Some(r.event_id);
                }
            }
        }
    }
}
