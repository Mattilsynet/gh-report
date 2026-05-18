//! `Dragline<T>` core state and constructors.
//!
//! Holds the struct definition (line + fiber lookup + bookkeeping),
//! `Default`/`new`, and the persistence-boundary [`Dragline::from_raw_parts`]
//! reassembly constructor. Behavioural surface (write/read methods)
//! lives in sibling [`super::api`]; commit machinery in
//! [`super::commit`].

use std::collections::{HashMap, HashSet};

use super::linevec::Linevec;
#[cfg(any(test, feature = "test-support"))]
use crate::error::PardosaError;
use crate::event::DomainId;
#[cfg(any(test, feature = "test-support"))]
use crate::event::Event;
use crate::fiber::Fiber;
use crate::fiber_state::FiberState;
use crate::frontier::FrontierPublisher;
#[cfg(any(test, feature = "test-support"))]
use pardosa_encoding::Encode;

/// Default tick interval: publish frontier every 1 000 committed events.
///
/// Count-based rather than wall-clock so the mechanism is deterministic in
/// tests without a runtime timer and requires no async executor. Wall-clock
/// anchoring belongs to the Phase 3 SEC-0010 production NATS wiring.
pub const DEFAULT_ANCHOR_INTERVAL: u64 = 1_000;

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
/// - `frontier` is updated on every commit via BLAKE3 chaining (PAR-0021:R3).
/// - `events_since_tick` resets to 0 after each `anchor_interval` publish.
#[derive(Debug)]
pub struct Dragline<T> {
    pub(super) line: Linevec<T>,
    pub(super) lookup: HashMap<DomainId, (Fiber, FiberState)>,
    pub(super) purged_ids: HashSet<DomainId>,
    pub(super) next_id: DomainId,
    pub(super) next_event_id: u64,
    pub(super) migrating: bool,
    /// Rolling BLAKE3 frontier hash (PAR-0021:R3). All-zero until the first event.
    pub(super) frontier: [u8; 32],
    /// NATS subject prefix component: subject = `pardosa.{stream_name}.frontier`.
    pub(super) stream_name: String,
    /// Publish frontier every `anchor_interval` committed events (PAR-0021:R4).
    /// Default: [`DEFAULT_ANCHOR_INTERVAL`].
    pub(super) anchor_interval: u64,
    /// Count of events since the last `anchor_interval` tick.
    pub(super) events_since_tick: u64,
    /// Optional publisher; `None` when created via [`Dragline::new`] (no NATS wiring).
    pub(super) publisher: Option<Box<dyn FrontierPublisher>>,
}

impl<T> Default for Dragline<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Dragline<T> {
    /// Create a new empty dragline with no publisher attached.
    ///
    /// Commits proceed normally; no frontier publish occurs. Suitable for
    /// unit tests and contexts where the Phase 3 NATS wiring (SEC-0010) is
    /// not yet available.
    #[must_use]
    pub fn new() -> Self {
        Dragline {
            line: Linevec::new(),
            lookup: HashMap::new(),
            purged_ids: HashSet::new(),
            next_id: DomainId::new(0),
            next_event_id: 0,
            migrating: false,
            frontier: [0u8; 32],
            stream_name: String::new(),
            anchor_interval: DEFAULT_ANCHOR_INTERVAL,
            events_since_tick: 0,
            publisher: None,
        }
    }

    /// Create a dragline wired to a `FrontierPublisher`.
    ///
    /// Every `anchor_interval` committed events, the current frontier hash is
    /// published to `pardosa.{stream_name}.frontier` (PAR-0021:R4).
    ///
    /// `anchor_interval` must be ≥ 1. A value of 0 is silently promoted to 1
    /// (publish on every commit) to avoid infinite-tick semantics.
    #[must_use]
    pub fn with_publisher<P: FrontierPublisher>(
        stream_name: String,
        anchor_interval: u64,
        publisher: P,
    ) -> Self {
        Dragline {
            line: Linevec::new(),
            lookup: HashMap::new(),
            purged_ids: HashSet::new(),
            next_id: DomainId::new(0),
            next_event_id: 0,
            migrating: false,
            frontier: [0u8; 32],
            stream_name,
            anchor_interval: anchor_interval.max(1),
            events_since_tick: 0,
            publisher: Some(Box::new(publisher)),
        }
    }

    /// The current frontier hash (PAR-0021:R3).
    ///
    /// Returns `[0u8; 32]` on an empty dragline. Updated on every commit via
    /// BLAKE3 chaining; deterministic from the append order.
    #[must_use]
    pub fn frontier(&self) -> [u8; 32] {
        self.frontier
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
    #[cfg(any(test, feature = "test-support"))]
    pub fn from_raw_parts(
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
            frontier: [0u8; 32],
            stream_name: String::new(),
            anchor_interval: DEFAULT_ANCHOR_INTERVAL,
            events_since_tick: 0,
            publisher: None,
        };
        d.verify_invariants()?;
        Ok(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DomainId, Event, Fiber, FiberState, Index, MigrationPolicy, PardosaError};
    use std::collections::{HashMap, HashSet};

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
}
