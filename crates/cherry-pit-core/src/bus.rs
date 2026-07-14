use std::future::Future;

use crate::aggregate::HandleCommand;
use crate::aggregate_id::AggregateId;
use crate::command::Command;
use crate::correlation::CorrelationContext;
use crate::error::{BusError, CreateResult, DispatchResult};
use crate::event::{DomainEvent, EventEnvelope};

/// Command routing port: load aggregate, handle command, persist events, publish.
///
/// Bound to a single aggregate type via `Aggregate` (CHE-0005:R1). Create
/// and dispatch are separate operations (CHE-0013:R1), async via RPITIT
/// with no `dyn Future` allocation (CHE-0018:R2, CHE-0025:R1–R2). See
/// [`CommandGateway`](crate::CommandGateway) for cross-cutting middleware.
///
/// # Distributed failure model (COM-0025:R1)
///
/// Timeout/cancellation/retry/duplicate-delivery are documented per-method
/// on [`create`](Self::create) and [`dispatch`](Self::dispatch).
///
/// - **Crash**: persist-then-publish; recovered via downstream catch-up
///   ([`EventBus`](crate::EventBus)) if a crash lands between the two.
/// - **Replay**: commands are one-time intent, not replayable — state is
///   reconstructed from events via [`EventStore::load`](crate::EventStore::load).
/// - **Recovery**: implementor-specific; each concrete `CommandBus`
///   documents its own procedure.
///
/// The doctest below type-checks the RPITIT surface without awaiting
/// (no async runtime in this crate, CHE-0029:R4).
///
/// ```
/// use cherry_pit_core::{AggregateId, Command, CommandBus, CorrelationContext, HandleCommand};
///
/// fn _type_check<B, C>(
///     bus: &B,
///     id: AggregateId,
///     cmd: C,
///     ctx: CorrelationContext,
/// ) where
///     B: CommandBus,
///     B::Aggregate: HandleCommand<C>,
///     C: Command + Clone,
/// {
///     let _create = bus.create(cmd.clone(), ctx.clone());
///     let _dispatch = bus.dispatch(id, cmd, ctx);
/// }
/// ```
pub trait CommandBus: Send + Sync + 'static {
    /// The single aggregate type this bus manages.
    type Aggregate: crate::aggregate::Aggregate;

    /// Create a new aggregate — full lifecycle without a known ID.
    ///
    /// Handles the command against a `Default` aggregate, persists via
    /// `EventStore::create` (assigns the ID), publishes, and returns the
    /// [`AggregateId`] plus envelopes.
    ///
    /// # Timeout
    ///
    /// MAY resolve to
    /// [`DispatchError::Infrastructure`](crate::DispatchError::Infrastructure);
    /// [`Retryable`](crate::ErrorCategory::Retryable) (CHE-0046:R1) — see
    /// Retry for the duplicate-creation hazard.
    ///
    /// # Cancellation
    ///
    /// MUST NOT publish events not first durably persisted; recoverable
    /// via downstream catch-up (see [`EventBus`](crate::EventBus)).
    ///
    /// # Retry
    ///
    /// NOT idempotent (same hazard as
    /// [`EventStore::create`](crate::EventStore::create)); safe retry
    /// requires an [`IdempotencyKey`](crate::IdempotencyKey) (CHE-0041) —
    /// without one, treat retryable errors as fatal.
    ///
    /// # Duplicate delivery
    ///
    /// A duplicate `create` with the same
    /// [`IdempotencyKey`](crate::IdempotencyKey) MUST be deduplicated
    /// (CHE-0041); without a key it creates a fresh aggregate.
    fn create<C>(
        &self,
        cmd: C,
        context: CorrelationContext,
    ) -> impl Future<Output = CreateResult<Self::Aggregate, C>> + Send
    where
        Self::Aggregate: HandleCommand<C>,
        C: Command;

    /// Load, handle, persist, publish — the full command lifecycle.
    ///
    /// If persistence fails, no events publish; an optimistic-concurrency
    /// violation returns `ConcurrencyConflict`.
    ///
    /// # Timeout
    ///
    /// MAY resolve to
    /// [`DispatchError::Infrastructure`](crate::DispatchError::Infrastructure);
    /// [`Retryable`](crate::ErrorCategory::Retryable) (CHE-0046:R1).
    ///
    /// # Cancellation
    ///
    /// The unit-of-work guarantee binds under drop: a cancelled dispatch
    /// either persists-and-publishes-all or persists-and-publishes-none.
    /// MUST NOT publish events that did not also durably persist.
    ///
    /// # Retry
    ///
    /// Safe to retry on
    /// [`DispatchError::ConcurrencyConflict`](crate::DispatchError::ConcurrencyConflict)
    /// and [`DispatchError::Infrastructure`](crate::DispatchError::Infrastructure)
    /// — both [`Retryable`](crate::ErrorCategory::Retryable) (CHE-0046:R1).
    /// [`DispatchError::Rejected`](crate::DispatchError::Rejected) and
    /// [`DispatchError::AggregateNotFound`](crate::DispatchError::AggregateNotFound)
    /// are [`Terminal`](crate::ErrorCategory::Terminal) — MUST NOT retry.
    ///
    /// # Duplicate delivery
    ///
    /// Handled via the command's [`IdempotencyKey`](crate::IdempotencyKey)
    /// (CHE-0041) plus the store's `expected_sequence` check (see
    /// [`EventStore::append`](crate::EventStore::append)) — a re-dispatch
    /// with the same key short-circuits to its prior outcome; otherwise it
    /// hits `ConcurrencyConflict` and is rejected, preserving at-most-once
    /// effect.
    fn dispatch<C>(
        &self,
        id: AggregateId,
        cmd: C,
        context: CorrelationContext,
    ) -> impl Future<Output = DispatchResult<Self::Aggregate, C>> + Send
    where
        Self::Aggregate: HandleCommand<C>,
        C: Command;
}

