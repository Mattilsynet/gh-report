//! End-to-end integration tests against an in-memory `EventStore`.
//!
//! S6 of the WU-4 cherry-pit-web mission package. Builds a real
//! `CommandGateway` over a real `EventStore` (both confined to this
//! `tests/` file — never re-exported from the crate's public surface
//! per CHE-0049 R1 and the WU-4 brief) and exercises the full
//! HTTP → router → gateway → store round-trip:
//!
//! - **Create** — `POST /v1/aggregates` produces 201 + assigned id;
//!   the store now contains the produced events.
//! - **Send** — `POST /v1/aggregates/:id/commands` against the
//!   created aggregate produces 200 and advances the stream.
//! - **Load known** — `GET /v1/aggregates/:id` returns the persisted
//!   event stream as a flat JSON array.
//! - **Load unknown** — `GET /v1/aggregates/:id` against an unseen id
//!   returns **200 with an empty payload** per **CHE-0049 R7** +
//!   **CHE-0019 R1** (never 404 from a read).
//! - **Error path** — a wire payload that drives the gateway into
//!   `DispatchError::Rejected` returns 422 per **CHE-0049 R4 + R6**.
//! - **Correlation echo** — a request carrying `X-Correlation-ID`
//!   arrives at the test router with a populated `CorrelationContext`,
//!   the produced events carry that correlation id (the store stamps
//!   it onto each envelope per CHE-0016), and the response echoes the
//!   same header back per **CHE-0049 R5**.

use std::convert::Infallible;
use std::error::Error;
use std::fmt;
use std::num::NonZeroU64;
use std::sync::Mutex;

use axum::{
    Router,
    body::Body,
    http::{HeaderValue, Request, StatusCode},
};
use cherry_pit_core::{
    Aggregate, AggregateId, Command, CommandGateway, CorrelationContext, DispatchError,
    DispatchResult, DomainEvent, EventEnvelope, EventStore, HandleCommand, StoreCreateResult,
    StoreError,
};
use cherry_pit_web::errors::{ErrorEnvelope, map_dispatch_error, map_store_error};
use cherry_pit_web::{AppState, CommandRouter, DispatchOutcome, build_router};
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use tower::ServiceExt;
use uuid::Uuid;

// ── Domain ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum CounterEvent {
    Created { initial: u32 },
    Incremented { by: u32 },
}

impl DomainEvent for CounterEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Created { .. } => "counter.created",
            Self::Incremented { .. } => "counter.incremented",
        }
    }
}

#[derive(Default)]
struct Counter {
    count: u32,
}

impl Aggregate for Counter {
    type Event = CounterEvent;
    fn apply(&mut self, event: &Self::Event) {
        match event {
            CounterEvent::Created { initial } => self.count = *initial,
            CounterEvent::Incremented { by } => self.count += *by,
        }
    }
}

#[derive(Debug)]
struct CreateCmd {
    initial: u32,
}
impl Command for CreateCmd {}

impl HandleCommand<CreateCmd> for Counter {
    type Error = Infallible;
    fn handle(&self, cmd: CreateCmd) -> Result<Vec<Self::Event>, Self::Error> {
        Ok(vec![CounterEvent::Created {
            initial: cmd.initial,
        }])
    }
}

#[derive(Debug)]
struct IncrementCmd {
    by: u32,
}
impl Command for IncrementCmd {}

