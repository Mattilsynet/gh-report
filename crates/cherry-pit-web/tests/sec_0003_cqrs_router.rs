//! SEC-0003 — Availability — Bound All Resource Consumption (CQRS surface).
//!
//! Library-surface enforcement smoke for [`cherry_pit_web::build_router`],
//! mirroring `tests/sec_0003_enforced_at_library_surface.rs` (which
//! covers the projection-side router). Mechanism ADR: CHE-0062
//! (library-attached availability layers).
//!
//! - **R1** (bounded allocation): body cap → `413 Payload Too Large` via
//!   `tower_http::limit::RequestBodyLimitLayer`.
//! - **R3** (backpressure): inflight cap → `503 Service Unavailable`
//!   with shedding-not-queueing semantics
//!   (`crate::middleware::limits::http_concurrency_limit`).
//!
//! WS limits are **not** in scope on the CQRS router: per CHE-0049 R3
//! the cqrs surface is HTTP-only, so [`LayerLimits::max_ws_connections`]
//! is honoured here only as a `LayerLimits` field placeholder (the value
//! is unused by [`cherry_pit_web::build_router`]).

use std::convert::Infallible;
use std::num::NonZeroU64;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use cherry_pit_core::{
    Aggregate, AggregateId, Command, CommandGateway, CorrelationContext, DispatchError,
    DispatchResult, DomainEvent, EventEnvelope, EventStore, HandleCommand, StoreCreateResult,
    StoreError,
};
use cherry_pit_web::errors::ErrorEnvelope;
use cherry_pit_web::{AppState, CommandRouter, DispatchOutcome, LayerLimits, build_router};
use serde::{Deserialize, Serialize};
use tower::ServiceExt;

#[derive(Debug, Clone, Serialize, Deserialize)]
enum StubEvent {
    Noop,
}

impl DomainEvent for StubEvent {
    fn event_type(&self) -> &'static str {
        "stub.noop"
    }
}

#[derive(Default)]
struct StubAggregate;

impl Aggregate for StubAggregate {
    type Event = StubEvent;
    fn apply(&mut self, _event: &Self::Event) {}
}

struct StubCmd;
impl Command for StubCmd {}

impl HandleCommand<StubCmd> for StubAggregate {
    type Error = Infallible;
    fn handle(&self, _cmd: StubCmd) -> Result<Vec<Self::Event>, Self::Error> {
        Ok(vec![StubEvent::Noop])
    }
}

struct StubGateway;

impl CommandGateway for StubGateway {
    type Aggregate = StubAggregate;

    async fn create<C>(
        &self,
        _cmd: C,
        _context: CorrelationContext,
    ) -> cherry_pit_core::CreateResult<Self::Aggregate, C>
    where
        Self::Aggregate: HandleCommand<C>,
        C: Command,
    {
        Err(DispatchError::Infrastructure("stub gateway".into()))
    }

    async fn send<C>(
        &self,
        _id: AggregateId,
        _cmd: C,
        _context: CorrelationContext,
    ) -> DispatchResult<Self::Aggregate, C>
    where
        Self::Aggregate: HandleCommand<C>,
        C: Command,
    {
        Err(DispatchError::Infrastructure("stub gateway".into()))
    }
}

struct StubStore;

impl EventStore for StubStore {
    type Event = StubEvent;

    async fn load(&self, _id: AggregateId) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        Ok(vec![])
    }

    async fn create(
        &self,
        _events: Vec<Self::Event>,
        _context: CorrelationContext,
    ) -> StoreCreateResult<Self::Event> {
        Err(StoreError::Infrastructure("stub store".into()))
    }

    async fn append(
        &self,
        _id: AggregateId,
        _expected_sequence: NonZeroU64,
        _events: Vec<Self::Event>,
        _context: CorrelationContext,
    ) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        Err(StoreError::Infrastructure("stub store".into()))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StubWire {
    Create,
}

#[derive(Clone)]
struct StubRouter;

impl CommandRouter for StubRouter {
    type Gateway = StubGateway;
    type Wire = StubWire;

