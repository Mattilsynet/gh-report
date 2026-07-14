//! Consumer-owned wire-deserialize-and-dispatch port.
//!
//! Realises **CHE-0050 R1–R5**: the boundary between HTTP plumbing
//! (status mapping, correlation echo, idempotency-key threading,
//! `/v1/` DTO contract — CHE-0049 R4–R6 + R10) and the consumer's
//! domain knowledge (wire DTO → `Command`, invoking
//! `CommandGateway::create` / `::send`).
//!
//! ## Why a port, not a sum-type
//!
//! `cherry-pit-core::Command` carries no `Deserialize` bound by design
//! (CHE-0014 R2). Rejected alternatives — a consumer-side
//! `enum AllCommands`, or moving create/send into `extra_routes` —
//! are in CHE-0050's Rejected Alternatives. This keeps deserialization
//! on the consumer, HTTP in cherry-pit-web.
//!
//! ## Object-unsafety is the design (CHE-0050 R4)
//!
//! The associated `Wire` type plus generic `Gateway` make this trait
//! object-unsafe by construction; every consumer writes one explicit
//! impl, keeping dispatch monomorphised (CHE-0049 R1: no `Box<dyn _>`
//! over infrastructure ports).

use cherry_pit_core::{AggregateId, CorrelationContext};
use serde::de::DeserializeOwned;

use crate::middleware::ErrorEnvelope;
use crate::middleware::IdempotencyKey;

/// Successful router dispatch outcome.
///
/// Minimal companion type for `Result<DispatchOutcome, ErrorEnvelope>`
/// — names the two cases the `/v1/aggregates` POST and
/// `/v1/aggregates/:id/commands` POST handlers convert into HTTP
/// responses (CHE-0049 R4):
///
/// * [`Created`](Self::Created) — produced by `create` flows; the
///   handler returns 201 Created with the new [`AggregateId`] in the
///   response body.
/// * [`Sent`](Self::Sent) — produced by `send` flows; the handler
///   returns 200 OK.
///
/// Carrying the freshly-assigned `AggregateId` here (rather than
/// reaching back into the gateway envelopes) keeps the router →
/// handler boundary a single typed value with no further indirection.
/// Event-envelope projection is out of scope for v0.1; if richer
/// payloads are needed later, this enum gains variants without
/// breaking existing impls (it is `#[non_exhaustive]`).
///
/// # Example
///
/// ```
/// use std::num::NonZeroU64;
/// use cherry_pit_core::AggregateId;
/// use cherry_pit_web::DispatchOutcome;
///
/// let id = AggregateId::new(NonZeroU64::new(7).unwrap());
/// let outcome = DispatchOutcome::Created { aggregate_id: id };
///
/// match outcome {
///     DispatchOutcome::Created { aggregate_id } => {
///         assert_eq!(aggregate_id, id);
///     }
///     DispatchOutcome::Sent => unreachable!(),
///     _ => unreachable!("non_exhaustive future variant"),
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DispatchOutcome {
    /// A new aggregate was created with the given id.
    Created {
        /// Store-assigned aggregate identifier.
        aggregate_id: AggregateId,
    },
    /// A command was successfully dispatched against an existing
    /// aggregate. No payload — the handler returns a bare 200 OK.
    Sent,
}