#[derive(Debug)]
struct IncrementError(&'static str);
impl fmt::Display for IncrementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl Error for IncrementError {}

impl HandleCommand<IncrementCmd> for Counter {
    type Error = IncrementError;
    fn handle(&self, cmd: IncrementCmd) -> Result<Vec<Self::Event>, Self::Error> {
        if cmd.by == 0 {
            return Err(IncrementError("increment by zero is rejected"));
        }
        Ok(vec![CounterEvent::Incremented { by: cmd.by }])
    }
}

// ── In-memory `EventStore` (test-only) ────────────────────────────────
//
// Confined to this `tests/` file per the WU-4 S6 brief — never
// `pub use`'d from the crate. Single-process Mutex is fine: tests run
// each request through a fresh `app()` and assertions read the store
// after the await point completes, so there's no real concurrency.

#[derive(Default)]
struct InMemStore {
    inner: Mutex<InMemInner>,
}

#[derive(Default)]
struct InMemInner {
    next_id: u64,
    streams: Vec<(AggregateId, Vec<EventEnvelope<CounterEvent>>)>,
}

impl InMemStore {
    fn new() -> Self {
        Self::default()
    }

    /// Read-only snapshot of one aggregate's stream — for assertions.
    fn snapshot(&self, id: AggregateId) -> Vec<EventEnvelope<CounterEvent>> {
        let guard = self.inner.lock().unwrap();
        guard
            .streams
            .iter()
            .find(|(k, _)| *k == id)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }
}

fn build_envelopes(
    aggregate_id: AggregateId,
    starting_sequence: u64,
    events: Vec<CounterEvent>,
    context: &CorrelationContext,
) -> Result<Vec<EventEnvelope<CounterEvent>>, StoreError> {
    let mut out = Vec::with_capacity(events.len());
    for (offset, ev) in events.into_iter().enumerate() {
        let seq_raw = starting_sequence + offset as u64;
        let seq = NonZeroU64::new(seq_raw).ok_or_else(|| {
            StoreError::Infrastructure(format!("invalid zero sequence at offset {offset}").into())
        })?;
        let envelope = EventEnvelope::new(
            Uuid::now_v7(),
            aggregate_id,
            seq,
            jiff::Timestamp::now(),
            context.correlation_id(),
            context.causation_id(),
            ev,
        )
        .map_err(|e| StoreError::Infrastructure(Box::new(e)))?;
        out.push(envelope);
    }
    Ok(out)
}

impl EventStore for InMemStore {
    type Event = CounterEvent;

    async fn load(&self, id: AggregateId) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        // CHE-0019 R1: unknown aggregate yields an empty Vec, not an error.
        Ok(self.snapshot(id))
    }

    async fn create(
        &self,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> StoreCreateResult<Self::Event> {
        if events.is_empty() {
            return Err(StoreError::Infrastructure(
                "cannot create aggregate with empty event list".into(),
            ));
        }
        let mut guard = self.inner.lock().unwrap();
        guard.next_id += 1;
        let raw_id = guard.next_id;
        let aggregate_id = AggregateId::new(NonZeroU64::new(raw_id).unwrap());
        let envelopes = build_envelopes(aggregate_id, 1, events, &context)?;
        guard.streams.push((aggregate_id, envelopes.clone()));
        Ok((aggregate_id, envelopes))
    }

    async fn append(
        &self,
        id: AggregateId,
        expected_sequence: NonZeroU64,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        if events.is_empty() {
            return Ok(vec![]);
        }
        let mut guard = self.inner.lock().unwrap();
        let stream = guard
            .streams
            .iter_mut()
            .find(|(k, _)| *k == id)
            .ok_or_else(|| {
                StoreError::Infrastructure(format!("append to never-created aggregate {id}").into())
            })?;
        let actual_sequence = stream.1.len() as u64;
        if actual_sequence != expected_sequence.get() {
            return Err(StoreError::ConcurrencyConflict {
                aggregate_id: id,
                expected_sequence,
                actual_sequence,
            });
        }
        let envelopes = build_envelopes(id, actual_sequence + 1, events, &context)?;
        stream.1.extend(envelopes.iter().cloned());
        Ok(envelopes)
    }
}

// ── In-memory `CommandGateway` over the store ─────────────────────────
//
// The cherry-pit-core gateway trait is generic over `C: Command`. We
// implement the load → handle → persist lifecycle directly against
// the store so each round-trip exercises real persistence, not the
// stub from `command_router_smoke.rs`.

struct InMemGateway {
    store: std::sync::Arc<InMemStore>,
}

impl InMemGateway {
    fn new(store: std::sync::Arc<InMemStore>) -> Self {
        Self { store }
    }
}

impl CommandGateway for InMemGateway {
    type Aggregate = Counter;

    async fn create<C>(
        &self,
        cmd: C,
        context: CorrelationContext,
    ) -> cherry_pit_core::CreateResult<Self::Aggregate, C>
    where
        Self::Aggregate: HandleCommand<C>,
        C: Command,
    {
        let agg = Counter::default();
        let events = agg.handle(cmd).map_err(DispatchError::Rejected)?;
        let (id, envelopes) = self
            .store
            .create(events, context)
            .await
            .map_err(|e| DispatchError::Infrastructure(Box::new(e)))?;
        Ok((id, envelopes))
    }

    async fn send<C>(
        &self,
        id: AggregateId,
        cmd: C,
        context: CorrelationContext,
    ) -> DispatchResult<Self::Aggregate, C>
    where
        Self::Aggregate: HandleCommand<C>,
        C: Command,
    {
        let history = self
            .store
            .load(id)
            .await
            .map_err(|e| DispatchError::Infrastructure(Box::new(e)))?;
        if history.is_empty() {
            return Err(DispatchError::AggregateNotFound { aggregate_id: id });
        }
        let mut agg = Counter::default();
        for env in &history {
            agg.apply(env.payload());
        }
        let last_seq = history.last().map(EventEnvelope::sequence).ok_or_else(|| {
            DispatchError::Infrastructure("loaded stream had zero sequence on tail event".into())
        })?;
        let new_events = agg.handle(cmd).map_err(DispatchError::Rejected)?;
        let envelopes = self
            .store
            .append(id, last_seq, new_events, context)
            .await
            .map_err(|e| match e {
                StoreError::ConcurrencyConflict {
                    aggregate_id,
                    expected_sequence,
                    actual_sequence,
                } => DispatchError::ConcurrencyConflict {
                    aggregate_id,
                    expected_sequence,
                    actual_sequence,
                },
                other => DispatchError::Infrastructure(Box::new(other)),
            })?;
        Ok(envelopes)
    }
}

// ── Wire DTO + `CommandRouter` impl ───────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CounterWire {
    /// `POST /v1/aggregates` — create with an initial counter value.
    Create { initial: u32 },
    /// `POST /v1/aggregates/:id/commands` — increment by `by`. The
    /// router uses `target` as the aggregate id (the path `:id` is
    /// not threaded into `dispatch` per CHE-0050 R1).
    Increment { target: u64, by: u32 },
}

