use std::error::Error;

use crate::command::Command;
use crate::event::DomainEvent;

/// The aggregate root — the consistency and transactional boundary.
///
/// An aggregate reconstructs its state by replaying events, and is the
/// only place business invariants are enforced. It only knows how to
/// apply events; command handling is added via [`HandleCommand`]
/// (CHE-0004: EDA + DDD + hexagonal).
///
/// `Default` is the zero-state starting point (CHE-0012). No `id()`
/// method: identity is infrastructure-owned — the store assigns
/// [`AggregateId`](crate::AggregateId) on creation; domain logic
/// needing its own ID stores one as a field set during the first
/// event's `apply` (CHE-0020). Full replay, no snapshotting, no
/// `Serialize`/`Deserialize` bounds on the trait (CHE-0037).
///
/// Single-writer: each instance is owned by one process, no
/// distributed coordination needed. Optimistic concurrency
/// (`expected_sequence` on
/// [`EventStore::append`](crate::EventStore::append)) is
/// defense-in-depth within the single writer (CHE-0006).
///
/// # Examples
///
/// ```
/// use cherry_pit_core::{Aggregate, DomainEvent};
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// enum CounterEvent { Incremented }
///
/// impl DomainEvent for CounterEvent {
///     fn event_type(&self) -> &'static str { "counter.incremented" }
/// }
///
/// #[derive(Default)]
/// struct Counter { count: u32 }
///
/// impl Aggregate for Counter {
///     type Event = CounterEvent;
///     fn apply(&mut self, event: &CounterEvent) {
///         match event {
///             CounterEvent::Incremented => self.count += 1,
///         }
///     }
/// }
///
/// let mut agg = Counter::default(); // CHE-0012: zero state
/// agg.apply(&CounterEvent::Incremented);
/// assert_eq!(agg.count, 1);
/// ```
pub trait Aggregate: Default + Send + Sync + 'static {
    /// Events this aggregate produces and is reconstructed from.
    type Event: DomainEvent;

    /// Apply an event to update internal state.
    ///
    /// Must be deterministic and total — it must never fail. This
    /// method is called during state reconstruction (replaying history)
    /// and after handling new commands. If apply could fail, the
    /// aggregate's history would become unloadable.
    fn apply(&mut self, event: &Self::Event);
}

/// Command handling is a separate trait so each command→aggregate
/// pair is verified at compile time (CHE-0008: pure command handling;
/// CHE-0015: error type per command).
///
/// An aggregate implements `HandleCommand` once per command type it
/// accepts. The compiler guarantees exhaustive handling — no forgotten
/// command, no runtime downcasting, no match-arm gaps.
///
/// `handle` takes `self` by shared reference — the aggregate inspects
/// state but does not mutate directly; state changes happen only
/// through events returned by `handle`, then applied via `apply`
/// (CHE-0008:R1). It takes ownership of the command, consuming its
/// one-time intent (CHE-0008:R2). `Error` is an associated type on
/// `HandleCommand`, not on `Aggregate`, since different commands may
/// have different error types (CHE-0015:R1).
///
/// # Examples
///
/// ```
/// use cherry_pit_core::{Aggregate, HandleCommand, Command, DomainEvent};
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// enum CounterEvent { Incremented }
/// impl DomainEvent for CounterEvent {
///     fn event_type(&self) -> &'static str { "counter.incremented" }
/// }
///
/// #[derive(Default)]
/// struct Counter { count: u32 }
/// impl Aggregate for Counter {
///     type Event = CounterEvent;
///     fn apply(&mut self, event: &CounterEvent) {
///         match event { CounterEvent::Incremented => self.count += 1 }
///     }
/// }
///
/// struct Increment;
/// impl Command for Increment {}
///
/// impl HandleCommand<Increment> for Counter {
///     type Error = std::convert::Infallible;
///     fn handle(&self, _cmd: Increment) -> Result<Vec<CounterEvent>, Self::Error> {
///         Ok(vec![CounterEvent::Incremented])
///     }
/// }
///
/// let agg = Counter::default();
/// let events = agg.handle(Increment).unwrap();
/// assert_eq!(events.len(), 1);
/// ```
pub trait HandleCommand<C: Command>: Aggregate {
    /// Domain-specific error for invariant violations.
    type Error: Error + Send + Sync;

    /// Handle a command against the current state.
    ///
    /// Returns zero or more events on success. Zero events means the
    /// command was accepted but no state change occurred (idempotent).
    /// Must be pure — no I/O, no side effects.
    ///
    /// # Errors
    ///
    /// Returns the domain-specific error type when a business invariant
    /// is violated and the command must be rejected.
    fn handle(&self, cmd: C) -> Result<Vec<Self::Event>, Self::Error>;
}

#[cfg(test)]
mod tests {
    //! Runtime coverage for CHE-0009 R1–R2: `apply` is infallible (returns `()`)
    //! and deterministic — replaying the same event sequence reconstructs the
    //! same state.
    //!
    //! Also exercises CHE-0010's `DomainEvent` supertrait bounds
    //! (`Serialize + DeserializeOwned + Clone + Send + Sync + 'static`) and
    //! CHE-0012's `Default = zero state` rule by starting from `Counter::default()`.

    use serde::{Deserialize, Serialize};

    use super::Aggregate;
    use crate::event::DomainEvent;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum CounterEvent {
        Incremented(i64),
    }

    impl DomainEvent for CounterEvent {
        fn event_type(&self) -> &'static str {
            "counter.incremented"
        }
    }

    #[derive(Default)]
    struct Counter {
        value: i64,
    }

    impl Aggregate for Counter {
        type Event = CounterEvent;
        fn apply(&mut self, event: &CounterEvent) {
            match event {
                CounterEvent::Incremented(n) => self.value += n,
            }
        }
    }

    #[test]
    fn apply_replays_event_stream_to_final_state() {
        let mut agg = Counter::default();
        assert_eq!(agg.value, 0, "default is zero state per CHE-0012 R1");
        let events = [
            CounterEvent::Incremented(1),
            CounterEvent::Incremented(2),
            CounterEvent::Incremented(3),
        ];
        for ev in &events {
            agg.apply(ev);
        }
        assert_eq!(agg.value, 6);
    }

    #[test]
    fn apply_is_deterministic_under_replay() {
        let events = [
            CounterEvent::Incremented(10),
            CounterEvent::Incremented(-3),
            CounterEvent::Incremented(7),
        ];
        let mut a = Counter::default();
        for ev in &events {
            a.apply(ev);
        }
        let mut b = Counter::default();
        for ev in &events {
            b.apply(ev);
        }
        assert_eq!(a.value, b.value);
        assert_eq!(a.value, 14);
    }

    #[test]
    fn apply_returns_unit_infallibly() {
        let mut agg = Counter::default();
        let (): () = agg.apply(&CounterEvent::Incremented(5));
        assert_eq!(agg.value, 5);
    }
}
