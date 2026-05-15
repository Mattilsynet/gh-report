//! Projection composition primitives local to cherry-pit-agent per
//! CHE-0051:R5.
//!
//! Two pieces:
//!
//! 1. [`ProjectionDriverExt`] — extension trait declared inside this
//!    crate adding the missing `apply_one` per-event primitive on top
//!    of `cherry_pit_projection::ProjectionDriver`. CHE-0051:R5
//!    states verbatim: *"the missing `apply_one` per-event primitive
//!    is added by `ProjectionDriverExt`, an extension trait declared
//!    inside cherry-pit-agent that imports the existing
//!    `ProjectionDriver` shape from cherry-pit-projection without
//!    editing it (CHE-0048 stays untouched per FOCUS.md §8)"*. C14
//!    forbids editing CHE-0048 source; the extension trait at the
//!    agent boundary is the sanctioned escape hatch.
//!
//! 2. [`ProjectionDriverTuple`] — heterogeneous tuple impls covering
//!    arities 1..=2 of `(D1, D2, …)` of `ProjectionDriver`-style
//!    drivers. v0.1 ships `(D1,)` and `(D1, D2)` only, which covers
//!    the FOCUS.md §4 step 5 ergonomic-benchmark gate (2-aggregate
//!    composition). Higher arities are a `// FOLLOW-UP S7` extension.
//!
//! Per CHE-0048 the `ProjectionDriver<P, S>` shape is a struct
//! parameterised on a single `P: Projection` and a typed
//! `S: EventStore<Event = P::Event>` — there is no `Box<dyn>`. The
//! tuple impls preserve that discipline by being generic over each
//! `(Pn, Sn)` pair.

use cherry_pit_core::{AggregateId, CorrelationContext, EventEnvelope, EventStore, Projection};
use cherry_pit_projection::{ProjectionDriver, ProjectionResult};

/// Extension trait adding per-event projection application on top of
/// `cherry_pit_projection::ProjectionDriver`'s replay-only surface.
///
/// Per CHE-0051:R5: the projection crate's `ProjectionDriver` ships
/// `replay`, `project_to_file`, `rebuild_file` — all stream-level
/// operations. The dispatch loop needs a single-envelope entry point
/// for incremental projection updates inside `App`'s publish handler.
/// `apply_one` provides that entry point without modifying CHE-0048's
/// driver (C14).
///
/// The default impl simply delegates to `Projection::apply` on a
/// caller-owned mutable projection — the driver itself is stateless
/// w.r.t. the live projection (it owns only the store binding). This
/// preserves single-writer-per-aggregate (CHE-0006) by leaving the
/// projection state where the consumer chooses to keep it.
pub trait ProjectionDriverExt<P, S>
where
    P: Projection,
    S: EventStore<Event = P::Event>,
{
    /// Apply a single event envelope to a caller-owned projection.
    ///
    /// Synchronous per CHE-0018:R1 — `Projection::apply` is sync.
    fn apply_one(&self, projection: &mut P, envelope: &EventEnvelope<P::Event>) {
        projection.apply(envelope);
    }

    /// Replay the entire stream into a fresh `P::default()`.
    ///
    /// Pass-through to [`ProjectionDriver::replay`] for ergonomic
    /// access through the extension trait surface.
    ///
    /// # Errors
    ///
    /// Surfaces [`cherry_pit_projection::ProjectionError`] from
    /// the underlying driver.
    fn replay_all(
        &self,
        aggregate_id: AggregateId,
        correlation: &CorrelationContext,
    ) -> impl std::future::Future<Output = ProjectionResult<P>> + Send;
}

impl<P, S> ProjectionDriverExt<P, S> for ProjectionDriver<P, S>
where
    P: Projection,
    S: EventStore<Event = P::Event>,
{
    fn replay_all(
        &self,
        aggregate_id: AggregateId,
        correlation: &CorrelationContext,
    ) -> impl std::future::Future<Output = ProjectionResult<P>> + Send {
        self.replay(aggregate_id, correlation)
    }
}