/// Per-harness slot capturing the last [`CorrelationContext`] seen
/// by [`InMemRouter::dispatch`], so tests can prove the context
/// flowed through extraction → middleware → router → gateway intact
/// (CHE-0049 R5).
///
/// Per-instance (not a `static`) so parallel tokio tests under
/// `cargo test` don't race each other through a shared global slot.
#[derive(Clone, Default)]
struct InMemRouter {
    last_correlation: std::sync::Arc<Mutex<Option<CorrelationContext>>>,
}

impl InMemRouter {
    fn new() -> Self {
        Self::default()
    }

    fn take_last_correlation(&self) -> Option<CorrelationContext> {
        self.last_correlation.lock().unwrap().take()
    }
}

impl CommandRouter for InMemRouter {
    type Gateway = InMemGateway;
    type Wire = CounterWire;

    async fn dispatch(
        &self,
        gateway: &Self::Gateway,
        ctx: CorrelationContext,
        _idempotency: Option<cherry_pit_web::correlation::IdempotencyKey>,
        wire: Self::Wire,
    ) -> Result<DispatchOutcome, ErrorEnvelope> {
        *self.last_correlation.lock().unwrap() = Some(ctx.clone());
        match wire {
            CounterWire::Create { initial } => {
                match gateway.create(CreateCmd { initial }, ctx).await {
                    Ok((aggregate_id, _envelopes)) => Ok(DispatchOutcome::Created { aggregate_id }),
                    Err(err) => Err(map_dispatch_error(&err)),
                }
            }
            CounterWire::Increment { target, by } => {
                let id = AggregateId::new(NonZeroU64::new(target).ok_or_else(|| {
                    // Map invalid target to a not-found-shaped error so
                    // we exercise the public mapping helpers consistently.
                    let err: DispatchError<IncrementError> =
                        DispatchError::Infrastructure("wire target must be non-zero".into());
                    map_dispatch_error(&err)
                })?);
                match gateway.send(id, IncrementCmd { by }, ctx).await {
                    Ok(_envelopes) => Ok(DispatchOutcome::Sent),
                    Err(err) => Err(map_dispatch_error(&err)),
                }
            }
        }
    }
}

