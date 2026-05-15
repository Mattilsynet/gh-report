//! Proptest correlation-propagation invariant per S7 §1 + CHE-0051:R6.
//!
//! Property: for any (`envelope_correlation_id`, `event_id`) where
//! `envelope_correlation_id != Some(event_id)` (mitigation #2 — avoid
//! the degenerate equal-uuids case that would let the property pass
//! vacuously), the [`correlation_for`] helper produces a
//! [`CorrelationContext`] whose correlation/causation IDs match
//! the documented mapping:
//!
//! - `Some(c)` → `(correlation = c, causation = event_id)`
//! - `None`    → `(correlation = event_id, causation = event_id)`
//!
//! The helper is the single point of truth for the dispatcher's
//! per-envelope context construction (see `dispatch.rs`); locking its
//! invariants under proptest closes the "G1 is too narrow" risk
//! linus called out at S5 R1.5.

use cherry_pit_agent::correlation_for;
use proptest::prelude::*;

fn uuid_strategy() -> impl Strategy<Value = uuid::Uuid> {
    any::<u128>().prop_map(|n| {
        // Use from_u128 so proptest shrinks toward simpler values.
        // We don't need uuid v7 timestamps for the invariant; any
        // 128-bit identity works.
        uuid::Uuid::from_u128(n.max(1))
    })
}

proptest! {
    #[test]
    fn correlation_for_threads_some_correlation(
        corr in uuid_strategy(),
        event_id in uuid_strategy(),
    ) {
        prop_assume!(corr != event_id);
        let ctx = correlation_for(Some(corr), event_id);
        prop_assert_eq!(ctx.correlation_id(), Some(corr));
        prop_assert_eq!(ctx.causation_id(), Some(event_id));
    }

    #[test]
    fn correlation_for_seeds_root_when_none(event_id in uuid_strategy()) {
        let ctx = correlation_for(None, event_id);
        prop_assert_eq!(ctx.correlation_id(), Some(event_id));
        prop_assert_eq!(ctx.causation_id(), Some(event_id));
    }
}