    async fn dispatch(
        &self,
        _gateway: &Self::Gateway,
        _ctx: CorrelationContext,
        _idempotency: Option<cherry_pit_web::correlation::IdempotencyKey>,
        wire: Self::Wire,
    ) -> Result<DispatchOutcome, ErrorEnvelope> {
        match wire {
            StubWire::Create => Ok(DispatchOutcome::Created {
                aggregate_id: AggregateId::new(NonZeroU64::new(1).unwrap()),
            }),
        }
    }
}

fn app_with(limits: LayerLimits) -> Router {
    let state: AppState<StubGateway, StubStore, StubRouter> =
        AppState::new(StubGateway, StubStore, StubRouter);
    build_router(state, limits, Router::new())
}

/// SEC-0003:R1 (bounded allocation) — body bytes exceeding
/// `LayerLimits.max_body_bytes` get `413 Payload Too Large` before
/// reaching the handler. Mechanism: CHE-0062 attaches
/// `tower_http::limit::RequestBodyLimitLayer` on the v1 sub-router.
///
/// `RequestBodyLimitLayer` short-circuits on `Content-Length > max`
/// before dispatching to a route, so the JSON-body dispatch never
/// runs — the body-cap layer firing first is exactly the invariant
/// we need on the cqrs surface.
#[tokio::test]
async fn cqrs_body_over_max_returns_413() {
    let limits = LayerLimits {
        max_body_bytes: 1024,
        max_inflight_requests: 1024,
        max_ws_connections: 1024,
    };
    let app = app_with(limits);

    let big_body = vec![0u8; 2048];
    let req = Request::builder()
        .method("POST")
        .uri("/v1/aggregates")
        .header("content-type", "application/json")
        .header("content-length", "2048")
        .body(Body::from(big_body))
        .expect("request build");

    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(
        resp.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "body of 2048 > max_body_bytes=1024 must be rejected with 413"
    );
}

/// SEC-0003:R1 — body within `max_body_bytes` passes the layer.
/// The dispatch then runs and returns the success status the stub
/// router produces (201 for `Create`), so the body-cap layer must
/// not front-run a valid request.
#[tokio::test]
async fn cqrs_body_under_max_passes_layer() {
    let limits = LayerLimits {
        max_body_bytes: 4096,
        max_inflight_requests: 1024,
        max_ws_connections: 1024,
    };
    let app = app_with(limits);

    let wire = StubWire::Create;
    let body = serde_json::to_vec(&wire).expect("serialize wire");
    let req = Request::builder()
        .method("POST")
        .uri("/v1/aggregates")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .expect("request build");

    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "small body must not trip max_body_bytes=4096; dispatch should succeed"
    );
}

/// SEC-0003:R3 (backpressure) — exhausting the inflight semaphore
/// returns `503`. Mechanism: CHE-0062 (`http_concurrency_limit`
/// 503-shedding middleware; **not** `tower::limit::ConcurrencyLimit`
/// which queues, per CHE-0062:R1).
///
/// With `max_inflight_requests = 0` `try_acquire` fails immediately on
/// every request: this proves the middleware is wired and exercises
/// the **shedding** branch without needing a notify-barrier to hold
/// two concurrent requests in flight. The donor crate's
/// `gh-report/.../server.rs:2164,:2209` validates the contended-
/// but-non-zero case in production; this smoke is sufficient to prove
/// the wiring on the cqrs surface.
#[tokio::test]
async fn cqrs_inflight_zero_permits_returns_503() {
    let limits = LayerLimits {
        max_body_bytes: 1024 * 1024,
        max_inflight_requests: 0,
        max_ws_connections: 1024,
    };
    let app = app_with(limits);

    let req = Request::builder()
        .uri("/v1/aggregates/1")
        .body(Body::empty())
        .expect("request build");

    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "max_inflight_requests=0 must shed every request with 503"
    );
}

/// Sanity: with permissive limits a normal GET reaches the handler and
/// returns the handler's status — confirms the layers don't
/// accidentally short-circuit valid traffic.
#[tokio::test]
async fn cqrs_permissive_limits_allow_load() {
    let app = app_with(LayerLimits::permissive_for_tests());

    let req = Request::builder()
        .uri("/v1/aggregates/1")
        .body(Body::empty())
        .expect("request build");

    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "permissive limits must not short-circuit valid traffic"
    );
}
