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
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    middleware::from_fn,
    response::IntoResponse,
    routing::{get, post},
};
use cherry_pit_core::{Aggregate, AggregateId, CommandGateway, DomainEvent, EventStore};
use serde::Serialize;
use tokio::sync::Semaphore;
use tower_http::limit::RequestBodyLimitLayer;

use crate::command_router::{CommandRouter, DispatchOutcome};
use crate::middleware::ErrorBody;
use crate::middleware::correlation_layer;
use crate::middleware::limits::http_concurrency_limit;
use crate::middleware::{LayerLimits, extract_correlation, extract_idempotency_key};
use crate::state::AppState;

/// Build the cherry-pit-web router.
///
/// Mounts routes under `/v1/` (CHE-0049 R9) and merges `extra_routes`
/// at the top level. The SEC-0003 availability layers and the
/// correlation echo wrap the merged router, so `extra_routes` are
/// covered too (CHE-0062:R4 — unconditional at every ingestion point;
/// CHE-0049:R5 — every response echoes the correlation identifier).
/// `limits.max_body_bytes` is a **ceiling**: consumers may nest a
/// tighter `RequestBodyLimitLayer` on their own route groups, and
/// still compose their own auth and rate-limit policy there
/// (CHE-0049:R2).
/// Returned [`Router`] has state applied ([`Router::with_state`]) —
/// ready for `axum::serve` or composition. Generic dispatch is
/// mandatory (CHE-0049 R1, CHE-0050 R4); `Box<dyn _>` forbidden.
///
/// # Type parameters
///
/// `G` — consumer's [`CommandGateway`]. `S` — consumer's
/// [`EventStore`], `Event` matches [`Aggregate::Event`]. `R` —
/// consumer's [`CommandRouter`] impl, bound to `G` (CHE-0050 R2).
///
/// # Parameters
///
/// - `state` — typed application state (CHE-0049:R1, CHE-0050:R2).
/// - `limits` — SEC-0003 availability-layer sizing applied to the
///   whole surface: body ceiling ([`RequestBodyLimitLayer`]), inflight
///   cap ([`http_concurrency_limit`], 503-shedding) per CHE-0062:R1/R4.
///   Tests use [`LayerLimits::permissive_for_tests`].
/// - `extra_routes` — stateless [`Router`] merged inside the SEC-0003
///   stack (CHE-0049:R2); pass [`Router::new`] when unused.
///
/// # Example
///
/// Minimal `AppState` + `extra_routes`; see [`AppState`] doctest.
///
/// ```
/// use std::num::NonZeroU64;
/// use axum::Router;
/// use cherry_pit_core::{
///     Aggregate, AggregateId, Command, CommandGateway, CorrelationContext,
///     CreateResult, DispatchResult, DomainEvent, EventEnvelope, EventStore,
///     HandleCommand, StoreCreateResult, StoreError,
/// };
/// use cherry_pit_web::{AppState, CommandRouter, DispatchOutcome, LayerLimits, build_router};
/// use cherry_pit_web::correlation::IdempotencyKey;
/// use cherry_pit_web::errors::ErrorEnvelope;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// enum E { Created }
/// impl DomainEvent for E { fn event_type(&self) -> &'static str { "e" } }
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
///     fn target_aggregate_id(_: &W) -> Option<AggregateId> { None }
/// }
///
/// let state = AppState::new(G, S, R);
/// let _app: Router = build_router(state, LayerLimits::permissive_for_tests(), Router::new());
/// ```
pub fn build_router<G, S, R>(
    state: AppState<G, S, R>,
    limits: LayerLimits,
    extra_routes: Router,
) -> Router
where
    G: CommandGateway,
    S: EventStore<Event = <G::Aggregate as Aggregate>::Event>,
    <G::Aggregate as Aggregate>::Event: Serialize,
    R: CommandRouter<Gateway = G> + Clone + Send + Sync + 'static,
{
    let http_semaphore = Arc::new(Semaphore::new(limits.max_inflight_requests.get()));
    let v1 = Router::new()
        .nest("/v1", v1_routes::<G, S, R>())
        .with_state(state);

    Router::new()
        .merge(v1)
        .merge(extra_routes)
        .layer(from_fn(correlation_layer))
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(limits.max_body_bytes.get()))
        .layer(axum::middleware::from_fn(move |request, next| {
            let sem = Arc::clone(&http_semaphore);
            http_concurrency_limit(sem, request, next)
        }))
}

