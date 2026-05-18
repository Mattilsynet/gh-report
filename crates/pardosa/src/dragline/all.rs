use std::collections::{HashMap, HashSet};

use super::linevec::Linevec;
use crate::error::PardosaError;
use crate::event::{DomainId, Event, Index};
use crate::fiber::Fiber;
// F2b: precursor-hash chain wiring per PAR-0021:R5. `to_vec` encodes a
// predecessor `Event<T>` via its `impl<T: Encode> Encode for Event<T>`
// (event.rs:305); `precursor_hash_of` is BLAKE3 of those bytes. P-include
// semantics: `precursor_hash` IS part of the encoded surface so the chain
// covers the prior event's full shape (event.rs:302).
use crate::fiber_state::{
    FiberAction, FiberState, LockedRescuePolicy, MigrationPolicy, transition,
};
use pardosa_encoding::{Encode, precursor_hash_of, to_vec};

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
/// - `next_id` auto-increments for new fiber creation. [`Dragline::create`]
///   advances it past any ids in `purged_ids` so the auto-assignment contract
///   does not stall when on-disk state lands `next_id` on a purged id.
/// - When `migrating` is true, application writes are rejected.
#[derive(Debug)]
pub struct Dragline<T> {
    line: Linevec<T>,
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

/// Description of a prepared, infallible mutation produced by the
/// `prepare` phase of `Dragline::commit_atomic` (FH5). Every field
/// here has had its fallibility discharged in `prepare`; `apply` only
/// performs operations whose invariants were checked upstream.
struct PreparedCommit<T> {
    event: Event<T>,
    event_id: u64,
    index: Index,
    domain_id: DomainId,
    lookup_op: LookupOp,
    /// Some when this writer advances the domain-id counter (create).
    next_id_advance: Option<DomainId>,
    /// Some when this writer clears a purged-id reservation (`create_reuse`).
    purged_remove: Option<DomainId>,
}

/// Shape of the lookup-table mutation a writer performs. Each variant
/// corresponds to one of the legal write patterns; encoding them as an
/// enum prevents writers from mixing patterns or forgetting steps.
enum LookupOp {
    /// Fresh fiber under a new domain id. Used by `create` and
    /// `create_reuse`.
    Insert { fiber: Fiber, state: FiberState },
    /// Advance an existing fiber's `current` index. `new_state` is Some
    /// when the writer also transitions the fiber state (detach,
    /// rescue Detached → Defined); None for plain `update`.
    ///
    /// `new_current` MUST have been validated via `Fiber::check_advance`
    /// in prepare; `apply` invokes `Fiber::advance_unchecked`.
    AdvanceFiber {
        new_current: Index,
        new_state: Option<FiberState>,
    },
    /// Replace the existing fiber wholesale (rescue Locked → Defined,
    /// where history is destroyed and a fresh fiber starts).
    ReplaceFiber { fiber: Fiber, new_state: FiberState },
}

impl<T> Dragline<T> {
    /// Create a new empty dragline.
    #[must_use]
    pub fn new() -> Self {
        Dragline {
            line: Linevec::new(),
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
    /// # Domain-ID skipping
    ///
    /// Auto-assignment advances `next_id` past any ids in `purged_ids`
    /// transparently. The on-disk format (and migration replays) can put
    /// `next_id` on top of a previously-purged id; pre-FH4 this raised
    /// `IdAlreadyExists` and herded the caller into using `create_reuse`
    /// with a specific id, breaking the auto-create contract. The skip
    /// loop is bounded by `u64::MAX`; if every remaining id is purged
    /// the call fails with `DomainIdOverflow` (no liveness corner).
    ///
    /// # Errors
    ///
    /// Returns an error if a migration is in progress, the event ID or
    /// domain ID counter overflows (including when skipping past purged
    /// ids exhausts the `u64` space), or the line index exceeds capacity.
    pub fn create(
        &mut self,
        timestamp: i64,
        domain_event: T,
    ) -> Result<AppendResult, PardosaError> {
        self.commit_atomic(|s| {
            // Advance next_id past any purged ids. `purged_ids` is a HashSet
            // so each membership check is O(1); the loop terminates either
            // when we find a fresh id (the common case: zero iterations) or
            // when checked_next overflows u64 (every remaining id is purged).
            let mut domain_id = s.next_id;
            while s.purged_ids.contains(&domain_id) {
                domain_id = domain_id.checked_next()?;
            }

            // After skipping, the id may now collide with an active fiber in
            // lookup. That would indicate a corrupted on-disk state (lookup
            // and next_id disagree) and is structurally impossible from the
            // public API alone — keep the defensive check.
            if s.lookup.contains_key(&domain_id) {
                return Err(PardosaError::IdAlreadyExists(domain_id));
            }

            let new_state = transition(FiberState::Undefined, FiberAction::Create)?;
            let event_id = s.peek_event_id()?;
            let index = s.next_index()?;
            let next_domain_id = domain_id.checked_next()?;
            let fiber = Fiber::new(index, 1, index)?;

            let event = Event::new(
                event_id,
                timestamp,
                domain_id,
                false,
                Index::NONE,
                // F2a: genesis event has no predecessor; zero hash is canonical.
                [0u8; 32],
                domain_event,
            );
            Ok(PreparedCommit {
                event,
                event_id,
                index,
                domain_id,
                lookup_op: LookupOp::Insert {
                    fiber,
                    state: new_state,
                },
                next_id_advance: Some(next_domain_id),
                purged_remove: None,
            })
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
        self.commit_atomic(|s| {
            if !s.purged_ids.contains(&domain_id) {
                return Err(PardosaError::IdNotPurged(domain_id));
            }

            let new_state = transition(FiberState::Purged, FiberAction::Create)?;
            let event_id = s.peek_event_id()?;
            let index = s.next_index()?;
            let fiber = Fiber::new(index, 1, index)?;

            let event = Event::new(
                event_id,
                timestamp,
                domain_id,
                false,
                Index::NONE,
                // F2a: genesis event has no predecessor; zero hash is canonical.
                [0u8; 32],
                domain_event,
            );
            Ok(PreparedCommit {
                event,
                event_id,
                index,
                domain_id,
                lookup_op: LookupOp::Insert {
                    fiber,
                    state: new_state,
                },
                next_id_advance: None,
                purged_remove: Some(domain_id),
            })
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
    pub fn update(
        &mut self,
        domain_id: DomainId,
        timestamp: i64,
        domain_event: T,
    ) -> Result<AppendResult, PardosaError>
    where
        T: Encode,
    {
        self.commit_atomic(|s| {
            let (fiber, state) = s
                .lookup
                .get(&domain_id)
                .ok_or(PardosaError::FiberNotFound(domain_id))?;

            let _new_state = transition(*state, FiberAction::Update)?;

            let event_id = s.peek_event_id()?;
            let index = s.next_index()?;
            let precursor = fiber.current();

            // Lift Fiber::advance fallibility into prepare (FH5: closes
            // the latent partial-commit window where line.append_validated
            // succeeded then fiber.advance Err'd on len overflow).
            fiber.check_advance(index)?;

            let event = Event::new(
                event_id,
                timestamp,
                domain_id,
                false,
                precursor,
                // F2b: BLAKE3 of canonical bytes of predecessor at `precursor`,
                // P-include semantics (event.rs:302). Genesis events have
                // `precursor == Index::NONE` and carry the zero hash.
                if precursor.is_none() {
                    [0u8; 32]
                } else {
                    precursor_hash_of(&to_vec(&s.line[precursor.as_usize()]))
                },
                domain_event,
            );
            Ok(PreparedCommit {
                event,
                event_id,
                index,
                domain_id,
                lookup_op: LookupOp::AdvanceFiber {
                    new_current: index,
                    new_state: None,
                },
                next_id_advance: None,
                purged_remove: None,
            })
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
    pub fn detach(
        &mut self,
        domain_id: DomainId,
        timestamp: i64,
        domain_event: T,
    ) -> Result<AppendResult, PardosaError>
    where
        T: Encode,
    {
        self.commit_atomic(|s| {
            let (fiber, state) = s
                .lookup
                .get(&domain_id)
                .ok_or(PardosaError::FiberNotFound(domain_id))?;

            let new_state = transition(*state, FiberAction::Detach)?;

            let event_id = s.peek_event_id()?;
            let index = s.next_index()?;
            let precursor = fiber.current();

            // FH5: lift Fiber::advance fallibility into prepare.
            fiber.check_advance(index)?;

            let event = Event::new(
                event_id,
                timestamp,
                domain_id,
                true,
                precursor,
                // F2b: BLAKE3 of canonical bytes of predecessor at `precursor`,
                // P-include semantics (event.rs:302). Genesis events have
                // `precursor == Index::NONE` and carry the zero hash.
                if precursor.is_none() {
                    [0u8; 32]
                } else {
                    precursor_hash_of(&to_vec(&s.line[precursor.as_usize()]))
                },
                domain_event,
            );
            Ok(PreparedCommit {
                event,
                event_id,
                index,
                domain_id,
                lookup_op: LookupOp::AdvanceFiber {
                    new_current: index,
                    new_state: Some(new_state),
                },
                next_id_advance: None,
                purged_remove: None,
            })
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
    ) -> Result<AppendResult, PardosaError>
    where
        T: Encode,
    {
        self.commit_atomic(|s| {
            let (fiber, state) = s
                .lookup
                .get(&domain_id)
                .ok_or(PardosaError::FiberNotFound(domain_id))?;

            let current_state = *state;
            let new_state = transition(current_state, FiberAction::Rescue)?;

            let event_id = s.peek_event_id()?;
            let index = s.next_index()?;

            // Locked → Defined: fresh start, no precursor, new fiber.
            // Detached → Defined: continue the chain, advance existing fiber.
            let (precursor, lookup_op) = match current_state {
                FiberState::Locked => {
                    let new_fiber = Fiber::new(index, 1, index)?;
                    (
                        Index::NONE,
                        LookupOp::ReplaceFiber {
                            fiber: new_fiber,
                            new_state,
                        },
                    )
                }
                FiberState::Detached => {
                    // FH5: lift Fiber::advance fallibility into prepare.
                    fiber.check_advance(index)?;
                    (
                        fiber.current(),
                        LookupOp::AdvanceFiber {
                            new_current: index,
                            new_state: Some(new_state),
                        },
                    )
                }
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
                // F2b: BLAKE3 of canonical bytes of predecessor at `precursor`,
                // P-include semantics (event.rs:302). Genesis events have
                // `precursor == Index::NONE` and carry the zero hash.
                if precursor.is_none() {
                    [0u8; 32]
                } else {
                    precursor_hash_of(&to_vec(&s.line[precursor.as_usize()]))
                },
                domain_event,
            );
            Ok(PreparedCommit {
                event,
                event_id,
                index,
                domain_id,
                lookup_op,
                next_id_advance: None,
                purged_remove: None,
            })
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
        self.line.as_slice()
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
    /// Verify all invariants documented on the struct (superset of
    /// [`verify_precursor_chains`](Self::verify_precursor_chains)).
    ///
    /// Intended for persistence-boundary call-sites: any constructor that
    /// reassembles a `Dragline` from external bytes (replay, migration,
    /// snapshot restore) must call this before exposing the value. The
    /// existing `verify_precursor_chains` only covers structural per-event
    /// invariants; this superset also asserts cross-event and bookkeeping
    /// invariants that PAR-0007 mandates.
    ///
    /// Checks (in addition to [`verify_precursor_chains`](Self::verify_precursor_chains)):
    /// - event-id strictly monotonic across `line`,
    /// - `purged_ids` and `lookup.keys()` are disjoint,
    /// - `next_event_id == last_event_id + 1` (or `0` when `line` is empty),
    /// - every fiber's `current()` index is `< line.len()`.
    ///
    /// O(n) time. Rejections surface as [`PardosaError::FiberInvariantViolation`]
    /// with a human-readable description; precursor-chain breakage retains its
    /// original [`PardosaError::BrokenPrecursorChain`] variant for callers that
    /// already match on it.
    ///
    /// # Errors
    ///
    /// Returns any [`PardosaError`] produced by [`verify_precursor_chains`](Self::verify_precursor_chains),
    /// or a [`PardosaError::FiberInvariantViolation`] when one of the
    /// cross-event / bookkeeping invariants fails.
    pub fn verify_invariants(&self) -> Result<(), PardosaError>
    where
        T: Encode,
    {
        self.verify_precursor_chains()?;

        // (a) event-id strictly monotonic across line
        for pair in self.line.windows(2) {
            if pair[0].event_id() >= pair[1].event_id() {
                return Err(PardosaError::FiberInvariantViolation(format!(
                    "event_id not monotonic: line[i]={} >= line[i+1]={}",
                    pair[0].event_id(),
                    pair[1].event_id(),
                )));
            }
        }

        // (b) purged_ids ∩ lookup.keys() == ∅
        for purged in &self.purged_ids {
            if self.lookup.contains_key(purged) {
                return Err(PardosaError::FiberInvariantViolation(format!(
                    "domain_id {purged:?} is both purged and present in lookup",
                )));
            }
        }

        // (c) next_event_id == last_event_id + 1 (or 0 if empty)
        let expected_next = match self.line.last() {
            None => 0,
            Some(last) => last.event_id().checked_add(1).ok_or_else(|| {
                PardosaError::FiberInvariantViolation(
                    "last event_id is u64::MAX — next_event_id cannot be derived".to_string(),
                )
            })?,
        };
        if self.next_event_id != expected_next {
            return Err(PardosaError::FiberInvariantViolation(format!(
                "next_event_id {} does not match expected {} (line.len()={})",
                self.next_event_id,
                expected_next,
                self.line.len(),
            )));
        }

        // (d) every fiber in lookup has anchor/current index < line.len()
        let line_len = self.line.len();
        for (id, (fiber, _)) in &self.lookup {
            let cur = fiber.current().as_usize();
            if cur >= line_len {
                return Err(PardosaError::FiberInvariantViolation(format!(
                    "fiber {id:?} current index {cur} >= line.len() {line_len}",
                )));
            }
        }

        Ok(())
    }

    /// Verify that every event's `precursor` (when set) points to an earlier
    /// event in the same domain.
    ///
    /// # Errors
    ///
    /// Returns [`PardosaError::BrokenPrecursorChain`] when an event names a
    /// precursor that is forward-looking (precursor index ≥ event index) or
    /// that belongs to a different domain.
    ///
    /// Returns [`PardosaError::PrecursorHashMismatch`] when an event's
    /// `precursor_hash` does not match BLAKE3 of the canonical bytes of the
    /// predecessor it points to (PAR-0021:R5, P-include semantics).
    pub fn verify_precursor_chains(&self) -> Result<(), PardosaError>
    where
        T: Encode,
    {
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

            // F2b: hash-chain check. Recompute the predecessor's BLAKE3
            // identity and compare against the precursor_hash the event
            // committed at write time. Any divergence is tamper / corruption.
            let expected = precursor_hash_of(&to_vec(precursor_event));
            let actual = event.precursor_hash();
            if actual != expected {
                return Err(PardosaError::PrecursorHashMismatch {
                    event_id: event.event_id(),
                    expected,
                    actual,
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

    // ── Atomic-commit helper (FH5) ────────────────────────────────────
    //
    // The four/five Dragline writers (create, create_reuse, update,
    // detach, rescue) all share a pre-validate-then-mutate shape. To
    // make partial commits unrepresentable, every fallible computation
    // is hoisted into a `prepare` closure that returns a `PreparedCommit`
    // describing the mutations to perform. `commit_atomic` then drives
    // the mutations through `apply_prepared`, which is infallible: the
    // line append is the last operation that can fail (it is gated by
    // `Linevec::append_validated`'s pre-validation), and every step
    // after it consists of operations whose fallibility was already
    // discharged in `prepare` (including `Fiber::advance`, lifted into
    // `Fiber::check_advance` here and replayed via
    // `Fiber::advance_unchecked` in apply).

    fn commit_atomic<F>(&mut self, prepare: F) -> Result<AppendResult, PardosaError>
    where
        F: FnOnce(&Self) -> Result<PreparedCommit<T>, PardosaError>,
    {
        self.reject_if_migrating()?;
        let prepared = prepare(self)?;
        self.apply_prepared(prepared)
    }

    /// Infallible mutation phase. Every error path was discharged in
    /// `prepare`; if `line.append_validated` returns Err here it
    /// indicates a same-writer bug — the prepare phase computed a
    /// candidate that violates Linevec's invariants. Surface loudly.
    fn apply_prepared(&mut self, p: PreparedCommit<T>) -> Result<AppendResult, PardosaError> {
        let PreparedCommit {
            event,
            event_id,
            index,
            domain_id,
            lookup_op,
            next_id_advance,
            purged_remove,
        } = p;

        // Single remaining fallible step. Anything but Ok here means
        // prepare missed a validation — bug, not user error.
        self.line.append_validated(event, event_id)?;

        match lookup_op {
            LookupOp::Insert { fiber, state } => {
                self.lookup.insert(domain_id, (fiber, state));
            }
            LookupOp::AdvanceFiber {
                new_current,
                new_state,
            } => {
                let (fiber, state) = self
                    .lookup
                    .get_mut(&domain_id)
                    .expect("prepare verified fiber presence");
                fiber.advance_unchecked(new_current);
                if let Some(ns) = new_state {
                    *state = ns;
                }
            }
            LookupOp::ReplaceFiber { fiber, new_state } => {
                let (slot_fiber, slot_state) = self
                    .lookup
                    .get_mut(&domain_id)
                    .expect("prepare verified fiber presence");
                *slot_fiber = fiber;
                *slot_state = new_state;
            }
        }

        if let Some(d) = purged_remove {
            self.purged_ids.remove(&d);
        }
        if let Some(d) = next_id_advance {
            self.next_id = d;
        }
        self.next_event_id = event_id + 1;

        Ok(AppendResult {
            domain_id,
            event_id,
            index,
        })
    }

    /// Reassemble a `Dragline` from raw parts, gated by [`verify_invariants`].
    ///
    /// Persistence-boundary surface used by tests today and by the future
    /// `load_from_disk` constructor; the boundary contract is that no
    /// `Dragline` value escapes this function unless every invariant in
    /// [`verify_invariants`] holds. Direct field construction within the
    /// crate bypasses this check by design (the write-path methods
    /// maintain the invariants by construction); any code reassembling
    /// state from external bytes must come through here.
    #[cfg(test)]
    pub(crate) fn from_raw_parts(
        line: Vec<Event<T>>,
        lookup: HashMap<DomainId, (Fiber, FiberState)>,
        purged_ids: HashSet<DomainId>,
        next_id: DomainId,
        next_event_id: u64,
        migrating: bool,
    ) -> Result<Self, PardosaError>
    where
        T: Encode,
    {
        let d = Dragline {
            line: Linevec::from_raw_unchecked(line),
            lookup,
            purged_ids,
            next_id,
            next_event_id,
            migrating,
        };
        d.verify_invariants()?;
        Ok(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fiber_state::MigrationPolicy;

    // ── Linevec::append_validated rejection cases ─────────────────────
    //
    // FH3: every production write must go through append_validated. These
    // tests cover the rejection paths directly on the newtype so a future
    // refactor that breaks one of the invariants surfaces here rather than
    // limping along until verify_invariants is next called.

    #[test]
    fn linevec_append_validated_rejects_mismatched_event_id() {
        let mut lv = Linevec::<&str>::new();
        let e = Event::new(
            5,
            1000,
            DomainId::new(0),
            false,
            Index::NONE,
            [0u8; 32],
            "x",
        );
        // caller claims expected_event_id = 7 but event carries 5
        let err = lv.append_validated(e, 7).unwrap_err();
        assert!(
            matches!(err, PardosaError::FiberInvariantViolation(ref m) if m.contains("!=")),
            "got: {err}"
        );
        assert_eq!(lv.len(), 0, "rejected append must not mutate line");
    }

    #[test]
    fn linevec_append_validated_rejects_non_monotonic_event_id() {
        let mut lv = Linevec::<&str>::new();
        let e0 = Event::new(
            0,
            1000,
            DomainId::new(0),
            false,
            Index::NONE,
            [0u8; 32],
            "a",
        );
        lv.append_validated(e0, 0).unwrap();
        // Try to append an event with event_id == last (0), violating strict >.
        let e_dup = Event::new(
            0,
            1001,
            DomainId::new(0),
            false,
            Index::NONE,
            [0u8; 32],
            "dup",
        );
        let err = lv.append_validated(e_dup, 0).unwrap_err();
        assert!(
            matches!(err, PardosaError::FiberInvariantViolation(ref m) if m.contains("not strictly greater")),
            "got: {err}"
        );
        assert_eq!(lv.len(), 1, "rejected append must not mutate line");
    }

    #[test]
    fn linevec_append_validated_rejects_forward_precursor() {
        let mut lv = Linevec::<&str>::new();
        let e0 = Event::new(
            0,
            1000,
            DomainId::new(0),
            false,
            Index::NONE,
            [0u8; 32],
            "a",
        );
        lv.append_validated(e0, 0).unwrap();
        // line.len() == 1, so precursor must be < 1; offer Index::new(5).
        let bad = Event::new(
            1,
            1001,
            DomainId::new(0),
            false,
            Index::new(5),
            [0u8; 32],
            "bad",
        );
        let err = lv.append_validated(bad, 1).unwrap_err();
        assert!(
            matches!(err, PardosaError::FiberInvariantViolation(ref m) if m.contains("precursor index")),
            "got: {err}"
        );
        assert_eq!(lv.len(), 1, "rejected append must not mutate line");
    }

    #[test]
    fn linevec_append_validated_rejects_cross_domain_precursor() {
        let mut lv = Linevec::<&str>::new();
        let e0 = Event::new(
            0,
            1000,
            DomainId::new(0),
            false,
            Index::NONE,
            [0u8; 32],
            "a",
        );
        lv.append_validated(e0, 0).unwrap();
        // event for domain 99 cannot point precursor at domain 0's event
        let bad = Event::new(
            1,
            1001,
            DomainId::new(99),
            false,
            Index::new(0),
            [0u8; 32],
            "bad",
        );
        let err = lv.append_validated(bad, 1).unwrap_err();
        assert!(
            matches!(err, PardosaError::BrokenPrecursorChain { .. }),
            "got: {err}"
        );
        assert_eq!(lv.len(), 1, "rejected append must not mutate line");
    }

    #[test]
    fn linevec_append_validated_accepts_valid_event() {
        let mut lv = Linevec::<&str>::new();
        let e0 = Event::new(
            0,
            1000,
            DomainId::new(0),
            false,
            Index::NONE,
            [0u8; 32],
            "a",
        );
        lv.append_validated(e0, 0).unwrap();
        let e1 = Event::new(
            1,
            1001,
            DomainId::new(0),
            false,
            Index::new(0),
            [0u8; 32],
            "b",
        );
        lv.append_validated(e1, 1).unwrap();
        assert_eq!(lv.len(), 2);
    }

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

    // ── FH4: create() liveness across interleaved purges ──────────────
    //
    // Before FH4, create() bailed with IdAlreadyExists the moment its
    // auto-assigned next_id collided with a purged_id. A long-running
    // stream that interleaved auto-creates with purges of low-numbered
    // ids could herd the caller into a corner where only the explicit
    // create_reuse(specific_id, …) path could make progress. These tests
    // assert create() now transparently advances past purged ids.

    #[test]
    fn create_advances_past_purged_ids_in_long_run() {
        // Liveness shape: heavy interleave of creates + purges via the
        // public API must never stall. Pre-FH4 this passes too (because
        // next_id is monotonic and races ahead of any purged id reachable
        // through the public API alone), but it pins down the API-level
        // contract the user expects.
        let mut d = Dragline::new();
        let mut created: Vec<DomainId> = Vec::new();
        for ts in 0..50_i64 {
            let r = d.create(1000 + ts, "x").unwrap();
            created.push(r.domain_id);
        }
        for (i, id) in created.iter().enumerate() {
            if i % 2 == 0 {
                d.detach(
                    *id,
                    2000 + i64::try_from(i).expect("test index fits in i64"),
                    "det",
                )
                .unwrap();
                d.migrate_fiber(*id, MigrationPolicy::Purge).unwrap();
            }
        }
        for ts in 3000..3050_i64 {
            let r = d
                .create(ts, "y")
                .expect("create() must not stall on liveness across purges");
            assert!(!matches!(d.fiber_state(r.domain_id), FiberState::Purged));
        }
    }

    #[test]
    fn create_skips_purged_when_next_id_collides() {
        // Direct mechanism test for the FH4 fix. The public API
        // monotonically advances next_id past every assignment, so the
        // collision condition is only reachable from external bytes
        // (load_from_disk, migration replay). We use from_raw_parts to
        // construct a Dragline whose next_id == 5 sits on a purged id.
        // Pre-FH4: create() fails with IdAlreadyExists. Post-FH4:
        // create() skips 5/6/7 and assigns 8.
        let mut purged_ids = HashSet::new();
        purged_ids.insert(DomainId::new(5));
        purged_ids.insert(DomainId::new(6));
        purged_ids.insert(DomainId::new(7));
        let mut d = Dragline::<&str>::from_raw_parts(
            Vec::new(),
            HashMap::new(),
            purged_ids,
            DomainId::new(5),
            0,
            false,
        )
        .unwrap();

        let r = d
            .create(1000, "fresh")
            .expect("create() must skip purged ids");
        assert_eq!(
            r.domain_id,
            DomainId::new(8),
            "create() should have skipped 5, 6, 7"
        );
        assert_eq!(d.next_domain_id(), DomainId::new(9));
    }

    #[test]
    fn create_overflows_when_remaining_ids_all_purged() {
        // Bounded-loop guarantee: if next_id is u64::MAX AND it is purged,
        // the skip loop has nowhere to advance and surfaces
        // DomainIdOverflow rather than looping forever.
        let mut purged_ids = HashSet::new();
        purged_ids.insert(DomainId::new(u64::MAX));
        let mut d = Dragline::<&str>::from_raw_parts(
            Vec::new(),
            HashMap::new(),
            purged_ids,
            DomainId::new(u64::MAX),
            0,
            false,
        )
        .unwrap();

        let err = d.create(1000, "x").unwrap_err();
        assert!(
            matches!(err, PardosaError::DomainIdOverflow),
            "expected DomainIdOverflow, got: {err}"
        );
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
        let bad_event = Event::new(
            99,
            2000,
            DomainId::new(99),
            false,
            Index::new(0),
            [0u8; 32],
            "broken",
        );
        d.line.force_push_unchecked(bad_event);

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
        let bad_event = Event::new(
            99,
            2000,
            DomainId::new(0),
            false,
            Index::new(5),
            [0u8; 32],
            "broken",
        );
        d.line.force_push_unchecked(bad_event);

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
        let bad_event = Event::new(
            99,
            2000,
            DomainId::new(0),
            false,
            Index::new(1),
            [0u8; 32],
            "broken",
        );
        d.line.force_push_unchecked(bad_event);

        let err = d.verify_precursor_chains().unwrap_err();
        assert!(
            matches!(err, PardosaError::BrokenPrecursorChain { .. }),
            "expected BrokenPrecursorChain, got: {err}"
        );
    }

    // ── Tamper regression (PAR-0021:R5, F2b) ──────────────────────────
    //
    // The hash-chain check exists so that a downstream actor cannot mutate
    // a historical event undetected: any byte change in event K invalidates
    // event K+1's `precursor_hash`. These tests exercise both directions —
    // happy chain stays green; tampered chain surfaces PrecursorHashMismatch
    // pinpointing the *successor* of the mutated event (the one whose
    // committed `precursor_hash` no longer matches the recomputed identity).
    //
    // The seam is `from_raw_parts` (L1109) — the in-crate persistence-
    // boundary reconstruction surface used by surrounding tests. No
    // `unsafe`, no new public surface, no new seam.

    #[test]
    fn verify_precursor_chains_detects_tampered_predecessor() {
        // Build a valid 3-event chain: create + two updates.
        let mut d = Dragline::<&'static str>::new();
        let r = d.create(1000, "created").unwrap();
        d.update(r.domain_id, 1001, "u1").unwrap();
        d.update(r.domain_id, 1002, "u2").unwrap();

        // Sanity: the chain is valid pre-tamper.
        assert!(d.verify_precursor_chains().is_ok());

        // Snapshot raw parts. We tamper line[1]'s payload — all other fields
        // preserved (event_id, timestamp, domain_id, precursor index, and
        // its OWN precursor_hash which was correct relative to genesis).
        // line[2]'s committed precursor_hash was computed over the original
        // line[1] bytes; after we swap line[1]'s payload the recomputation
        // at verify time will diverge.
        let mut line = d.line.as_slice().to_vec();
        let original = line[1].clone();
        let tampered = Event::new(
            original.event_id(),
            original.timestamp(),
            original.domain_id(),
            original.detached(),
            original.precursor(),
            original.precursor_hash(),
            "TAMPERED", // payload swap — different bytes than "u1"
        );
        let expected_mismatch_event_id = line[2].event_id();
        line[1] = tampered;

        // Reconstruct via from_raw_parts. verify_invariants runs internally
        // and calls verify_precursor_chains, so from_raw_parts returns
        // Err(PrecursorHashMismatch) — gate works as intended.
        let err = Dragline::from_raw_parts(
            line,
            d.lookup.clone(),
            d.purged_ids.clone(),
            d.next_id,
            d.next_event_id,
            false,
        )
        .unwrap_err();

        match err {
            PardosaError::PrecursorHashMismatch {
                event_id,
                expected,
                actual,
            } => {
                assert_eq!(
                    event_id, expected_mismatch_event_id,
                    "mismatch should pinpoint the successor (line[2]), not the tampered event itself",
                );
                assert_ne!(expected, actual, "hashes must differ on tamper");
            }
            other => panic!("expected PrecursorHashMismatch, got: {other}"),
        }
    }

    #[test]
    fn verify_precursor_chains_valid_chain_is_regression_canary() {
        // Mirrors verify_precursor_chains_valid (L1609) but exists explicitly
        // as a guard against an F2b regression that would compute different
        // hashes on the write path vs the verify path. If write/verify ever
        // disagree, this test goes red even without tampering.
        let mut d = Dragline::<&'static str>::new();
        let r = d.create(1000, "created").unwrap();
        d.update(r.domain_id, 1001, "u1").unwrap();
        d.update(r.domain_id, 1002, "u2").unwrap();
        d.update(r.domain_id, 1003, "u3").unwrap();

        // Each non-genesis event's committed precursor_hash MUST match
        // BLAKE3 of its predecessor's encoded bytes. Anything else means
        // writer and verifier diverged in their hash derivation.
        assert!(d.verify_precursor_chains().is_ok());
    }

    // ── verify_invariants (superset of precursor chains) ─────────────

    // Snapshot of the private fields that `from_raw_parts` rebuilds from.
    // Boxed-tuple alternative tripped clippy::type_complexity; a small
    // struct keeps the helper readable and the field set self-documenting.
    struct ValidParts {
        line: Vec<Event<&'static str>>,
        lookup: HashMap<DomainId, (Fiber, FiberState)>,
        purged_ids: HashSet<DomainId>,
        next_id: DomainId,
        next_event_id: u64,
    }

    // Helper: build a dragline that mirrors a small, valid sequence so
    // tests can perturb individual fields before re-running verify_invariants.
    fn build_valid() -> ValidParts {
        let mut d = Dragline::<&'static str>::new();
        let r1 = d.create(1000, "a").unwrap();
        d.update(r1.domain_id, 1001, "a2").unwrap();
        let r2 = d.create(1002, "b").unwrap();
        d.detach(r2.domain_id, 1003, "b-detach").unwrap();
        ValidParts {
            line: d.line.as_slice().to_vec(),
            lookup: d.lookup.clone(),
            purged_ids: d.purged_ids.clone(),
            next_id: d.next_id,
            next_event_id: d.next_event_id,
        }
    }

    #[test]
    fn verify_invariants_accepts_public_api_built_dragline() {
        // Sanity: anything the write-path produces must satisfy the superset.
        // Failure here is a Bucket B observation — back-brief moltke.
        let mut d = Dragline::<&str>::new();
        let r1 = d.create(1000, "a").unwrap();
        d.update(r1.domain_id, 1001, "a2").unwrap();
        let r2 = d.create(1002, "b").unwrap();
        d.detach(r2.domain_id, 1003, "b-detach").unwrap();
        d.migrate_fiber(r2.domain_id, MigrationPolicy::Purge)
            .unwrap();
        d.create_reuse(r2.domain_id, 1004, "b-reuse").unwrap();

        assert!(d.verify_invariants().is_ok(), "{:?}", d.verify_invariants());
    }

    #[test]
    fn verify_invariants_empty_dragline_ok() {
        let d = Dragline::<&str>::new();
        assert!(d.verify_invariants().is_ok());
    }

    #[test]
    fn verify_invariants_rejects_non_monotonic_event_ids() {
        let ValidParts {
            mut line,
            lookup,
            purged_ids,
            next_id,
            next_event_id,
        } = build_valid();
        let last_idx = line.len() - 1;
        let prev_id = line[last_idx - 1].event_id();
        let dup = Event::new(
            prev_id, // duplicate previous event_id — violates strict <
            line[last_idx].timestamp(),
            line[last_idx].domain_id(),
            line[last_idx].detached(),
            line[last_idx].precursor(),
            line[last_idx].precursor_hash(),
            *line[last_idx].domain_event(),
        );
        line[last_idx] = dup;

        let err = Dragline::from_raw_parts(line, lookup, purged_ids, next_id, next_event_id, false)
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            matches!(err, PardosaError::FiberInvariantViolation(_))
                && msg.contains("event_id not monotonic"),
            "got: {err}"
        );
    }

    #[test]
    fn verify_invariants_rejects_purged_id_in_lookup() {
        let ValidParts {
            line,
            lookup,
            mut purged_ids,
            next_id,
            next_event_id,
        } = build_valid();
        let live_id = *lookup.keys().next().unwrap();
        purged_ids.insert(live_id);

        let err = Dragline::from_raw_parts(line, lookup, purged_ids, next_id, next_event_id, false)
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            matches!(err, PardosaError::FiberInvariantViolation(_))
                && msg.contains("both purged and present in lookup"),
            "got: {err}"
        );
    }

    #[test]
    fn verify_invariants_rejects_wrong_next_event_id() {
        let ValidParts {
            line,
            lookup,
            purged_ids,
            next_id,
            next_event_id,
        } = build_valid();
        let bad_next = next_event_id + 1; // off-by-one

        let err = Dragline::from_raw_parts(line, lookup, purged_ids, next_id, bad_next, false)
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            matches!(err, PardosaError::FiberInvariantViolation(_))
                && msg.contains("next_event_id"),
            "got: {err}"
        );
    }

    #[test]
    fn verify_invariants_rejects_nonzero_next_event_id_on_empty_line() {
        let err = Dragline::<&str>::from_raw_parts(
            Vec::new(),
            HashMap::new(),
            HashSet::new(),
            DomainId::new(0),
            1, // empty line ⇒ next_event_id must be 0
            false,
        )
        .unwrap_err();
        assert!(
            matches!(err, PardosaError::FiberInvariantViolation(_)),
            "got: {err}"
        );
    }

    #[test]
    fn verify_invariants_rejects_fiber_index_out_of_bounds() {
        let ValidParts {
            line,
            mut lookup,
            purged_ids,
            next_id,
            next_event_id,
        } = build_valid();
        let line_len_u64 = u64::try_from(line.len()).unwrap();
        let bogus_index = Index::new(line_len_u64); // == len ⇒ out of bounds
        let bogus_fiber = Fiber::new(bogus_index, 1, bogus_index).unwrap();
        lookup.insert(DomainId::new(999), (bogus_fiber, FiberState::Defined));

        let err = Dragline::from_raw_parts(line, lookup, purged_ids, next_id, next_event_id, false)
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            matches!(err, PardosaError::FiberInvariantViolation(_))
                && msg.contains(">= line.len()"),
            "got: {err}"
        );
    }

    #[test]
    fn verify_invariants_propagates_broken_precursor_chain() {
        // Superset must surface structural per-event breakage through the
        // original variant so existing callers that match on it keep working.
        let ValidParts {
            mut line,
            lookup,
            purged_ids,
            next_id,
            next_event_id: _,
        } = build_valid();
        let pos = u64::try_from(line.len()).unwrap();
        let bad = Event::new(
            line.last().unwrap().event_id() + 1,
            9999,
            DomainId::new(0),
            false,
            Index::new(pos + 5), // forward reference
            [0u8; 32],
            "bad",
        );
        line.push(bad);
        let new_next = u64::try_from(line.len()).unwrap();

        let err = Dragline::from_raw_parts(line, lookup, purged_ids, next_id, new_next, false)
            .unwrap_err();
        assert!(
            matches!(err, PardosaError::BrokenPrecursorChain { .. }),
            "got: {err}"
        );
    }

    #[test]
    fn from_raw_parts_accepts_valid_state() {
        let ValidParts {
            line,
            lookup,
            purged_ids,
            next_id,
            next_event_id,
        } = build_valid();
        let d = Dragline::from_raw_parts(line, lookup, purged_ids, next_id, next_event_id, false)
            .expect("valid state must round-trip through from_raw_parts");
        assert!(d.verify_invariants().is_ok());
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

            // FH5 (adr-fmt-pp3c, adr-fmt-w2bs): commit_atomic must be all
            // or nothing. For every fallible step reachable inside any
            // writer, on Err the three observable counters must be
            // unchanged: (a) next_event_id, (b) line.len(), (c) lookup
            // contents. Schema-hash injection (per bead AC) does not fit
            // Dragline's `T` (no trait bound) — instead we exercise every
            // reachable failure mode of every writer and assert atomicity
            // by counter comparison.
            #[test]
            fn commit_atomic_preserves_state_on_err(
                writer_pick in 0u8..5,
                failure_mode in 0u8..3,
                seed_creates in 1usize..8,
            ) {
                let mut d = Dragline::<String>::new();
                let mut ids: Vec<DomainId> = Vec::new();
                for i in 0..seed_creates {
                    let r = d.create(i64::try_from(i).unwrap(), format!("seed{i}")).unwrap();
                    ids.push(r.domain_id);
                }
                // detach one to enable rescue path
                let detached_id = if ids.len() >= 2 {
                    let id = ids[1];
                    d.detach(id, 1000, "det".into()).unwrap();
                    Some(id)
                } else {
                    None
                };

                // Engineer the Dragline into a state where the chosen
                // writer's chosen failure_mode will Err.
                match failure_mode {
                    0 => {
                        // Migration mode: every writer Errs in commit_atomic
                        // BEFORE prepare runs (reject_if_migrating).
                        d.set_migrating(true);
                    }
                    1 => {
                        // DomainIdOverflow for create() path; other writers
                        // hit different (or no) Err. We only assert atomicity,
                        // which holds in either case.
                        let line: Vec<Event<String>> = d.read_line().to_vec();
                        let lookup: HashMap<DomainId, (Fiber, FiberState)> = ids
                            .iter()
                            .filter_map(|id| {
                                let s = d.fiber_state(*id);
                                if matches!(s, FiberState::Undefined) {
                                    None
                                } else {
                                    Some((*id, (d.lookup.get(id).unwrap().0.clone(), s)))
                                }
                            })
                            .collect();
                        let next_event_id = d.next_event_id();
                        d = Dragline::<String>::from_raw_parts(
                            line, lookup, HashSet::new(), DomainId::new(u64::MAX), next_event_id, false,
                        ).unwrap();
                    }
                    _ => {
                        // FiberNotFound for update/detach/rescue.
                        // For create / create_reuse this mode does not force
                        // an Err — but atomicity is still asserted only when
                        // Err occurs, so a successful call is fine.
                    }
                }

                // Snapshot pre-call state.
                let pre_next_event_id = d.next_event_id();
                let pre_line_len = d.line_len();
                let pre_lookup_snapshot: HashMap<DomainId, FiberState> =
                    ids.iter().map(|id| (*id, d.fiber_state(*id))).collect();
                let pre_next_id = d.next_domain_id();

                // Dispatch the writer. For mode 2 (FiberNotFound) we pass
                // a never-present DomainId::new(u64::MAX).
                let bogus = DomainId::new(u64::MAX);
                let target_id = if failure_mode == 2 {
                    bogus
                } else {
                    detached_id.unwrap_or(ids[0])
                };

                let result: Result<AppendResult, PardosaError> = match writer_pick {
                    0 => d.create(2000, "x".into()),
                    1 => d.create_reuse(bogus, 2000, "x".into()),
                    2 => d.update(target_id, 2000, "x".into()),
                    3 => d.detach(ids[0], 2000, "x".into()),
                    _ => d.rescue(
                        target_id,
                        LockedRescuePolicy::PreserveAuditTrail,
                        2000,
                        "x".into(),
                    ),
                };

                // Atomicity property: if the call Erred, NO state advanced.
                // If it succeeded, that is also a valid outcome (this
                // proptest does not require the call to fail — only that
                // failure, when it occurs, leaves no partial commit).
                if result.is_err() {
                    prop_assert_eq!(
                        d.next_event_id(),
                        pre_next_event_id,
                        "next_event_id advanced on Err"
                    );
                    prop_assert_eq!(
                        d.line_len(),
                        pre_line_len,
                        "line.len() changed on Err"
                    );
                    prop_assert_eq!(
                        d.next_domain_id(),
                        pre_next_id,
                        "next_domain_id advanced on Err"
                    );
                    for id in &ids {
                        prop_assert_eq!(
                            d.fiber_state(*id),
                            *pre_lookup_snapshot.get(id).unwrap(),
                            "fiber state changed on Err for id {:?}", id
                        );
                    }
                }
            }

            // Targeted regression for the latent partial-commit window
            // (Bucket B evidence bead adr-fmt-w2bs): line.append_validated
            // succeeded then fiber.advance Err'd on len overflow, leaving
            // an extra event in the line without next_event_id/state
            // advancing. After FH5 commit_atomic + Fiber::check_advance
            // lift, this window is closed: fiber-overflow is caught in
            // prepare BEFORE the line append.
            #[test]
            fn fiber_advance_overflow_does_not_partial_commit(_dummy in 0..1u8) {
                // Construct a Dragline with one fiber whose len is already
                // at u64::MAX, so the next update's check_advance will Err
                // on len overflow.
                let index0 = Index::new(0);
                let fiber = Fiber::new(index0, u64::MAX, index0).unwrap();
                let mut lookup = HashMap::new();
                let domain_id = DomainId::new(0);
                lookup.insert(domain_id, (fiber, FiberState::Defined));
                let event = Event::new(0u64, 0, domain_id, false, Index::NONE, [0u8; 32], "seed".to_string());
                let mut d = Dragline::<String>::from_raw_parts(
                    vec![event], lookup, HashSet::new(), DomainId::new(1), 1, false,
                ).unwrap();

                let pre_event_id = d.next_event_id();
                let pre_line_len = d.line_len();

                let err = d.update(domain_id, 100, "u".into()).unwrap_err();
                prop_assert!(
                    matches!(err, PardosaError::FiberInvariantViolation(_)),
                    "expected FiberInvariantViolation, got: {err:?}"
                );
                prop_assert_eq!(d.next_event_id(), pre_event_id, "next_event_id advanced on overflow");
                prop_assert_eq!(d.line_len(), pre_line_len, "line gained an event on overflow");
            }
        }
    }
}
