//! Consumer-owned wire-deserialize-and-dispatch port.
//!
//! Realises **CHE-0050 R1–R5**. The trait is the boundary between
//! cherry-pit-web's HTTP plumbing (status mapping, correlation echo,
//! idempotency-key threading, `/v1/` DTO contract — CHE-0049 R4–R6 +
//! R10) and the consumer's domain knowledge (which wire DTO maps to
//! which `Command`, and how to invoke `CommandGateway::create` /
//! `::send` against it).
//!
//! ## Why a port, not a sum-type
//!
//! `cherry-pit-core::Command` carries no `Deserialize` bound by
//! design (CHE-0014 R2). cherry-pit-web therefore cannot deserialize
//! request bodies into a `Command` from generic code. The two
//! alternatives — pushing a sum-type `enum AllCommands` onto the
//! consumer or moving the create/send handlers entirely into
//! `extra_routes` — were rejected in CHE-0050 (see Rejected
//! Alternatives). This trait keeps deserialization and dispatch on
//! the consumer side while leaving every HTTP concern in cherry-pit-web.
//!
//! ## Object-unsafety is the design (CHE-0050 R4)
//!
//! The combination of an associated type (`Wire`) and a generic
//! gateway parameter (`Gateway`) makes this trait **object-unsafe by
//! construction**. cherry-pit-web ships zero blanket impls and zero
//! default impls; every consumer writes one explicit impl per
//! cherry-pit-web instance. Combined with CHE-0049 R1 (no
//! `Box<dyn _>` over infrastructure ports) the trait is monomorphised
//! end-to-end, preserving zero-cost dispatch.

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
/// 1. Name a single [`Wire`](Self::Wire) type — the request DTO
///    cherry-pit-web hands to [`dispatch`](Self::dispatch) after JSON
///    body deserialization.
/// 2. Bind [`Gateway`](Self::Gateway) to a concrete
///    [`cherry_pit_core::CommandGateway`] (the same `G` carried by
///    the surrounding [`AppState`](crate::AppState)).
/// 3. Translate the wire DTO into a `Command` and invoke
///    `gateway.create(...)` or `gateway.send(...)` as appropriate
///    for the wire variant.
/// 4. Map the gateway's typed error into an [`ErrorEnvelope`] using
///    the public mapping helpers (`map_dispatch_error`,
///    `map_store_error`, `map_bus_error`) — cherry-pit-web's handlers
///    will attach correlation echo (CHE-0049 R5) and convert the
///    triple into an axum response, but the router is responsible
///    for picking the right mapper for the error it surfaced.
///
/// `dispatch` is the **only** method on the trait by design (R1):
/// adding more would invite consumers to host HTTP concerns the trait
/// has no business with.
///
/// ## Bounds
///
/// `CommandRouter` itself carries no supertrait bounds. The
/// `Send + Sync + 'static + Clone` bounds live on the third type
/// parameter of [`AppState`](crate::AppState) and
/// [`build_router`](crate::build_router) per CHE-0050 R2 — they are
/// requirements of the **storage** of an `R`, not of the trait
/// surface itself, and stating them here would force every impl to
/// repeat them even when stored under a different envelope (e.g.
/// inside test harnesses that don't use `AppState`).
///
/// # Example
///
/// A minimal `CommandRouter` impl. The stub gateway returns
/// `Infallible` errors; a real consumer maps the gateway error via
/// `map_dispatch_error` (CHE-0050 R1).
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
    /// The consumer's [`CommandGateway`] type — bound to the
    /// surrounding [`AppState`]'s `G` via
    /// `R: CommandRouter<Gateway = G>` (CHE-0050 R2).
    ///
    /// [`CommandGateway`]: cherry_pit_core::CommandGateway
    /// [`AppState`]: crate::AppState
    type Gateway: cherry_pit_core::CommandGateway;

    /// The wire-format DTO carried in the request body.
    ///
    /// Per CHE-0050 R1 this type — and **only** this type — carries
    /// the [`DeserializeOwned`] bound. `cherry-pit-core::Command`
    /// remains free of `Deserialize` per CHE-0014 R2.
    type Wire: DeserializeOwned + Send + 'static;

    /// Translate a wire DTO into a domain command and dispatch it
    /// through the gateway.
    ///
    /// cherry-pit-web has already:
    ///
    /// * extracted the [`CorrelationContext`] from inbound headers
    ///   (CHE-0049 R5),
    /// * extracted the optional [`IdempotencyKey`] (CHE-0049 R6,
    ///   CHE-0046 R3),
    /// * deserialized the request body into `Self::Wire`.
    ///
    /// The implementation must:
    ///
    /// * pick the right gateway method (`create` for new-aggregate
    ///   wire variants, `send` for command-on-existing variants),
    /// * pass `ctx` through to the gateway unchanged,
    /// * thread `idempotency` to whatever consumer-side replay store
    ///   the wire DTO targets (CHE-0046 R3),
    /// * return [`DispatchOutcome::Created { aggregate_id }`] for
    ///   `create` flows or [`DispatchOutcome::Sent`] for `send`
    ///   flows,
    /// * on gateway error, build an [`ErrorEnvelope`] via the public
    ///   `map_dispatch_error` / `map_store_error` / `map_bus_error`
    ///   helpers — never construct one ad-hoc (the helpers are the
    ///   single source of truth for CHE-0049 R10 status mapping).
    ///
    /// The returned [`ErrorEnvelope`] **must not** carry HTTP-level
    /// concerns the handler owns: correlation echo headers are added
    /// by the surrounding middleware (CHE-0049 R5); response status
    /// is the triple's first element; the router does not construct
    /// `axum::response::Response` values directly (CHE-0050 R3).
    fn dispatch(
        &self,
        gateway: &Self::Gateway,
        ctx: CorrelationContext,
        idempotency: Option<IdempotencyKey>,
        wire: Self::Wire,
    ) -> impl std::future::Future<Output = Result<DispatchOutcome, ErrorEnvelope>> + Send;
}