/// Versioned routes mounted under `/v1/` per CHE-0049 R9.
fn v1_routes<G, S, R>() -> Router<AppState<G, S, R>>
where
    G: CommandGateway,
    S: EventStore<Event = <G::Aggregate as Aggregate>::Event>,
    <G::Aggregate as Aggregate>::Event: Serialize,
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
    let echo = ctx.clone();
    let idempotency = extract_idempotency_key(&headers);
    let outcome = state
        .router()
        .dispatch(state.gateway(), ctx, idempotency, wire)
        .await;
    match outcome {
        Ok(DispatchOutcome::Created { aggregate_id }) => {
            (StatusCode::CREATED, Json(CreatedBody { aggregate_id })).into_response()
        }
        Ok(DispatchOutcome::Sent) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(
                ErrorBody {
                    code: "router_misroute",
                    message: "router returned Sent on create endpoint".to_string(),
                    correlation_id: None,
                }
                .with_correlation(&echo),
            ),
        )
            .into_response(),
        Err((status, headers, body)) => (status, headers, Json(body)).into_response(),
    }
}

/// Outcome of resolving the URL path aggregate id against the wire
/// DTO's own declared target on the send path.
///
/// Three states, no boolean: absence of a body target is its own
/// variant, so it cannot collapse into agreement. A caller must name
/// every arm to consume this, which is what keeps
/// [`send_handler`] from failing open on an unverifiable request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathBodyAgreement {
    /// Both authorities name the same aggregate.
    Agrees,
    /// Both name an aggregate, and they differ.
    Mismatch,
    /// The wire DTO names no aggregate, so the path id is
    /// unconfirmable — not confirmed.
    BodyDeclaresNoTarget,
}

impl PathBodyAgreement {
    fn resolve(path: AggregateId, body: Option<AggregateId>) -> Self {
        match body {
            Some(target) if target == path => Self::Agrees,
            Some(_) => Self::Mismatch,
            None => Self::BodyDeclaresNoTarget,
        }
    }
}

/// `POST /v1/aggregates/:id/commands` — send handler.
///
/// The path `:id` and the wire DTO are two potential authorities over
/// one aggregate identity. This handler resolves them against each
/// other via [`PathBodyAgreement`] and dispatches only on
/// [`PathBodyAgreement::Agrees`], so neither authority can silently
/// win. Status for a disagreement is selected here, never in
/// [`CommandRouter::dispatch`] (CHE-0050:R3).
async fn send_handler<G, S, R>(
    State(state): State<AppState<G, S, R>>,
    Path(id): Path<NonZeroU64>,
    headers: HeaderMap,
    Json(wire): Json<R::Wire>,
) -> axum::response::Response
where
    G: CommandGateway,
    S: EventStore<Event = <G::Aggregate as Aggregate>::Event>,
    R: CommandRouter<Gateway = G> + Clone + Send + Sync + 'static,
{
    let ctx = extract_correlation(&headers);
    let echo = ctx.clone();

    let refusal =
        match PathBodyAgreement::resolve(AggregateId::new(id), R::target_aggregate_id(&wire)) {
            PathBodyAgreement::Agrees => None,
            PathBodyAgreement::Mismatch => Some(ErrorBody {
                code: "path_body_target_mismatch",
                message: "path aggregate id disagrees with the command's target aggregate"
                    .to_string(),
                correlation_id: None,
            }),
            PathBodyAgreement::BodyDeclaresNoTarget => Some(ErrorBody {
                code: "path_body_target_absent",
                message: "command names no target aggregate, so the path aggregate id \
                      cannot be confirmed"
                    .to_string(),
                correlation_id: None,
            }),
        };
    if let Some(body) = refusal {
        return (StatusCode::BAD_REQUEST, Json(body.with_correlation(&echo))).into_response();
    }

    let idempotency = extract_idempotency_key(&headers);
    let outcome = state
        .router()
        .dispatch(state.gateway(), ctx, idempotency, wire)
        .await;
    match outcome {
        Ok(DispatchOutcome::Sent) => StatusCode::OK.into_response(),
        Ok(DispatchOutcome::Created { aggregate_id }) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(
                ErrorBody {
                    code: "router_misroute",
                    message: format!("router returned Created({aggregate_id}) on send endpoint"),
                    correlation_id: None,
                }
                .with_correlation(&echo),
            ),
        )
            .into_response(),
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
struct LoadedBody<E: DomainEvent + Serialize> {
    aggregate_id: AggregateId,
    events: Vec<LoadedEvent<E>>,
}

/// Wire-friendly projection of [`cherry_pit_core::EventEnvelope`] —
/// public-method accessors only, no struct-literal access to private
/// fields of the upstream type.
#[derive(Debug, Serialize)]
struct LoadedEvent<E: DomainEvent + Serialize> {
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
    <G::Aggregate as Aggregate>::Event: Serialize,
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
