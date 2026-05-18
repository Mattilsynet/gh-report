//! Axum router assembly for `cherry-pit-web`.
//!
//! Realises **CHE-0049 R9** (versioned `/v1/` DTO contract),
//! **CHE-0049 R2** (zero default auth; consumer-attached `extra_routes`
//! merge point), **CHE-0049 R4–R6 + R10** (status mapping, correlation
//! echo, idempotency-key threading on the create/send POST paths), and
//! **CHE-0050 R2–R3** (third type parameter `R`; handlers retain HTTP
//! concerns while delegating wire-deserialize-and-dispatch to the
//! consumer's [`CommandRouter`] impl).

use std::num::NonZeroU64;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    middleware::from_fn,
    response::IntoResponse,
    routing::{get, post},
};
use cherry_pit_core::{Aggregate, AggregateId, CommandGateway, DomainEvent, EventStore};
use serde::Serialize;

use crate::command_router::{CommandRouter, DispatchOutcome};
use crate::middleware::ErrorBody;
use crate::middleware::correlation_layer;
use crate::middleware::{extract_correlation, extract_idempotency_key};
use crate::state::AppState;

/// Build the cherry-pit-web router.
///
/// Mounts cherry-pit-web's own routes under `/v1/` per CHE-0049 R9 and
/// merges `extra_routes` at the top level so consumers can attach
/// auth-protected or non-versioned surfaces (CHE-0049 R2).
///
/// The returned [`Router`] has its state already applied via
/// [`Router::with_state`] — it is ready to be served by
/// `axum::serve` or composed into a larger application.
///
/// # Type parameters
///
/// * `G` — the consumer's concrete [`CommandGateway`].
/// * `S` — the consumer's concrete [`EventStore`], whose
///   [`EventStore::Event`] matches the aggregate's
///   [`Aggregate::Event`].
/// * `R` — the consumer's concrete [`CommandRouter`] impl, bound to
///   the same `G` (CHE-0050 R2).
///
/// Generic dispatch is mandatory per CHE-0049 R1 and CHE-0050 R4;
/// `Box<dyn _>` over any of the three ports is forbidden.
///
/// # Example
///
/// Construct an axum [`Router`] from `AppState` plus an optional
/// `extra_routes` merge point. See the [`AppState`] doctest for the
/// full minimal stub set; reproduced here for self-containment.
///
/// ```
/// use std::num::NonZeroU64;
/// use axum::Router;
/// use cherry_pit_core::{
///     Aggregate, AggregateId, Command, CommandGateway, CorrelationContext,
///     CreateResult, DispatchResult, DomainEvent, EventEnvelope, EventStore,
///     HandleCommand, StoreCreateResult, StoreError,
/// };
/// use cherry_pit_web::{AppState, CommandRouter, DispatchOutcome, build_router};
/// use cherry_pit_web::correlation::IdempotencyKey;
/// use cherry_pit_web::errors::ErrorEnvelope;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// enum E { Created }
/// impl DomainEvent for E { fn event_type(&self) -> &'static str { "e" } }
/// // CHE-0064:R2 — hand-rolled Encode is mandatory; not derive-able.
/// impl pardosa_encoding::Encode for E {
///     fn encode(&self, out: &mut Vec<u8>) { match self { Self::Created => out.push(0u8) } }
/// }
/// #[derive(Default)] struct A;
/// impl Aggregate for A {
///     type Event = E;
///     fn apply(&mut self, _: &Self::Event) {}
/// }
/// #[derive(Debug)] struct C;
/// impl Command for C {}
/// impl HandleCommand<C> for A {
///     type Error = std::convert::Infallible;
///     fn handle(&self, _: C) -> Result<Vec<E>, Self::Error> { Ok(vec![]) }
/// }
/// #[derive(Clone)] struct G;
/// impl CommandGateway for G {
///     type Aggregate = A;
///     async fn create<Cmd>(&self, _: Cmd, _: CorrelationContext) -> CreateResult<A, Cmd>
///         where A: HandleCommand<Cmd>, Cmd: Command
///     { Ok((AggregateId::new(NonZeroU64::new(1).unwrap()), vec![])) }
///     async fn send<Cmd>(&self, _: AggregateId, _: Cmd, _: CorrelationContext) -> DispatchResult<A, Cmd>
///         where A: HandleCommand<Cmd>, Cmd: Command
///     { Ok(vec![]) }
/// }
/// struct S;
/// impl EventStore for S {
///     type Event = E;
///     async fn load(&self, _: AggregateId) -> Result<Vec<EventEnvelope<E>>, StoreError> { Ok(vec![]) }
///     async fn create(&self, _: Vec<E>, _: CorrelationContext) -> StoreCreateResult<E> {
///         Ok((AggregateId::new(NonZeroU64::new(1).unwrap()), vec![]))
///     }
///     async fn append(&self, _: AggregateId, _: NonZeroU64, _: Vec<E>, _: CorrelationContext)
///         -> Result<Vec<EventEnvelope<E>>, StoreError>
///     { Ok(vec![]) }
/// }
/// #[derive(Deserialize)] struct W;
/// #[derive(Clone)] struct R;
/// impl CommandRouter for R {
///     type Gateway = G;
///     type Wire = W;
///     async fn dispatch(&self, _: &G, _: CorrelationContext, _: Option<IdempotencyKey>, _: W)
///         -> Result<DispatchOutcome, ErrorEnvelope>
///     { Ok(DispatchOutcome::Sent) }
/// }
///
/// let state = AppState::new(G, S, R);
/// // `extra_routes` is the consumer-attached merge point per CHE-0049 R2.
/// let _app: Router = build_router(state, Router::new());
/// ```
pub fn build_router<G, S, R>(state: AppState<G, S, R>, extra_routes: Router) -> Router
where
    G: CommandGateway,
    S: EventStore<Event = <G::Aggregate as Aggregate>::Event>,
    R: CommandRouter<Gateway = G> + Clone + Send + Sync + 'static,
{
    // Apply state to cherry-pit-web's own `/v1/` subtree first, leaving
    // the outer router stateless (`Router<()>`). This lets consumers
    // pass a vanilla stateless `extra_routes` for auth / non-versioned
    // surfaces without forcing them to know `AppState`'s type.
    let v1 = Router::new()
        .nest("/v1", v1_routes::<G, S, R>())
        .layer(from_fn(correlation_layer))
        .with_state(state);
    Router::new().merge(extra_routes).merge(v1)
}