/// Heterogeneous fixed-arity tuple of `ProjectionDriver` instances per
/// CHE-0051:R5.
///
/// Each tuple element is a distinct `ProjectionDriver<Pn, Sn>` where
/// every `(Pn, Sn)` pair is independent — the tuple shape preserves
/// per-projection type discipline (no `Box<dyn Projection>`,
/// CHE-0005:R1).
///
/// v0.1 ships arities **1 and 2** which suffice for the FOCUS.md §4
/// step 5 ergonomic-benchmark gate (2-aggregate composition). Higher
/// arities up to ~8 are tracked as a `// FOLLOW-UP S7` extension
/// gated by the ergonomic benchmark — if the benchmark passes at
/// arity 2 with comfortable headroom, macro-expansion to arity 8 is
/// purely mechanical and lands in S7.
///
/// The trait is currently a marker — driver-level operations
/// (`apply_one`, `replay_all`) are exercised on the individual
/// elements via destructuring or pattern matching at the consumer
/// site. A future extension may add `apply_to_all(&mut state_tuple,
/// envelope)` once the ergonomic benchmark reveals the call shape
/// the consumer actually wants.
pub trait ProjectionDriverTuple {
    /// Number of projections in the tuple. Const-folded at the call
    /// site so consumers can `assert!(<T as ProjectionDriverTuple>::ARITY == 2)`.
    const ARITY: usize;
}

impl<P1, S1> ProjectionDriverTuple for (ProjectionDriver<P1, S1>,)
where
    P1: Projection,
    S1: EventStore<Event = P1::Event>,
{
    const ARITY: usize = 1;
}

impl<P1, S1, P2, S2> ProjectionDriverTuple for (ProjectionDriver<P1, S1>, ProjectionDriver<P2, S2>)
where
    P1: Projection,
    S1: EventStore<Event = P1::Event>,
    P2: Projection,
    S2: EventStore<Event = P2::Event>,
{
    const ARITY: usize = 2;
}

// FOLLOW-UP S7: extend `ProjectionDriverTuple` impls to arity 8 via a
// declarative macro once the ergonomic benchmark validates the 2-arity
// shape. The brief ("fixed-arity (works to ~8)") sets 8 as the
// ceiling; v0.1 only needs 2 for the wiring-vs-domain-LOC gate.

/// Marker for "no projections wired" — used when `App::new` is called
/// without projection parameters. The unit type implements
/// [`ProjectionDriverTuple`] with arity 0 so an empty composition is
/// expressible without special-casing in `App`.
impl ProjectionDriverTuple for () {
    const ARITY: usize = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use cherry_pit_core::{
        CorrelationContext, DomainEvent, EventEnvelope, StoreCreateResult, StoreError,
    };
    use serde::{Deserialize, Serialize};
    use std::num::NonZeroU64;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    enum CounterEvent {
        Incremented,
    }

    impl DomainEvent for CounterEvent {
        fn event_type(&self) -> &'static str {
            "counter.incremented"
        }
    }

    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    struct CounterView {
        total: u64,
    }

    impl Projection for CounterView {
        type Event = CounterEvent;
        fn apply(&mut self, _event: &EventEnvelope<Self::Event>) {
            self.total += 1;
        }
    }

    struct UnusedStore;

    impl EventStore for UnusedStore {
        type Event = CounterEvent;

        async fn load(
            &self,
            _id: AggregateId,
        ) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
            Ok(Vec::new())
        }

        async fn create(
            &self,
            _events: Vec<Self::Event>,
            _context: CorrelationContext,
        ) -> StoreCreateResult<Self::Event> {
            Err(StoreError::Infrastructure("unused".into()))
        }

        async fn append(
            &self,
            _id: AggregateId,
            _expected_sequence: NonZeroU64,
            _events: Vec<Self::Event>,
            _context: CorrelationContext,
        ) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
            Err(StoreError::Infrastructure("unused".into()))
        }
    }

    fn envelope() -> EventEnvelope<CounterEvent> {
        EventEnvelope::new(
            uuid::Uuid::now_v7(),
            AggregateId::new(NonZeroU64::new(1).unwrap()),
            NonZeroU64::new(1).unwrap(),
            jiff::Timestamp::now(),
            None,
            None,
            CounterEvent::Incremented,
        )
        .unwrap()
    }

    #[test]
    fn apply_one_delegates_to_projection_apply() {
        let driver = ProjectionDriver::<CounterView, _>::new(UnusedStore);
        let mut view = CounterView::default();
        driver.apply_one(&mut view, &envelope());
        driver.apply_one(&mut view, &envelope());
        assert_eq!(view.total, 2);
    }

    #[test]
    fn tuple_arity_0() {
        assert_eq!(<() as ProjectionDriverTuple>::ARITY, 0);
    }

    #[test]
    fn tuple_arity_1() {
        type T = (ProjectionDriver<CounterView, UnusedStore>,);
        assert_eq!(<T as ProjectionDriverTuple>::ARITY, 1);
    }

    #[test]
    fn tuple_arity_2() {
        type T = (
            ProjectionDriver<CounterView, UnusedStore>,
            ProjectionDriver<CounterView, UnusedStore>,
        );
        assert_eq!(<T as ProjectionDriverTuple>::ARITY, 2);
    }
}