/// Consumer-owned wire-deserialize-and-dispatch port (CHE-0050 R1).
///
/// Implementors:
///
/// 1. Name a [`Wire`](Self::Wire) type — the request DTO handed to
///    [`dispatch`](Self::dispatch) after JSON deserialization.
/// 2. Bind [`Gateway`](Self::Gateway) to a concrete
///    [`cherry_pit_core::CommandGateway`] (the `G` on
///    [`AppState`](crate::AppState)).
/// 3. Translate the DTO, invoke `gateway.create(...)` or
///    `gateway.send(...)`.
/// 4. Map the gateway's error via the public mapping helpers
///    (`map_dispatch_error`, `map_store_error`, `map_bus_error`) —
///    never ad-hoc.
///
/// `dispatch` is the **only** method by design (R1).
///
/// ## Bounds
///
/// No supertrait bounds itself; `Send + Sync + 'static + Clone` live
/// on [`AppState`](crate::AppState)'s third parameter (CHE-0050 R2) —
/// requirements of *storing* `R`, not the trait, so test harnesses
/// needn't repeat them.
///
/// # Example
///
/// A minimal impl; the stub gateway returns `Infallible` errors, a
/// real consumer maps the error via `map_dispatch_error`.
///
/// ```
/// use std::num::NonZeroU64;
/// use cherry_pit_core::{
///     Aggregate, AggregateId, Command, CommandGateway, CorrelationContext,
///     CreateResult, DispatchResult, DomainEvent, HandleCommand,
/// };
/// use cherry_pit_web::{CommandRouter, DispatchOutcome};
/// use cherry_pit_web::correlation::IdempotencyKey;
/// use cherry_pit_web::errors::ErrorEnvelope;
/// use serde::{Deserialize, Serialize};
///
/// // Domain — minimal aggregate + event + command.
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// enum E { Created }
/// impl DomainEvent for E {
///     fn event_type(&self) -> &'static str { "e.created" }
/// }
/// #[derive(Default)]
/// struct A;
/// impl Aggregate for A {
///     type Event = E;
///     fn apply(&mut self, _: &Self::Event) {}
/// }
/// #[derive(Debug)] struct C;
/// impl Command for C {}
/// impl HandleCommand<C> for A {
///     type Error = std::convert::Infallible;
///     fn handle(&self, _: C) -> Result<Vec<E>, Self::Error> {
///         Ok(vec![E::Created])
///     }
/// }
///
/// // Stub gateway — returns a fixed aggregate id.
/// #[derive(Clone)]
/// struct G;
/// impl CommandGateway for G {
///     type Aggregate = A;
///     async fn create<Cmd>(
///         &self, _: Cmd, _: CorrelationContext,
///     ) -> CreateResult<A, Cmd>
///     where A: HandleCommand<Cmd>, Cmd: Command,
///     {
///         Ok((AggregateId::new(NonZeroU64::new(1).unwrap()), vec![]))
///     }
///     async fn send<Cmd>(
///         &self, _: AggregateId, _: Cmd, _: CorrelationContext,
///     ) -> DispatchResult<A, Cmd>
///     where A: HandleCommand<Cmd>, Cmd: Command,
///     { Ok(vec![]) }
/// }
///
/// // Wire DTO + router impl.
/// #[derive(Deserialize)] struct Wire;
///
/// #[derive(Clone)]
/// struct R;
/// impl CommandRouter for R {
///     type Gateway = G;
///     type Wire = Wire;
///     async fn dispatch(
///         &self,
///         gateway: &G,
///         ctx: CorrelationContext,
///         _idempotency: Option<IdempotencyKey>,
///         _wire: Wire,
///     ) -> Result<DispatchOutcome, ErrorEnvelope> {
///         let (aggregate_id, _) = gateway.create(C, ctx).await.unwrap();
///         Ok(DispatchOutcome::Created { aggregate_id })
///     }
/// }
///
/// // Smoke check the impl runs.
/// let outcome = tokio::runtime::Runtime::new().unwrap().block_on(async {
///     R.dispatch(&G, CorrelationContext::none(), None, Wire).await
/// });
/// assert!(matches!(outcome, Ok(DispatchOutcome::Created { .. })));
/// ```
pub trait CommandRouter {
    /// The consumer's [`CommandGateway`] type — bound via
    /// `R: CommandRouter<Gateway = G>` (CHE-0050 R2).
    ///
    /// [`CommandGateway`]: cherry_pit_core::CommandGateway
    type Gateway: cherry_pit_core::CommandGateway;

    /// The wire-format DTO carried in the request body.
    ///
    /// Per CHE-0050 R1 only this type carries the
    /// [`DeserializeOwned`] bound; `Command` stays free of
    /// `Deserialize` (CHE-0014 R2).
    type Wire: DeserializeOwned + Send + 'static;

    /// Translate a wire DTO into a domain command and dispatch it
    /// through the gateway.
    ///
    /// cherry-pit-web has already extracted [`CorrelationContext`]
    /// (CHE-0049 R5), the optional [`IdempotencyKey`] (CHE-0049 R6,
    /// CHE-0046 R3), and deserialized the body into `Self::Wire`.
    ///
    /// The implementation must:
    ///
    /// * pick the right gateway method (`create` new-aggregate,
    ///   `send` command-on-existing),
    /// * pass `ctx` through unchanged,
    /// * thread `idempotency` to the consumer replay store
    ///   (CHE-0046 R3),
    /// * return [`DispatchOutcome::Created { aggregate_id }`] for
    ///   `create` or [`DispatchOutcome::Sent`] for `send`,
    /// * on error, build an [`ErrorEnvelope`] via the public mapping
    ///   helpers (single source of truth, CHE-0049 R10).
    ///
    /// The returned [`ErrorEnvelope`] must not carry HTTP concerns:
    /// correlation echo is added by surrounding middleware (CHE-0049
    /// R5); the router does not build `axum::response::Response`
    /// directly (CHE-0050 R3).
    fn dispatch(
        &self,
        gateway: &Self::Gateway,
        ctx: CorrelationContext,
        idempotency: Option<IdempotencyKey>,
        wire: Self::Wire,
    ) -> impl std::future::Future<Output = Result<DispatchOutcome, ErrorEnvelope>> + Send;
}
