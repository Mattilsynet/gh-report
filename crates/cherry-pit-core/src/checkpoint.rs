//! Durable projection checkpoint type.
//!
//! Per CHE-0048 R9, `ProjectionCheckpoint` is the canonical foundational
//! data type carrying `(aggregate_id, projection_name, last_sequence)` for
//! the read side. Storage backends (e.g. `cherry-pit-projection`'s
//! `FileProjectionStore`) consume this type; the type itself owns no I/O
//! and pulls no async / storage dependencies (CHE-0029 R4–R5).

use serde::{Deserialize, Serialize};

use crate::AggregateId;

/// Durable checkpoint for one `(aggregate_id, projection_name)` pair.
///
/// A checkpoint records the highest event sequence that has been folded into
/// the persisted projection snapshot. Per CHE-0024 R3/R4 the checkpoint is
/// written *after* the snapshot side-effect completes; on restart, replay
/// resumes from the checkpoint's `last_sequence + 1`.
///
/// # Examples
///
/// ```
/// use std::num::NonZeroU64;
/// use cherry_pit_core::AggregateId;
/// use cherry_pit_core::ProjectionCheckpoint;
///
/// let id = AggregateId::new(NonZeroU64::new(42).unwrap());
/// let cp = ProjectionCheckpoint::new(id, "counter_view", 7);
///
/// assert_eq!(cp.aggregate_id(), id);
/// assert_eq!(cp.projection_name(), "counter_view");
/// assert_eq!(cp.last_sequence(), 7);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectionCheckpoint {
    aggregate_id: AggregateId,
    projection_name: String,
    last_sequence: u64,
}

impl ProjectionCheckpoint {
    /// Build a checkpoint after applying all events through `last_sequence`.
    #[must_use]
    pub fn new(
        aggregate_id: AggregateId,
        projection_name: impl Into<String>,
        last_sequence: u64,
    ) -> Self {
        Self {
            aggregate_id,
            projection_name: projection_name.into(),
            last_sequence,
        }
    }

    /// Aggregate stream this checkpoint belongs to.
    #[must_use]
    pub const fn aggregate_id(&self) -> AggregateId {
        self.aggregate_id
    }

    /// Stable handler/projection identity.
    #[must_use]
    pub fn projection_name(&self) -> &str {
        &self.projection_name
    }

    /// Last applied event sequence.
    #[must_use]
    pub const fn last_sequence(&self) -> u64 {
        self.last_sequence
    }
}

#[cfg(test)]
mod tests {
    //! Runtime coverage for CHE-0048 R9.
    //!
    //! Asserts `ProjectionCheckpoint`'s constructor stores all three fields and
    //! the accessors return them unchanged, plus that `Clone` + `PartialEq` + `Eq`
    //! round-trip correctly (the derived impls).

    use std::num::NonZeroU64;

    use super::ProjectionCheckpoint;
    use crate::AggregateId;

    fn sample_id(n: u64) -> AggregateId {
        AggregateId::new(NonZeroU64::new(n).expect("non-zero literal"))
    }

    #[test]
    fn new_constructs_with_given_fields() {
        let id = sample_id(42);
        let cp = ProjectionCheckpoint::new(id, "counter_view", 7);
        // Round-trip via Debug as a smoke test that fields are stored
        // (accessor coverage is asserted below).
        let dbg = format!("{cp:?}");
        assert!(
            dbg.contains("42"),
            "debug should expose aggregate id: {dbg}"
        );
        assert!(
            dbg.contains("counter_view"),
            "debug should expose name: {dbg}"
        );
        assert!(dbg.contains('7'), "debug should expose sequence: {dbg}");
    }

    #[test]
    fn accessors_return_constructor_inputs() {
        let id = sample_id(100);
        let cp = ProjectionCheckpoint::new(id, "orders_view", 123);
        assert_eq!(cp.aggregate_id(), id);
        assert_eq!(cp.projection_name(), "orders_view");
        assert_eq!(cp.last_sequence(), 123);
    }

    #[test]
    fn clone_eq_round_trip() {
        let id = sample_id(1);
        let cp = ProjectionCheckpoint::new(id, "view", 0);
        let cloned = cp.clone();
        assert_eq!(cp, cloned);
        // Distinguishability — differing sequence breaks equality.
        let other = ProjectionCheckpoint::new(id, "view", 1);
        assert_ne!(cp, other);
        // Distinguishability — differing projection name breaks equality.
        let other2 = ProjectionCheckpoint::new(id, "view2", 0);
        assert_ne!(cp, other2);
    }
}