/// Versioned routes mounted under `/v1/` per CHE-0049 R9.
fn v1_routes<G, S, R>() -> Router<AppState<G, S, R>>
where
    G: CommandGateway,
    S: EventStore<Event = <G::Aggregate as Aggregate>::Event>,
    R: CommandRouter<Gateway = G> + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/aggregates", post(create_handler::<G, S, R>))
        .route("/aggregates/{id}", get(load_handler::<G, S, R>))
        .route("/aggregates/{id}/commands", post(send_handler::<G, S, R>))
}

/// JSON body returned by the create handler on success.
///
/// 201 Created carries the store-assigned aggregate id so the client
/// can immediately address subsequent `send` calls. The body shape is
/// intentionally minimal — projection / event-envelope payloads are
/// out of scope for v0.1 (CHE-0049 R4).
#[derive(Debug, serde::Serialize)]
struct CreatedBody {
    aggregate_id: AggregateId,
}

/// `POST /v1/aggregates` — create handler.
///
/// CHE-0049 R4–R6 + R10 obligations stay in this function; the router
/// is invoked solely for wire-deserialize-and-dispatch (CHE-0050 R3).
async fn create_handler<G, S, R>(
    State(state): State<AppState<G, S, R>>,
    headers: HeaderMap,
    Json(wire): Json<R::Wire>,
) -> axum::response::Response
where
    G: CommandGateway,
    S: EventStore<Event = <G::Aggregate as Aggregate>::Event>,
    R: CommandRouter<Gateway = G> + Clone + Send + Sync + 'static,
{
    let ctx = extract_correlation(&headers);
    let idempotency = extract_idempotency_key(&headers);
    let outcome = state
        .router()
        .dispatch(state.gateway(), ctx, idempotency, wire)
        .await;
    match outcome {
        Ok(DispatchOutcome::Created { aggregate_id }) => {
            (StatusCode::CREATED, Json(CreatedBody { aggregate_id })).into_response()
        }
        Ok(DispatchOutcome::Sent) => {
            // POST /v1/aggregates is a create endpoint; a Sent outcome
            // here indicates the consumer's router mis-routed the wire
            // variant. 500 is the conservative response — it preserves
            // the invariant that this endpoint never silently treats a
            // command-on-existing dispatch as creation.
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorBody {
                    code: "router_misroute",
                    message: "router returned Sent on create endpoint".to_string(),
                    correlation_id: None,
                }),
            )
                .into_response()
        }
        Err((status, headers, body)) => (status, headers, Json(body)).into_response(),
    }
}

