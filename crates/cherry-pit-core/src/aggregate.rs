use std::error::Error;

use crate::command::Command;
use crate::event::DomainEvent;

/// The aggregate root — the consistency and transactional boundary.
///
/// An aggregate reconstructs its state by replaying events. It is the
/// only place where business invariants are enforced. The aggregate
/// itself only knows how to apply events — command handling is added
/// via the [`HandleCommand`] trait.
/// (CHE-0004: EDA + DDD + hexagonal; CHE-0009 R1–R2: infallible apply;
/// CHE-0012 R1–R3: `Default` = zero state, no constructor args;
/// CHE-0020 R1: no `id()` method; CHE-0037 R1–R2: full replay,
/// no Serialize/Deserialize on Aggregate trait.)
///
/// # Single-writer design
///
/// Cherry-pit assumes single-writer aggregates: each aggregate
/// instance is owned by exactly one process. No distributed
/// coordination is needed — the owning process serializes commands
/// internally. Optimistic concurrency (`expected_sequence` on
/// [`EventStore::append`](crate::EventStore::append)) serves as
/// defense-in-depth within the single writer.
/// (CHE-0006: single-writer per aggregate.)
///
/// # Design rationale
///
/// - `Default` — the aggregate starts as a blank slate. State is built
///   entirely by replaying events through `apply`.
///   (CHE-0012: aggregate default = zero state.)
/// - No `id()` method — aggregate identity is managed by the
///   infrastructure layer (event store, repository). The store assigns
///   [`AggregateId`](crate::AggregateId) values on creation. If the
///   domain logic needs its own ID, it stores it as a field set during
///   the first event's `apply`.
///   (CHE-0020: infrastructure-owned identity.)
///
/// # Examples
///
/// ```
/// use cherry_pit_core::{Aggregate, DomainEvent};
/// use pardosa_encoding::Encode;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// enum CounterEvent { Incremented }
///
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
/// pair is verified at compile time.
/// (CHE-0008: pure command handling; CHE-0015: error type per command.)
///
/// An aggregate implements `HandleCommand` once per command type it
/// accepts. The compiler guarantees exhaustive handling — you cannot
/// forget to implement a command. No runtime downcasting, no
/// match-arm gaps.
///
/// # Design rationale
///
/// - `handle` takes `self` by shared reference — the aggregate inspects
///   its current state but does not mutate directly. State changes
///   happen only through events returned by `handle`, then applied
///   via `apply`. (CHE-0008 R1/R2: `&self`, consumes `C` by value.)
/// - `handle` takes ownership of the command — a command represents
///   one-time intent. After handling, it is consumed.
/// - `Error` is an associated type on `HandleCommand`, not on
///   `Aggregate` — different commands may have different error types.
///   (CHE-0015 R1: error type per command.)
///
/// # Examples
///
/// ```
/// use cherry_pit_core::{Aggregate, HandleCommand, Command, DomainEvent};
/// use pardosa_encoding::Encode;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// enum CounterEvent { Incremented }
/// impl DomainEvent for CounterEvent {
///     fn event_type(&self) -> &'static str { "counter.incremented" }
/// }
/// // CHE-0064:R2 — hand-rolled Encode per PAR-0024:R5.
/// impl Encode for CounterEvent {
///     fn encode(&self, out: &mut Vec<u8>) {
///         match self { CounterEvent::Incremented => out.push(0u8) }
///     }
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

    // CHE-0064:R2 — hand-rolled Encode (no derive) per PAR-0024:R5.
    impl pardosa_encoding::Encode for CounterEvent {
        fn encode(&self, out: &mut Vec<u8>) {
            match self {
                CounterEvent::Incremented(v) => {
                    out.push(0u8);
                    v.encode(out);
                }
            }
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
        // Same event stream → same final state, twice.
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
        // CHE-0009 R1: trait signature returns () — captured via type-ascription
        // closure; if `apply` ever returned a Result, this would fail to compile.
        let mut agg = Counter::default();
        let (): () = agg.apply(&CounterEvent::Incremented(5));
        assert_eq!(agg.value, 5);
    }
}
