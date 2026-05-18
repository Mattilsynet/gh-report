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
use cherry_pit_web::{AppState, CommandRouter, DispatchOutcome, build_router};
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use tower::ServiceExt;

// ── Stub domain ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
enum StubEvent {
    Noop,
}

impl DomainEvent for StubEvent {
    fn event_type(&self) -> &'static str {
        "stub.noop"
    }
}

impl pardosa_encoding::Encode for StubEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Noop => out.push(0u8),
        }
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

// ── Stub gateway ──────────────────────────────────────────────────────
//
// Inert: the smoke router never calls `create` / `send` (it short-
// circuits in `dispatch` based on the wire payload). We still need a
// real impl because `AppState<G, S, R>` requires a concrete `G`.

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

// ── Stub store ────────────────────────────────────────────────────────
//
// Same rationale as the gateway: present to satisfy the `(G, S, R)`
// arity, never invoked by the smoke router.

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

// ── Stub router ───────────────────────────────────────────────────────
//
// The wire DTO carries a minimal discriminator the test uses to drive
// each branch of the response mapping in `router.rs` without needing
// actual gateway behaviour.

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StubWire {
    /// Resolves to `DispatchOutcome::Created { aggregate_id: 1 }`.
    Create,
    /// Resolves to `DispatchOutcome::Sent`.
    Send,
    /// Resolves to `Err` via `map_dispatch_error(&Rejected(_))` — 422.
    RejectMe,
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
            StubWire::Send => Ok(DispatchOutcome::Sent),
            StubWire::RejectMe => {
                // Mirror the production path: build the envelope
                // through the public mapper rather than constructing
                // it ad-hoc — preserves CHE-0049 R10 fidelity in the
                // smoke test.
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

// ── Test harness ──────────────────────────────────────────────────────

fn app() -> Router {
    let state: AppState<StubGateway, StubStore, StubRouter> =
        AppState::new(StubGateway, StubStore, StubRouter);
    build_router(state, Router::new())
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
    // CHE-0049 R6 / S3: DispatchError::Rejected → 422 Unprocessable Entity.
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