/// `POST /v1/aggregates/:id/commands` — send handler.
///
/// The path `:id` is parsed but not currently propagated into the
/// router signature — CHE-0050 R1 fixes the `dispatch` shape and the
/// wire DTO is expected to carry whatever target id the consumer's
/// `Command` requires. The `Path` extractor remains so the route
/// pattern is honoured and a malformed `:id` still 400s before
/// reaching the router.
async fn send_handler<G, S, R>(
    State(state): State<AppState<G, S, R>>,
    Path(_id): Path<String>,
    headers: HeaderMap,
    Json(wire): Json<R::Wire>,
) -> axum::response::Response
where
    G: CommandGateway,
    S: EventStore<Event = <G::Aggregate as Aggregate>::Event>,
    R: CommandRouter<Gateway = G> + Clone + Send + Sync + 'static,
{
    let ctx = extract_correlation(&headers);
    let idempotency = extract_idempotency_key(&headers);
    let outcome = state
        .router()
        .dispatch(state.gateway(), ctx, idempotency, wire)
        .await;
    match outcome {
        Ok(DispatchOutcome::Sent) => StatusCode::OK.into_response(),
        Ok(DispatchOutcome::Created { aggregate_id }) => {
            // Symmetric mis-route check: send endpoint should never
            // report a fresh creation.
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorBody {
                    code: "router_misroute",
                    message: format!("router returned Created({aggregate_id}) on send endpoint"),
                    correlation_id: None,
                }),
            )
                .into_response()
        }
        Err((status, headers, body)) => (status, headers, Json(body)).into_response(),
    }
}

/// JSON body returned by the load handler.
///
/// Carries the aggregate's event stream as a flat array of
/// [`LoadedEvent`] entries — wire-friendly projection of
/// [`cherry_pit_core::EventEnvelope`] without exposing the envelope's
/// private fields. An unknown aggregate yields `events: []` per
/// **CHE-0049 R7** (200 with an empty list, never 404 — CHE-0019 R1).
#[derive(Debug, Serialize)]
struct LoadedBody<E: DomainEvent> {
    aggregate_id: AggregateId,
    events: Vec<LoadedEvent<E>>,
}

/// Wire-friendly projection of [`cherry_pit_core::EventEnvelope`] —
/// public-method accessors only, no struct-literal access to private
/// fields of the upstream type.
#[derive(Debug, Serialize)]
struct LoadedEvent<E: DomainEvent> {
    event_id: uuid::Uuid,
    sequence: NonZeroU64,
    event_type: &'static str,
    payload: E,
}

/// `GET /v1/aggregates/:id` — load handler.
///
/// Realises **CHE-0049 R7** + **CHE-0019 R1**: a load against an
/// unknown aggregate returns 200 with an empty `events` list rather
/// than 404. 404 is reserved for `DispatchError::AggregateNotFound`
/// surfaced by *command* dispatch, not for read.
///
/// Path-id parsing failures (non-numeric or zero) surface as 400 via
/// the [`Path<NonZeroU64>`] extractor before this handler runs;
/// store-layer errors are mapped via [`crate::map_store_error`].
async fn load_handler<G, S, R>(
    State(state): State<AppState<G, S, R>>,
    Path(id): Path<NonZeroU64>,
) -> axum::response::Response
where
    G: CommandGateway,
    S: EventStore<Event = <G::Aggregate as Aggregate>::Event>,
    R: CommandRouter<Gateway = G> + Clone + Send + Sync + 'static,
{
    let aggregate_id = AggregateId::new(id);
    match state.store().load(aggregate_id).await {
        Ok(envelopes) => {
            let events = envelopes
                .into_iter()
                .map(|env| LoadedEvent {
                    event_id: env.event_id(),
                    sequence: env.sequence(),
                    event_type: env.payload().event_type(),
                    payload: env.payload().clone(),
                })
                .collect();
            (
                StatusCode::OK,
                Json(LoadedBody {
                    aggregate_id,
                    events,
                }),
            )
                .into_response()
        }
        Err(err) => {
            let (status, headers, body) = crate::middleware::map_store_error(&err);
            (status, headers, Json(body)).into_response()
        }
    }
}
