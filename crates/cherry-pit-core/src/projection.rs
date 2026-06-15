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
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// enum CounterEvent { Incremented }
/// impl DomainEvent for CounterEvent {
///     fn event_type(&self) -> &'static str { "counter.incremented" }
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

/// A statically-wired read-side port over one projection type.
///
/// `ReadPort` resolves consumer-owned query DTOs from already-materialised
/// projection state. Implementations must not load write-side history,
/// dispatch commands, or mutate the projection through this contract.
///
/// # Static wiring
///
/// Associated types bind each implementation to one projection state type, one
/// query type, and one response type. The resolver is an associated function
/// rather than an object method, so type-erased registries such as
/// `Box<dyn ReadPort>` are rejected by the compiler.
pub trait ReadPort {
    /// Projection state read by this port.
    type Projection;

    /// Consumer-owned query DTO.
    type Query;

    /// Consumer-owned response DTO.
    type Response;

    /// Resolve a query from projection state without side effects.
    fn resolve(projection: &Self::Projection, query: Self::Query) -> Self::Response;
}