/// Port for publishing events to downstream consumers.
///
/// Bound to a single event type via `Event` (CHE-0005:R1); cross-context
/// via event subscriptions, not shared stores (CHE-0005:R3), async via
/// RPITIT (CHE-0018:R2, CHE-0025:R1–R2). Called by `CommandBus` after
/// events persist via [`EventStore`](crate::EventStore), for fan-out to
/// Policies, Projections, and external integrations. Secondary (driven)
/// port; concrete implementations live in infrastructure crates.
///
/// # Distributed failure model (COM-0025:R1)
///
/// Timeout/cancellation/retry/duplicate-delivery documented on
/// [`publish`](Self::publish).
///
/// - **Crash**: at-least-once, since `CommandBus` persists before
///   publishing; recovered via catch-up from
///   [`EventStore::load`](crate::EventStore::load).
/// - **Replay**: not a replay source — re-derive via
///   [`EventStore::load`](crate::EventStore::load); implementors MUST NOT
///   add `replay()` without superseding this trait.
/// - **Recovery**: implementor-specific; documented per concrete
///   `EventBus`.
///
/// The doctest below type-checks the RPITIT surface without awaiting.
///
/// ```
/// use cherry_pit_core::{EventBus, EventEnvelope};
///
/// fn _type_check<B: EventBus>(bus: &B, events: &[EventEnvelope<B::Event>]) {
///     let _publish = bus.publish(events);
/// }
/// ```
pub trait EventBus: Send + Sync + 'static {
    /// The single domain event type this bus publishes.
    type Event: DomainEvent;

    /// Publish events to all registered consumers.
    ///
    /// Called by `CommandBus` after events persist; publication failure is
    /// non-fatal — tracking-style processors catch up on missed publications.
    ///
    /// # Timeout
    ///
    /// MAY resolve to a [`BusError`](crate::BusError); always
    /// [`Retryable`](crate::ErrorCategory::Retryable) (CHE-0046:R1) and
    /// safe to retry since events are durably persisted.
    ///
    /// # Cancellation
    ///
    /// MAY leave some, all, or none of the events delivered — consumers
    /// MUST tolerate this and catch up from the
    /// [`EventStore`](crate::EventStore).
    ///
    /// # Retry
    ///
    /// Safe to retry on any [`BusError`](crate::BusError). Never required
    /// for correctness: the store is the source of truth, the bus a
    /// fan-out channel.
    ///
    /// # Duplicate delivery
    ///
    /// At-least-once: an event MAY be delivered more than once. Consumers
    /// MUST be idempotent — typically tracking `(aggregate_id, sequence)`
    /// via [`ProjectionCheckpoint`](crate::ProjectionCheckpoint).
    fn publish(
        &self,
        events: &[EventEnvelope<Self::Event>],
    ) -> impl Future<Output = Result<(), BusError>> + Send;
}