// ── Test harness ──────────────────────────────────────────────────────

struct Harness {
    app: Router,
    store: std::sync::Arc<InMemStore>,
    router: InMemRouter,
}

fn harness() -> Harness {
    let store = std::sync::Arc::new(InMemStore::new());
    let gateway = InMemGateway::new(std::sync::Arc::clone(&store));
    let router = InMemRouter::new();
    let state: AppState<InMemGateway, InMemStore, InMemRouter> = AppState::from_arcs(
        std::sync::Arc::new(gateway),
        std::sync::Arc::clone(&store),
        router.clone(),
    );
    let app = build_router(state, Router::new());
    Harness { app, store, router }
}

#[expect(
    dead_code,
    reason = "test helper retained for symmetry with the WU-4 brief's HTTP-response assertion vocabulary; not exercised by every test, but kept inline so any future assertion needing body bytes can use it without a separate refactor."
)]
async fn body_string(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

fn json_post(uri: &str, body: &CounterWire) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

// ── Tests ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_round_trip_persists_events_in_store() {
    let h = harness();
    let response = h
        .app
        .clone()
        .oneshot(json_post(
            "/v1/aggregates",
            &CounterWire::Create { initial: 7 },
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let body = body_json(response).await;
    let assigned = body
        .get("aggregate_id")
        .and_then(serde_json::Value::as_u64)
        .expect("create body must echo aggregate_id");
    assert_eq!(assigned, 1, "first aggregate gets id 1");

    let id = AggregateId::new(NonZeroU64::new(assigned).unwrap());
    let stream = h.store.snapshot(id);
    assert_eq!(stream.len(), 1, "create must persist one event");
    assert_eq!(stream[0].sequence().get(), 1);
    assert_eq!(*stream[0].payload(), CounterEvent::Created { initial: 7 });
}

#[tokio::test]
async fn send_round_trip_advances_event_stream() {
    let h = harness();
    // First create.
    let _ = h
        .app
        .clone()
        .oneshot(json_post(
            "/v1/aggregates",
            &CounterWire::Create { initial: 0 },
        ))
        .await
        .unwrap();

    // Then send an Increment.
    let response = h
        .app
        .clone()
        .oneshot(json_post(
            "/v1/aggregates/1/commands",
            &CounterWire::Increment { target: 1, by: 3 },
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let id = AggregateId::new(NonZeroU64::new(1).unwrap());
    let stream = h.store.snapshot(id);
    assert_eq!(stream.len(), 2, "create + increment = 2 events");
    assert_eq!(stream[1].sequence().get(), 2);
    assert_eq!(*stream[1].payload(), CounterEvent::Incremented { by: 3 });
}

#[tokio::test]
async fn load_known_aggregate_returns_event_stream() {
    let h = harness();
    let _ = h
        .app
        .clone()
        .oneshot(json_post(
            "/v1/aggregates",
            &CounterWire::Create { initial: 11 },
        ))
        .await
        .unwrap();
    let _ = h
        .app
        .clone()
        .oneshot(json_post(
            "/v1/aggregates/1/commands",
            &CounterWire::Increment { target: 1, by: 4 },
        ))
        .await
        .unwrap();

    let response = h
        .app
        .clone()
        .oneshot(get("/v1/aggregates/1"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_json(response).await;
    assert_eq!(body["aggregate_id"], serde_json::json!(1));
    let events = body["events"].as_array().expect("events array");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["event_type"], "counter.created");
    assert_eq!(events[0]["sequence"], serde_json::json!(1));
    assert_eq!(events[1]["event_type"], "counter.incremented");
    assert_eq!(events[1]["sequence"], serde_json::json!(2));
}

#[tokio::test]
async fn load_unknown_aggregate_returns_200_with_empty_events() {
    // CHE-0049 R7 + CHE-0019 R1: unknown aggregate is a 200, never 404.
    let h = harness();
    let response = h
        .app
        .clone()
        .oneshot(get("/v1/aggregates/999"))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "load of unknown aggregate must be 200 per CHE-0049 R7, not 404"
    );

    let body = body_json(response).await;
    assert_eq!(body["aggregate_id"], serde_json::json!(999));
    assert_eq!(
        body["events"],
        serde_json::json!([]),
        "unknown aggregate yields an empty events list per CHE-0019 R1"
    );
}

#[tokio::test]
async fn rejected_command_maps_to_422_end_to_end() {
    // CHE-0049 R4 + R6: domain-rejected commands return 422 + the
    // typed error preserved losslessly in the body.
    let h = harness();
    let _ = h
        .app
        .clone()
        .oneshot(json_post(
            "/v1/aggregates",
            &CounterWire::Create { initial: 0 },
        ))
        .await
        .unwrap();

    // `IncrementCmd { by: 0 }` triggers `IncrementError("...zero...")`.
    let response = h
        .app
        .clone()
        .oneshot(json_post(
            "/v1/aggregates/1/commands",
            &CounterWire::Increment { target: 1, by: 0 },
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let body = body_json(response).await;
    assert_eq!(body["code"], "rejected");
    let message = body["message"].as_str().expect("message string");
    assert!(
        message.contains("increment by zero is rejected"),
        "422 body must preserve the typed error Display: {message}"
    );

    // The store stream must still be at length 1 — the rejected
    // increment never reached `append`.
    let id = AggregateId::new(NonZeroU64::new(1).unwrap());
    assert_eq!(h.store.snapshot(id).len(), 1);
}

#[tokio::test]
async fn correlation_round_trip_propagates_and_echoes() {
    // CHE-0049 R5: inbound X-Correlation-ID is parsed into the
    // CorrelationContext seen by the router AND echoed on the response
    // header. CHE-0016 propagates it onto persisted envelopes.
    let h = harness();
    let corr = Uuid::now_v7();

    let request = Request::builder()
        .method("POST")
        .uri("/v1/aggregates")
        .header("content-type", "application/json")
        .header("x-correlation-id", corr.to_string())
        .body(Body::from(
            serde_json::to_vec(&CounterWire::Create { initial: 1 }).unwrap(),
        ))
        .unwrap();

    let response = h.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    // Echo header on the response.
    assert_eq!(
        response.headers().get("x-correlation-id"),
        Some(&HeaderValue::from_str(&corr.to_string()).unwrap()),
        "response must echo the inbound correlation id"
    );

    // The router observed the same correlation id.
    let observed = h
        .router
        .take_last_correlation()
        .expect("router must have recorded a CorrelationContext");
    assert_eq!(observed.correlation_id(), Some(corr));

    // The store stamped it onto the persisted envelope.
    let id = AggregateId::new(NonZeroU64::new(1).unwrap());
    let stream = h.store.snapshot(id);
    assert_eq!(stream.len(), 1);
    assert_eq!(stream[0].correlation_id(), Some(corr));
}

// ── Compile-time trait reach (re-stated) ──────────────────────────────
//
// `map_store_error` is exercised indirectly through the load handler;
// keep a direct reference here so a future refactor that drops the
// public mapper from the surface fails this file at compile time as
// well as the existing unit tests. #[expect] fails closed: if the
// reachability check ever gains a real caller, this attribute fires
// as unfulfilled and must be removed.
#[expect(
    dead_code,
    reason = "compile-time reachability anchor for `map_store_error`; the function body is the assertion that the public mapper still type-checks at the test crate's call site."
)]
fn public_mappers_remain_reachable() {
    let err = StoreError::Infrastructure("x".into());
    let _ = map_store_error(&err);
}
