//! Smoke test for [`cherry_pit_web::CommandRouter`] integration.
//!
//! Exercises the full HTTP → router → response loop with minimal
//! stub `CommandGateway`, `EventStore`, and `CommandRouter` impls:
//!
//! - `POST /v1/aggregates` with a valid wire payload returns 201 and
//!   echoes the assigned aggregate id.
//! - `POST /v1/aggregates/:id/commands` with a valid wire payload
//!   returns 200.
//! - A wire payload signalling an error variant maps to a non-2xx
//!   status per CHE-0049 R6 / S3 (uses
//!   [`cherry_pit_web::map_dispatch_error`] under the hood).
//!
//! Heavyweight integration coverage against an in-memory `EventStore`
//! lands in S6; this test only proves the trait wires up end-to-end.

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
use cherry_pit_web::errors::{ErrorEnvelope, map_dispatch_error};
use cherry_pit_web::{AppState, CommandRouter, DispatchOutcome, LayerLimits, build_router};
use http_body_util::BodyExt;
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

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "test double answers from in-memory state with no I/O to await; the `async` keyword is dictated by the trait signature under test"
    )]
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

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "test double answers from in-memory state with no I/O to await; the `async` keyword is dictated by the trait signature under test"
    )]
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

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "test double answers from in-memory state with no I/O to await; the `async` keyword is dictated by the trait signature under test"
    )]
    async fn load(&self, _id: AggregateId) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        Ok(vec![])
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "test double answers from in-memory state with no I/O to await; the `async` keyword is dictated by the trait signature under test"
    )]
    async fn create(
        &self,
        _events: Vec<Self::Event>,
        _context: CorrelationContext,
    ) -> StoreCreateResult<Self::Event> {
        Err(StoreError::Infrastructure("stub store".into()))
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "test double answers from in-memory state with no I/O to await; the `async` keyword is dictated by the trait signature under test"
    )]
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
    /// Resolves to `DispatchOutcome::Created { aggregate_id: 1 }`.
    Create,
    /// Resolves to `DispatchOutcome::Sent`.
    Send,
    /// Resolves to `Err` via `map_dispatch_error(&Rejected(_))` — 422.
    RejectMe,
    /// Carries no target aggregate id at all.
    Untargeted,
}

#[derive(Clone)]
struct StubRouter;

impl CommandRouter for StubRouter {
    type Gateway = StubGateway;
    type Wire = StubWire;

    fn target_aggregate_id(wire: &Self::Wire) -> Option<AggregateId> {
        match wire {
            StubWire::Create | StubWire::Send | StubWire::RejectMe => {
                Some(AggregateId::new(NonZeroU64::new(1).unwrap()))
            }
            StubWire::Untargeted => None,
        }
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "test double answers from in-memory state with no I/O to await; the `async` keyword is dictated by the trait signature under test"
    )]
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
            StubWire::Send | StubWire::Untargeted => Ok(DispatchOutcome::Sent),
            StubWire::RejectMe => {
                let err: DispatchError<RejectErr> = DispatchError::Rejected(RejectErr("nope"));
                Err(map_dispatch_error(&err))
            }
        }
    }
}

#[derive(Debug)]
struct RejectErr(&'static str);
impl std::fmt::Display for RejectErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for RejectErr {}

fn app() -> Router {
    let state: AppState<StubGateway, StubStore, StubRouter> =
        AppState::new(StubGateway, StubStore, StubRouter);
    build_router(state, LayerLimits::permissive_for_tests(), Router::new())
}

async fn body_string(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn json_post(uri: &str, body: &StubWire) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

const MISROUTE_CORRELATION_ID: &str = "6f1d2c3b-4a59-4c7e-8b0d-1e2f3a4b5c6d";

fn correlated_json_post(uri: &str, body: &StubWire) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-correlation-id", MISROUTE_CORRELATION_ID)
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

#[tokio::test]
async fn create_endpoint_returns_201_with_aggregate_id() {
    let response = app()
        .oneshot(json_post("/v1/aggregates", &StubWire::Create))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let body = body_string(response).await;
    assert!(
        body.contains(r#""aggregate_id":1"#),
        "201 body must echo the assigned aggregate id: {body}"
    );
}

#[tokio::test]
async fn send_endpoint_returns_200() {
    let response = app()
        .oneshot(json_post("/v1/aggregates/1/commands", &StubWire::Send))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn rejected_error_maps_to_422() {
    let response = app()
        .oneshot(json_post("/v1/aggregates", &StubWire::RejectMe))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let body = body_string(response).await;
    assert!(
        body.contains(r#""code":"rejected""#),
        "error body must carry the stable code: {body}"
    );
}

#[tokio::test]
async fn create_endpoint_misroute_body_carries_the_request_correlation_id() {
    let response = app()
        .oneshot(correlated_json_post("/v1/aggregates", &StubWire::Send))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = body_string(response).await;
    assert!(
        body.contains(r#""code":"router_misroute""#),
        "misroute body must carry the stable code: {body}"
    );
    assert!(
        body.contains(&format!(r#""correlation_id":"{MISROUTE_CORRELATION_ID}""#)),
        "misroute body must carry the request correlation id: {body}"
    );
}

#[tokio::test]
async fn send_endpoint_misroute_body_carries_the_request_correlation_id() {
    let response = app()
        .oneshot(correlated_json_post(
            "/v1/aggregates/1/commands",
            &StubWire::Create,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = body_string(response).await;
    assert!(
        body.contains(r#""code":"router_misroute""#),
        "misroute body must carry the stable code: {body}"
    );
    assert!(
        body.contains(&format!(r#""correlation_id":"{MISROUTE_CORRELATION_ID}""#)),
        "misroute body must carry the request correlation id: {body}"
    );
}

#[tokio::test]
async fn misroute_body_elides_correlation_id_when_the_request_carries_none() {
    let response = app()
        .oneshot(json_post("/v1/aggregates", &StubWire::Send))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = body_string(response).await;
    assert!(
        !body.contains("correlation_id"),
        "CHE-0039 R2 forbids synthesising a correlation id: {body}"
    );
}

#[tokio::test]
async fn send_endpoint_rejects_a_path_id_that_disagrees_with_the_body_target() {
    let response = app()
        .oneshot(json_post("/v1/aggregates/999/commands", &StubWire::Send))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "path id 999 disagrees with the body target 1 and must be rejected"
    );
    let body = body_string(response).await;
    assert!(
        body.contains(r#""code":"path_body_target_mismatch""#),
        "mismatch must carry its own stable code: {body}"
    );
}

#[tokio::test]
async fn send_endpoint_rejects_a_body_that_declares_no_target() {
    let response = app()
        .oneshot(json_post(
            "/v1/aggregates/1/commands",
            &StubWire::Untargeted,
        ))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "an absent body target is unverifiable, not verified; it must not fail open"
    );
    let body = body_string(response).await;
    assert!(
        body.contains(r#""code":"path_body_target_absent""#),
        "absent target must be distinguishable from a mismatch: {body}"
    );
}

#[tokio::test]
async fn send_endpoint_rejects_a_non_numeric_path_id() {
    let response = app()
        .oneshot(json_post(
            "/v1/aggregates/not-a-number/commands",
            &StubWire::Send,
        ))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "a malformed :id must 400 in the extractor before the router runs"
    );
}

#[tokio::test]
async fn send_endpoint_rejects_a_zero_path_id() {
    let response = app()
        .oneshot(json_post("/v1/aggregates/0/commands", &StubWire::Send))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "CHE-0011:R2 puts the zero-check at the integer-entry boundary"
    );
}
