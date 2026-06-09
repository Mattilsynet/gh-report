//! Durable projection checkpoint type.
//!
//! Per CHE-0048 R9, `ProjectionCheckpoint` is the canonical foundational
//! data type carrying `(aggregate_id, projection_name, last_sequence)` for
//! the read side. Storage backends (e.g. `cherry-pit-projection`'s
//! `FileProjectionStore`) consume this type; the type itself owns no I/O
//! and pulls no async / storage dependencies (CHE-0029 R4–R5).

use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};

use crate::AggregateId;

/// Durable checkpoint for one `(aggregate_id, projection_name)` pair.
///
/// A checkpoint records the highest event sequence that has been folded into
/// the persisted projection snapshot. Per CHE-0024 R3/R4 the checkpoint is
/// written *after* the snapshot side-effect completes; on restart, replay
/// resumes from the checkpoint's `last_sequence + 1`.
///
/// `last_sequence` is `NonZeroU64` because event-stream sequence numbers
/// start at 1 (SEC-0008:R2, `EventEnvelope::sequence` is `NonZeroU64`). A
/// checkpoint exists only when at least one event has been folded into the
/// snapshot; an empty stream produces no checkpoint at all (the absence is
/// the "never any events" signal). Encoding the invariant in the type
/// eliminates the wire-format pothole where `last_sequence=0` was
/// indistinguishable from a phantom checkpoint for a never-created
/// aggregate.
///
/// # Examples
///
/// ```
/// use std::num::NonZeroU64;
/// use cherry_pit_core::AggregateId;
/// use cherry_pit_core::ProjectionCheckpoint;
///
/// let id = AggregateId::new(NonZeroU64::new(42).unwrap());
/// let seven = NonZeroU64::new(7).unwrap();
/// let cp = ProjectionCheckpoint::new(id, "counter_view", seven);
///
/// assert_eq!(cp.aggregate_id(), id);
/// assert_eq!(cp.projection_name(), "counter_view");
/// assert_eq!(cp.last_sequence(), seven);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectionCheckpoint {
    aggregate_id: AggregateId,
    projection_name: String,
    last_sequence: NonZeroU64,
}

impl ProjectionCheckpoint {
    /// Build a checkpoint after applying all events through `last_sequence`.
    #[must_use]
    pub fn new(
        aggregate_id: AggregateId,
        projection_name: impl Into<String>,
        last_sequence: NonZeroU64,
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

    /// Last applied event sequence, guaranteed `>= 1` by the type.
    #[must_use]
    pub const fn last_sequence(&self) -> NonZeroU64 {
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

    fn seq(n: u64) -> NonZeroU64 {
        NonZeroU64::new(n).expect("non-zero literal")
    }

    #[test]
    fn new_constructs_with_given_fields() {
        let id = sample_id(42);
        let cp = ProjectionCheckpoint::new(id, "counter_view", seq(7));
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
        let cp = ProjectionCheckpoint::new(id, "orders_view", seq(123));
        assert_eq!(cp.aggregate_id(), id);
        assert_eq!(cp.projection_name(), "orders_view");
        assert_eq!(cp.last_sequence(), seq(123));
    }

    #[test]
    fn clone_eq_round_trip() {
        let id = sample_id(1);
        let cp = ProjectionCheckpoint::new(id, "view", seq(1));
        let cloned = cp.clone();
        assert_eq!(cp, cloned);
        let other = ProjectionCheckpoint::new(id, "view", seq(2));
        assert_ne!(cp, other);
        let other2 = ProjectionCheckpoint::new(id, "view2", seq(1));
        assert_ne!(cp, other2);
    }
}
