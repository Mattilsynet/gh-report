use std::future::Future;

use crate::aggregate::HandleCommand;
use crate::aggregate_id::AggregateId;
use crate::command::Command;
use crate::correlation::CorrelationContext;
use crate::error::{CreateResult, DispatchResult};

/// The primary entry point for dispatching commands into the system.
///
/// Wraps the [`CommandBus`](crate::CommandBus) with cross-cutting concerns
/// (interceptors, retry, logging); every primary adapter and every
/// `Policy` dispatches through the gateway, the outermost port on the
/// driving side of the hexagon (CHE-0005:R1, CHE-0018:R2, CHE-0025:R1-R2,
/// CHE-0046:R1-R2).
///
/// Bound to a single aggregate type: the compiler rejects commands not
/// accepted by the bound aggregate.
///
/// # Distributed failure model (COM-0025:R1)
///
/// Crash, Replay, Recovery follow [`CommandBus`](crate::CommandBus)'s
/// trait-level contract verbatim: no crash-visible state beyond the bus's
/// guarantee, no replay primitive (rebuild via
/// [`EventStore::load`](crate::EventStore::load) only), and each concrete
/// gateway documents its own recovery. Timeout, cancellation, retry, and
/// duplicate-delivery live on [`create`](Self::create) and
/// [`send`](Self::send).
///
/// # Doctest - contract type-check
///
/// RPITIT async; no async runtime (CHE-0029:R4). Constructs futures
/// without awaiting.
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
    /// Runs dispatch interceptors, delegates to the `CommandBus`, and
    /// optionally retries on transient infrastructure failure. Returns the
    /// store-assigned [`AggregateId`] and the produced envelopes.
    ///
    /// # Timeout
    ///
    /// MAY resolve to
    /// [`DispatchError::Infrastructure`](crate::DispatchError::Infrastructure)
    /// after the deadline; timeouts are
    /// [`Retryable`](crate::ErrorCategory::Retryable) (CHE-0046:R1). May
    /// spend part of the deadline on interceptors before delegating to
    /// [`CommandBus::create`](crate::CommandBus::create).
    ///
    /// # Cancellation
    ///
    /// Dropping the future cancels the pipeline; teardown MUST be drop-safe,
    /// retries MUST NOT add bus calls. Unit-of-work guarantee binds under
    /// cancellation.
    ///
    /// # Retry
    ///
    /// Hosts retry policy: MAY retry
    /// [`Retryable`](crate::ErrorCategory::Retryable) per CHE-0046:R1, MUST
    /// stop on [`Terminal`](crate::ErrorCategory::Terminal) per CHE-0046:R2.
    /// Retrying `create` needs an
    /// [`IdempotencyKey`](crate::IdempotencyKey) — require one or refuse.
    ///
    /// # Duplicate delivery
    ///
    /// Same contract as
    /// [`CommandBus::create`](crate::CommandBus::create): idempotency is the
    /// [`IdempotencyKey`](crate::IdempotencyKey) (CHE-0041); without it,
    /// duplicate calls produce distinct aggregates.
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
    /// Runs dispatch interceptors, delegates to the `CommandBus`, and
    /// optionally retries on transient infrastructure failure. Returns the
    /// event envelopes the aggregate produced.
    ///
    /// # Timeout
    ///
    /// MAY resolve to
    /// [`DispatchError::Infrastructure`](crate::DispatchError::Infrastructure)
    /// after the deadline; timeouts are
    /// [`Retryable`](crate::ErrorCategory::Retryable) (CHE-0046:R1).
    ///
    /// # Cancellation
    ///
    /// Dropping the future cancels the pipeline, including retries. The
    /// unit-of-work guarantee binds: a cancelled `send`
    /// persists-and-publishes-all or -none.
    ///
    /// # Retry
    ///
    /// Retries on [`Retryable`](crate::ErrorCategory::Retryable) errors are
    /// the gateway's responsibility (CHE-0046:R1).
    /// [`DispatchError::ConcurrencyConflict`](crate::DispatchError::ConcurrencyConflict)
    /// retries MUST reload the aggregate; blind retry livelocks against a
    /// moving sequence head. [`Terminal`](crate::ErrorCategory::Terminal)
    /// errors MUST NOT be retried (CHE-0046:R2).
    ///
    /// # Duplicate delivery
    ///
    /// Same contract as
    /// [`CommandBus::dispatch`](crate::CommandBus::dispatch): duplicates
    /// de-duplicate via [`IdempotencyKey`](crate::IdempotencyKey)
    /// (CHE-0041); the store's `expected_sequence` check rejects stale
    /// retries otherwise.
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
