use crate::event::{DomainEvent, EventEnvelope};

/// A projection folds events into a query-optimized read model.
/// (CHE-0009: infallible `apply`.)
///
/// Projections are the read side of CQRS. They consume events and
/// build denormalized views optimized for specific query patterns.
///
/// # Design rationale
///
/// - `Default` — projections can be rebuilt from scratch at any time
///   by replaying the full event history. No migration story needed.
/// - Receives `EventEnvelope` — projections often use metadata
///   (timestamp for time-based views, sequence for ordering).
/// - No error return — projection application must be total. If a
///   projection cannot handle an event, that is a bug, not a runtime
///   error. (CHE-0009 R1: infallible apply.)
///
/// # Examples
///
/// ```
/// use cherry_pit_core::{Projection, DomainEvent, EventEnvelope};
/// use pardosa_encoding::Encode;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// enum CounterEvent { Incremented }
/// impl DomainEvent for CounterEvent {
///     fn event_type(&self) -> &'static str { "counter.incremented" }
/// }
/// // CHE-0064:R2 — hand-rolled Encode (no derive) per PAR-0024:R5.
/// impl Encode for CounterEvent {
///     fn encode(&self, out: &mut Vec<u8>) {
///         match self { CounterEvent::Incremented => out.push(0u8) }
///     }
/// }
///
/// #[derive(Default)]
/// struct CounterView { total: u64 }
///
/// impl Projection for CounterView {
///     type Event = CounterEvent;
///     fn apply(&mut self, _event: &EventEnvelope<CounterEvent>) {
///         self.total += 1;
///     }
/// }
/// ```
pub trait Projection: Default + Send + Sync + 'static {
    /// The event type this projection consumes.
    type Event: DomainEvent;

    /// Apply an event to update the read model.
    ///
    /// Must be deterministic and total. A projection can always be
    /// rebuilt from scratch by replaying all events.
    fn apply(&mut self, event: &EventEnvelope<Self::Event>);
}
