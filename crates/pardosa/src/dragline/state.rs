//! `Dragline<T>` core state and constructors.
//!
//! Holds the struct definition (line + fiber lookup + bookkeeping),
//! `Default`/`new`, and the persistence-boundary [`Dragline::from_raw_parts`]
//! reassembly constructor. Behavioural surface (write/read methods)
//! lives in sibling [`super::api`]; commit machinery in
//! [`super::commit`].

use std::collections::{HashMap, HashSet};

use super::linevec::Linevec;
#[cfg(test)]
use crate::error::PardosaError;
use crate::event::DomainId;
#[cfg(test)]
use crate::event::Event;
use crate::fiber::Fiber;
use crate::fiber_state::FiberState;
#[cfg(test)]
use pardosa_encoding::Encode;

/// Result of a successful append operation.
#[derive(Debug, Clone, Copy)]
pub struct AppendResult {
    /// The domain ID of the affected fiber.
    pub domain_id: DomainId,
    /// The globally monotonic event ID assigned to this event.
    pub event_id: u64,
    /// The position of this event in the line.
    pub index: crate::event::Index,
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
    pub(super) line: Linevec<T>,
    pub(super) lookup: HashMap<DomainId, (Fiber, FiberState)>,
    pub(super) purged_ids: HashSet<DomainId>,
    pub(super) next_id: DomainId,
    pub(super) next_event_id: u64,
    pub(super) migrating: bool,
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
            line: Linevec::new(),
            lookup: HashMap::new(),
            purged_ids: HashSet::new(),
            next_id: DomainId::new(0),
            next_event_id: 0,
            migrating: false,
        }
    }

    /// Reassemble a `Dragline` from raw parts, gated by [`Dragline::verify_invariants`].
    ///
    /// Persistence-boundary surface used by tests today and by the future
    /// `load_from_disk` constructor; the boundary contract is that no
    /// `Dragline` value escapes this function unless every invariant in
    /// `verify_invariants` holds. Direct field construction within the
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
