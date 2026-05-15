use std::future::Future;

use crate::aggregate::HandleCommand;
use crate::aggregate_id::AggregateId;
use crate::command::Command;
use crate::correlation::CorrelationContext;
use crate::error::{BusError, CreateResult, DispatchResult};
use crate::event::{DomainEvent, EventEnvelope};

/// The internal command routing and execution mechanism.
/// (CHE-0005 R1: port bound to single aggregate type; CHE-0013 R1:
/// separate create from dispatch; CHE-0018 R2: async RPITIT;
/// CHE-0025 R1–R2: RPITIT, no dyn Future allocation.)
///
/// The bus performs the actual work:
/// 1. Load the aggregate from the event store (replay via apply).
/// 2. Call `HandleCommand::handle()` with the command.
/// 3. Persist the produced events (store creates envelopes).
/// 4. Publish envelopes to the event bus for fan-out.
///
/// Bound to a single aggregate type via the `Aggregate` associated
/// type. The compiler proves that commands, events, store, and bus
/// all agree on the same aggregate — no cross-aggregate ID/type
/// mismatches are possible.
///
/// The [`CommandGateway`](crate::CommandGateway) wraps the bus and adds
/// cross-cutting middleware.
///
/// # Distributed failure model (COM-0025:R1)
///
/// Three failure modes apply at trait level rather than per-method.
/// The four method-scoped semantics (timeout, cancellation, retry,
/// duplicate-delivery) are documented on [`create`](Self::create)
/// and [`dispatch`](Self::dispatch).
///
/// ## Crash
///
/// Implementors MUST guarantee that a process kill mid-call leaves the
/// system in a consistent state. The bus's unit-of-work is "persist
/// events, then publish": a crash before persistence MUST result in
/// zero observable side effects; a crash after persistence but before
/// publication MUST leave the events durably stored, with publication
/// recovered by downstream catch-up subscribers (see
/// [`EventBus`](crate::EventBus)'s tracking-style guarantee). The bus
/// MUST NOT publish events that have not been durably persisted.
///
/// ## Replay
///
/// Commands are NOT replayable (COM-0025:R1). [`Command`](crate::Command)
/// represents one-time intent and is consumed on handling (moved, not
/// borrowed). The bus therefore exposes no `replay` primitive; historic
/// state is reconstructed by replaying *events* via
/// [`EventStore::load`](crate::EventStore::load), not by re-dispatching
/// commands.
///
/// ## Recovery
///
/// Implementor-level recovery (drain of in-flight commands during
/// shutdown, dead-letter handling for poison commands, interceptor
/// failure handling) lives outside the port surface. Each concrete
/// `CommandBus` MUST document its recovery procedure in its own
/// rustdoc.
///
/// # Doctest — contract type-check
///
/// The trait is async via RPITIT (CHE-0018:R2) and cherry-pit-core
/// declares no async runtime (CHE-0029:R4); the doctest constructs the
/// futures without awaiting them, verifying that the trait's surface
/// type-checks.
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
    /// The bus:
    /// 1. Creates a `Default` aggregate.
    /// 2. Handles the command (producing events).
    /// 3. Persists via `EventStore::create` (store assigns the ID).
    /// 4. Publishes envelopes to the event bus.
    ///
    /// Returns the store-assigned [`AggregateId`] and produced envelopes.
    ///
    /// # Timeout
    ///
    /// The returned future MAY resolve to
    /// [`DispatchError::Infrastructure`](crate::DispatchError::Infrastructure)
    /// after the implementor's deadline elapses. Timeouts are
    /// [`Retryable`](crate::ErrorCategory::Retryable) (CHE-0046:R1) but
    /// see "Retry" below — duplicate-creation hazard applies.
    ///
    /// # Cancellation
    ///
    /// Dropping the returned future cancels the in-flight create.
    /// Because `create` performs both persistence and publication, an
    /// implementor MUST NOT publish events that were not first durably
    /// persisted. Cancellation between persistence and publication
    /// MUST be recoverable by downstream catch-up
    /// (see [`EventBus`](crate::EventBus)).
    ///
    /// # Retry
    ///
    /// `create` is NOT idempotent — see
    /// [`EventStore::create`](crate::EventStore::create)'s identical
    /// hazard. Naive retry MAY produce duplicate aggregates with
    /// distinct IDs. Safe retry requires an
    /// [`IdempotencyKey`](crate::IdempotencyKey) carried on the command
    /// (CHE-0041); implementors MAY use it to short-circuit duplicate
    /// `create` calls. Without an idempotency key, callers SHOULD treat
    /// [`Retryable`](crate::ErrorCategory::Retryable) errors from
    /// `create` as fatal at the caller layer.
    ///
    /// # Duplicate delivery
    ///
    /// Idempotency is the command-layer concern: a duplicate `create`
    /// invocation with the same [`IdempotencyKey`](crate::IdempotencyKey)
    /// MUST be detected by the implementor and de-duplicated (CHE-0041);
    /// without an idempotency key, the duplicate creates a fresh
    /// aggregate. The bus cannot infer duplicates structurally because
    /// no aggregate ID exists yet.
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
    /// Implementors manage the unit of work: if event persistence
    /// fails, no events are published. If optimistic concurrency
    /// is violated, a `ConcurrencyConflict` error is returned.
    ///
    /// # Timeout
    ///
    /// The returned future MAY resolve to
    /// [`DispatchError::Infrastructure`](crate::DispatchError::Infrastructure)
    /// after the implementor's deadline elapses. Timeouts are
    /// [`Retryable`](crate::ErrorCategory::Retryable) (CHE-0046:R1).
    ///
    /// # Cancellation
    ///
    /// Dropping the returned future cancels the load/handle/persist
    /// pipeline. The unit-of-work guarantee binds under drop: a
    /// cancelled dispatch either persists-and-publishes-all or
    /// persists-and-publishes-none. Implementors MUST NOT publish
    /// events that did not also durably persist.
    ///
    /// # Retry
    ///
    /// `dispatch` is safe to retry on
    /// [`DispatchError::ConcurrencyConflict`](crate::DispatchError::ConcurrencyConflict)
    /// (the typical pattern: reload, re-handle, re-persist) and on
    /// [`DispatchError::Infrastructure`](crate::DispatchError::Infrastructure).
    /// Both are [`Retryable`](crate::ErrorCategory::Retryable)
    /// (CHE-0046:R1).
    /// [`DispatchError::Rejected`](crate::DispatchError::Rejected) and
    /// [`DispatchError::AggregateNotFound`](crate::DispatchError::AggregateNotFound)
    /// are [`Terminal`](crate::ErrorCategory::Terminal) — MUST NOT be
    /// retried.
    ///
    /// # Duplicate delivery
    ///
    /// Duplicate command delivery is handled via the command's
    /// [`IdempotencyKey`](crate::IdempotencyKey) (CHE-0041) and the
    /// store's `expected_sequence` optimistic-concurrency check (see
    /// [`EventStore::append`](crate::EventStore::append)). A
    /// re-dispatched command with the same `IdempotencyKey` MUST be
    /// short-circuited to its prior outcome; without an idempotency
    /// key, a retried dispatch will hit `ConcurrencyConflict` on the
    /// stale `expected_sequence` and be rejected, preserving the
    /// at-most-once effect.
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
/// (CHE-0005 R1: port bound to single event type; CHE-0005 R3:
/// cross-context communication via event subscriptions;
/// CHE-0018 R2: async RPITIT; CHE-0025 R1–R2: RPITIT.)
///
/// After the `CommandBus` persists new events via the
/// [`EventStore`](crate::EventStore), it publishes them through the
/// `EventBus` for fan-out to Policies, Projections, and external
/// integrations.
///
/// Each bus instance is bound to a single domain event type. In a
/// distributed system, each bounded context has its own `EventBus`
/// publishing its aggregate's events (e.g. to a dedicated NATS
/// subject). Cross-context consumption uses separate subscriptions
/// typed to the foreign event type.
///
/// This is a secondary (driven) port. Concrete implementations
/// (in-memory synchronous fan-out, NATS-backed, channel-based) live
/// in infrastructure crates.
///
/// # Distributed failure model (COM-0025:R1)
///
/// Four failure modes apply at trait level. The three method-scoped
/// semantics (timeout, cancellation, retry, duplicate-delivery) are
/// documented on [`publish`](Self::publish).
///
/// ## Crash
///
/// Implementors MUST guarantee that a crash mid-`publish` cannot lose
/// events that were durably persisted by the
/// [`EventStore`](crate::EventStore) but not yet delivered to
/// consumers. Because the `CommandBus` always persists before
/// publishing, publication is at-least-once: a crash before the
/// publish call completes is recoverable by downstream tracking-style
/// processors which can read from
/// [`EventStore::load`](crate::EventStore::load) to catch up. The bus
/// itself MUST NOT promise exactly-once delivery — see
/// "Duplicate delivery" on [`publish`](Self::publish).
///
/// ## Replay
///
/// The `EventBus` is NOT a replay source (COM-0025:R1). Historical
/// events MUST be re-derived by calling
/// [`EventStore::load`](crate::EventStore::load), which is the
/// designated replay primitive. Catch-up subscribers consume the
/// bus for live events and the store for backfill; the bus exposes
/// no `replay()` primitive and implementors MUST NOT add one without
/// changing this trait.
///
/// ## Recovery
///
/// Implementor-level recovery procedures (subscription replay from a
/// checkpoint via [`ProjectionCheckpoint`](crate::ProjectionCheckpoint),
/// dead-letter handling for poison events, broker reconnection on
/// transport failure) live outside the port surface. Each concrete
/// `EventBus` MUST document its recovery procedure in its own
/// rustdoc, alongside its delivery and ordering guarantees.
///
/// # Doctest — contract type-check
///
/// The trait is async via RPITIT (CHE-0018:R2) and cherry-pit-core has
/// no async runtime (CHE-0029:R4); the doctest constructs the future
/// without awaiting it to verify the surface type-checks.
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
    /// Called by the `CommandBus` after events are successfully
    /// persisted. Because events are already safely stored, publication
    /// failure is non-fatal — tracking-style processors can catch up
    /// on missed publications.
    ///
    /// # Timeout
    ///
    /// The returned future MAY resolve to a
    /// [`BusError`](crate::BusError) carrying an
    /// implementation-defined timeout error after the implementor's
    /// deadline elapses. [`BusError`](crate::BusError) is always
    /// [`Retryable`](crate::ErrorCategory::Retryable) (CHE-0046:R1) —
    /// and trivially safe to retry since the events are durably
    /// persisted (see trait-level "Crash" and "Replay").
    ///
    /// # Cancellation
    ///
    /// Dropping the returned future cancels in-flight publication.
    /// Because events are already persisted and the
    /// [`EventStore`](crate::EventStore) is the source of truth,
    /// cancellation MAY leave some, all, or none of the events
    /// delivered to consumers — downstream tracking-style processors
    /// MUST tolerate this and catch up from the store.
    ///
    /// # Retry
    ///
    /// `publish` is safe to retry on any
    /// [`BusError`](crate::BusError) — all `BusError`s are
    /// [`Retryable`](crate::ErrorCategory::Retryable). Retries MAY
    /// cause duplicate delivery — see below.
    /// Retrying publication is never required for correctness because
    /// the store is the source of truth; the bus is a fan-out channel,
    /// not the durable record.
    ///
    /// # Duplicate delivery
    ///
    /// Publication is at-least-once. Implementors MAY deliver an event
    /// more than once on retry, on reconnection, or as part of
    /// catch-up. Consumers MUST be idempotent — typically by tracking
    /// `(aggregate_id, sequence)` via
    /// [`ProjectionCheckpoint`](crate::ProjectionCheckpoint), which
    /// gives consumers the structural key needed to detect re-delivery.
    fn publish(
        &self,
        events: &[EventEnvelope<Self::Event>],
    ) -> impl Future<Output = Result<(), BusError>> + Send;
}
