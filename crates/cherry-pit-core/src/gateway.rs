use std::future::Future;

use crate::aggregate::HandleCommand;
use crate::aggregate_id::AggregateId;
use crate::command::Command;
use crate::correlation::CorrelationContext;
use crate::error::{CreateResult, DispatchResult};

/// The primary entry point for dispatching commands into the system.
/// (CHE-0005 R1: port bound to single aggregate type; CHE-0018 R2:
/// async RPITIT; CHE-0025 R1–R2: RPITIT, no dyn Future allocation;
/// CHE-0046 R1–R2: retry on Retryable, stop on Terminal.)
///
/// Every primary adapter (webhook listener, REST API poller, scheduled
/// job) and every Policy dispatches commands through the gateway. The
/// gateway is the outermost port on the driving side of the hexagon.
///
/// The gateway adds cross-cutting concerns (interceptors, retry,
/// logging) on top of the [`CommandBus`](crate::CommandBus).
///
/// Bound to a single aggregate type. The compiler verifies that every
/// command dispatched through this gateway is accepted by the bound
/// aggregate — no runtime routing errors possible.
///
/// # Distributed failure model (COM-0025:R1)
///
/// Three failure modes apply at trait level. The four method-scoped
/// semantics (timeout, cancellation, retry, duplicate-delivery) are
/// documented on [`create`](Self::create) and [`send`](Self::send).
///
/// ## Crash
///
/// The gateway adds interceptors on top of the
/// [`CommandBus`](crate::CommandBus); the bus owns the persist/publish
/// unit-of-work and its crash guarantees (see `CommandBus`'s "Crash"
/// docstring). Gateway-level state (interceptor cursors, retry
/// budgets, in-flight metadata) MUST be either reconstructible after a
/// crash or explicitly transient. The gateway MUST NOT introduce
/// crash-visible state that the bus does not already guarantee.
///
/// ## Replay
///
/// Commands are NOT replayable (COM-0025:R1, same as
/// [`CommandBus`](crate::CommandBus)). The gateway exposes no replay
/// primitive; rebuilding aggregate state is done by replaying *events*
/// via [`EventStore::load`](crate::EventStore::load), never by
/// re-running commands through `create`/`send`.
///
/// ## Recovery
///
/// Implementor-level recovery (interceptor teardown, retry-budget
/// state clean-up, request-tracing flush, partial-shutdown drain) lives
/// outside the port surface. Each concrete `CommandGateway` MUST
/// document its recovery procedure in its own rustdoc.
///
/// # Doctest — contract type-check
///
/// The trait is async via RPITIT (CHE-0018:R2) and cherry-pit-core
/// declares no async runtime (CHE-0029:R4); the doctest constructs the
/// futures without awaiting them.
///
/// ```
/// use cherry_pit_core::{AggregateId, Command, CommandGateway, CorrelationContext, HandleCommand};
///
/// fn _type_check<G, C>(
///     gw: &G,
///     id: AggregateId,
///     cmd: C,
///     ctx: CorrelationContext,
/// ) where
///     G: CommandGateway,
///     G::Aggregate: HandleCommand<C>,
///     C: Command + Clone,
/// {
///     let _create = gw.create(cmd.clone(), ctx.clone());
///     let _send = gw.send(id, cmd, ctx);
/// }
/// ```
pub trait CommandGateway: Send + Sync + 'static {
    /// The single aggregate type this gateway dispatches to.
    type Aggregate: crate::aggregate::Aggregate;

    /// Create a new aggregate instance.
    ///
    /// The gateway:
    /// 1. Runs dispatch interceptors (logging, metadata, validation).
    /// 2. Delegates to the `CommandBus`.
    /// 3. Optionally retries on transient infrastructure failure.
    ///
    /// Returns the store-assigned [`AggregateId`] and the event
    /// envelopes produced by the aggregate on success.
    ///
    /// # Timeout
    ///
    /// The returned future MAY resolve to
    /// [`DispatchError::Infrastructure`](crate::DispatchError::Infrastructure)
    /// after the gateway's deadline (typically tighter than the bus's)
    /// elapses. Timeouts are
    /// [`Retryable`](crate::ErrorCategory::Retryable) (CHE-0046:R1).
    /// The gateway MAY consume some of the deadline on interceptor
    /// processing before delegating to
    /// [`CommandBus::create`](crate::CommandBus::create).
    ///
    /// # Cancellation
    ///
    /// Dropping the returned future cancels the gateway pipeline.
    /// Interceptor teardown MUST be drop-safe; in-flight retries MUST
    /// terminate without producing additional bus calls. The bus's
    /// unit-of-work guarantee (see
    /// [`CommandBus::create`](crate::CommandBus::create)) binds under
    /// gateway cancellation.
    ///
    /// # Retry
    ///
    /// The gateway is the standard place to host retry policy:
    /// implementors MAY transparently retry on
    /// [`Retryable`](crate::ErrorCategory::Retryable) errors per
    /// CHE-0046:R1 and MUST stop on
    /// [`Terminal`](crate::ErrorCategory::Terminal) per CHE-0046:R2.
    /// As with [`CommandBus::create`](crate::CommandBus::create),
    /// retry of `create` is unsafe without an
    /// [`IdempotencyKey`](crate::IdempotencyKey) — gateway retry MUST
    /// either require an idempotency key on the command or refuse to
    /// retry `create`.
    ///
    /// # Duplicate delivery
    ///
    /// Same contract as
    /// [`CommandBus::create`](crate::CommandBus::create): idempotency
    /// is the command's [`IdempotencyKey`](crate::IdempotencyKey)
    /// (CHE-0041); without it, duplicate `create` calls produce
    /// distinct aggregates. Gateway-level retry MUST respect the
    /// idempotency requirement.
    fn create<C>(
        &self,
        cmd: C,
        context: CorrelationContext,
    ) -> impl Future<Output = CreateResult<Self::Aggregate, C>> + Send
    where
        Self::Aggregate: HandleCommand<C>,
        C: Command;

    /// Dispatch a command targeting an existing aggregate instance.
    ///
    /// The gateway:
    /// 1. Runs dispatch interceptors (logging, metadata, validation).
    /// 2. Delegates to the `CommandBus`.
    /// 3. Optionally retries on transient infrastructure failure.
    ///
    /// Returns the event envelopes produced by the aggregate on success.
    ///
    /// # Timeout
    ///
    /// The returned future MAY resolve to
    /// [`DispatchError::Infrastructure`](crate::DispatchError::Infrastructure)
    /// after the gateway's deadline elapses. Timeouts are
    /// [`Retryable`](crate::ErrorCategory::Retryable) (CHE-0046:R1).
    /// As with [`create`](Self::create), the gateway MAY consume part
    /// of the deadline before delegating to
    /// [`CommandBus::dispatch`](crate::CommandBus::dispatch).
    ///
    /// # Cancellation
    ///
    /// Dropping the returned future cancels the gateway pipeline,
    /// including any in-flight retries. The bus's unit-of-work
    /// guarantee (see
    /// [`CommandBus::dispatch`](crate::CommandBus::dispatch)) binds
    /// under gateway cancellation: a cancelled `send` either
    /// persists-and-publishes-all or persists-and-publishes-none.
    ///
    /// # Retry
    ///
    /// Retries on
    /// [`Retryable`](crate::ErrorCategory::Retryable) errors are the
    /// gateway's responsibility per CHE-0046:R1.
    /// [`DispatchError::ConcurrencyConflict`](crate::DispatchError::ConcurrencyConflict)
    /// retries MUST reload the aggregate (typically by re-entering the
    /// gateway with a fresh dispatch); blind retry of the same future
    /// without reload will livelock against a moving sequence head.
    /// [`Terminal`](crate::ErrorCategory::Terminal) errors
    /// ([`Rejected`](crate::DispatchError::Rejected),
    /// [`AggregateNotFound`](crate::DispatchError::AggregateNotFound))
    /// MUST NOT be retried (CHE-0046:R2).
    ///
    /// # Duplicate delivery
    ///
    /// Same contract as
    /// [`CommandBus::dispatch`](crate::CommandBus::dispatch):
    /// duplicate commands are de-duplicated via the command's
    /// [`IdempotencyKey`](crate::IdempotencyKey) (CHE-0041); without
    /// it, the underlying store's `expected_sequence` check on
    /// [`EventStore::append`](crate::EventStore::append) preserves the
    /// at-most-once-effect property by rejecting stale retries with
    /// `ConcurrencyConflict`.
    fn send<C>(
        &self,
        id: AggregateId,
        cmd: C,
        context: CorrelationContext,
    ) -> impl Future<Output = DispatchResult<Self::Aggregate, C>> + Send
    where
        Self::Aggregate: HandleCommand<C>,
        C: Command;
}
