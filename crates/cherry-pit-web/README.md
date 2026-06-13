# cherry-pit-web

HTTP adapter exposing the cherry-pit `CommandGateway` over [axum].

This crate realises **CHE-0049** (cherry-pit-web design): translates HTTP
requests into domain commands, dispatches them via the gateway, and maps
outcomes to HTTP responses with correlation propagation (CHE-0039) and a
stable JSON error contract (CHE-0015). It also realises **CHE-0050**
(`CommandRouter` port): wire-deserialize-and-dispatch is consumer-owned and
threaded through `AppState` as a third type parameter `R`.

## Role

`cherry-pit-web` is the HTTP-edge adapter for command-side traffic. It owns
status mapping, correlation echo, idempotency-key threading, and the
versioned `/v1/` DTO contract; it **does not** own auth, WebSocket surfaces,
or static-content caching. Consumers attach those via the `extra_routes`
merge point (CHE-0049 R2 / R3 / R8).

It is a sibling adapter crate to `cherry-pit-projection` and depends on
`cherry-pit-core`. It does not stack on the gateway implementation — the
consumer constructs both and hands them in via `AppState`.

## Public API summary

Top-level (always available):

- `AppState<G, S, R>` — generic typed state per CHE-0049 R1 + CHE-0050 R2.
  No `Box<dyn _>`; the trio of gateway, store, and router is monomorphised
  end-to-end.
- `build_router` — assembles the axum router with cherry-pit-web's routes
  mounted under `/v1/` per CHE-0049 R9 and the consumer's `extra_routes`
  merged at the top level.
- `CommandRouter`, `DispatchOutcome` — the consumer-owned port (CHE-0050
  R1). Object-unsafe by construction (R4); zero blanket impls.
- `compute_etag`, `compress_zstd`, `security_headers`,
  `normalize_request_path`, `sanitize_path_segment` — the five R8 utility
  primitives, flat-re-exported per CHE-0049:R14.

Per CHE-0049:R14 + CHE-0030:R1/R2 the `middleware` module itself is
implementation detail. Other deliberate-public items reach consumers
through three dedicated submodules:

- `cherry_pit_web::errors` — `ErrorBody`, `ErrorEnvelope`,
  `map_dispatch_error`, `map_store_error`, `map_bus_error`,
  `post_persist_cancellation_response`. JSON shape and triple form for
  error responses; single source of truth for CHE-0049:R4 + R10 status
  mapping.
- `cherry_pit_web::correlation` — `correlation_layer`,
  `correlation_from_extensions`, `extract_correlation`,
  `extract_idempotency_key`, `IdempotencyKey`. Header-driven
  propagation per CHE-0049:R5 + R6; never auto-generates an
  idempotency key (CHE-0046:R3).
- `cherry_pit_web::path` — `PathSegmentError` (companion error type
  for the path utilities above).

Under `feature = "projection"`, see the [Projection surface](#projection-surface-feature--projection) section below.

## Minimal usage

```rust,no_run
use std::num::NonZeroU64;
use axum::Router;
use cherry_pit_core::{
    Aggregate, AggregateId, Command, CommandGateway, CorrelationContext,
    CreateResult, DispatchResult, DomainEvent, EventEnvelope, EventStore,
    HandleCommand, StoreCreateResult, StoreError,
};
use cherry_pit_web::{AppState, CommandRouter, DispatchOutcome, build_router};
use cherry_pit_web::correlation::IdempotencyKey;
use cherry_pit_web::errors::ErrorEnvelope;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CounterEvent { Created }
impl DomainEvent for CounterEvent {
    fn event_type(&self) -> &'static str { "counter.created" }
}

#[derive(Default)]
struct Counter;
impl Aggregate for Counter {
    type Event = CounterEvent;
    fn apply(&mut self, _: &Self::Event) {}
}

#[derive(Debug)] struct CreateCmd;
impl Command for CreateCmd {}
impl HandleCommand<CreateCmd> for Counter {
    type Error = std::convert::Infallible;
    fn handle(&self, _: CreateCmd) -> Result<Vec<CounterEvent>, Self::Error> {
        Ok(vec![CounterEvent::Created])
    }
}

# struct G;
# impl CommandGateway for G {
#     type Aggregate = Counter;
#     async fn create<C>(&self, _: C, _: CorrelationContext) -> CreateResult<Counter, C>
#         where Counter: HandleCommand<C>, C: Command
#     { Ok((AggregateId::new(NonZeroU64::new(1).unwrap()), vec![])) }
#     async fn send<C>(&self, _: AggregateId, _: C, _: CorrelationContext) -> DispatchResult<Counter, C>
#         where Counter: HandleCommand<C>, C: Command
#     { Ok(vec![]) }
# }
# impl Clone for G { fn clone(&self) -> Self { G } }
# struct S;
# impl EventStore for S {
#     type Event = CounterEvent;
#     async fn load(&self, _: AggregateId) -> Result<Vec<EventEnvelope<CounterEvent>>, StoreError> { Ok(vec![]) }
#     async fn create(&self, _: Vec<CounterEvent>, _: CorrelationContext) -> StoreCreateResult<CounterEvent> {
#         Ok((AggregateId::new(NonZeroU64::new(1).unwrap()), vec![]))
#     }
#     async fn append(&self, _: AggregateId, _: NonZeroU64, _: Vec<CounterEvent>, _: CorrelationContext)
#         -> Result<Vec<EventEnvelope<CounterEvent>>, StoreError>
#     { Ok(vec![]) }
# }
#[derive(Deserialize)] struct Wire;
#[derive(Clone)] struct CounterRouter;
impl CommandRouter for CounterRouter {
    type Gateway = G;
    type Wire = Wire;
    async fn dispatch(
        &self,
        gateway: &G,
        ctx: CorrelationContext,
        _idempotency: Option<IdempotencyKey>,
        _wire: Wire,
    ) -> Result<DispatchOutcome, ErrorEnvelope> {
        let (aggregate_id, _) = gateway.create(CreateCmd, ctx).await.unwrap();
        Ok(DispatchOutcome::Created { aggregate_id })
    }
}

# async fn run(my_gateway: G, my_store: S, my_router: CounterRouter) {
let state = AppState::new(my_gateway, my_store, my_router);
let app: Router = build_router(state, Router::new());
// `axum::serve(listener, app).await` — out of scope for this crate.
# let _ = app;
# }
```

For the heavier exercise — full create/send/load round-trips against an
in-memory `EventStore` — see `tests/integration_inmem.rs`.

## Projection surface (`feature = "projection"`)

Under the optional `projection` feature the crate exposes a **second**
axum router — `build_projection_router<P>` — alongside the default
command-side `build_router`. This is the read-side adapter realising
CHE-0049 R11–R14: HTTP snapshot fetch plus a narrowed WebSocket push
for snapshot deltas. Without the feature, none of the items below are
compiled.

### Rationale for the feature gate

The projection surface adds an axum WebSocket dependency edge and a
broadcast-channel runtime obligation that command-side consumers do
not need. Keeping it behind `feature = "projection"` preserves the
narrow default surface (HTTP-only, no WS) while letting projection-
aware consumers opt in. The flag is additive — enabling it does not
change any item already exposed by the default build.

```toml
[dependencies]
cherry-pit-web = { version = "0.1", features = ["projection"] }
```

### Public entry points

All re-exported flat from the crate root under the feature gate:

- `ProjectionSource` — port trait the consumer implements to expose a
  snapshot (`HashMap<String, PageEntry>` shared via `Arc`), a
  `broadcast::Receiver<PageUpdate>` for deltas, and a readiness flag.
  **Sealed against `dyn` use** per CHE-0049:R12 + CHE-0005:R1: a
  generic-method seal makes the trait not dyn-compatible, so
  `Box<dyn ProjectionSource>` fails to compile (E0038, locked by a
  trybuild test under `tests/compile_fail/`). The intended use is the
  generic parameter `P: ProjectionSource`.
- `ProjectionState<P>` — typed state mirroring `AppState`'s no-erasure
  posture.
- `PageEntry`, `PageUpdate` — snapshot and delta payload shapes.
- `build_projection_router<P>` — constructs the axum router that
  mounts:
  - `GET /v1/healthz` — liveness (static `{"v":1,"status":"ok"}`).
  - `GET /v1/readyz` — readiness mapped from
    `ProjectionSource::is_ready`.
  - `GET /v1/{*path}` — snapshot fetch with ETag/304 + zstd
    negotiation, reusing the R8 utilities `compute_etag` /
    `compress_zstd`.
  - `GET /ws` — WebSocket upgrade subscribing to
    `ProjectionSource::subscribe`. The WS envelope carries `"v": 1`
    per CHE-0049:R13. On `broadcast::RecvError::Lagged` the per-socket
    task closes the WS with code 1001 ("Going Away"); clients recover
    by re-fetching the snapshot then re-attaching a fresh WS — the
    snapshot is the durable checkpoint per CHE-0048:R2.
- `ServerConfig`, `ServerConfigBuilder`, `ValidatedConfig`,
  `ConfigError`, `ServerError` — projection-side server configuration
  (bind address, feature toggles). `ValidatedConfig` is the
  parse-don't-validate output of `ServerConfigBuilder::build`.

### Wiring sketch

```rust,no_run
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use axum::Router;
use tokio::sync::broadcast;
use cherry_pit_web::{
    PageEntry, PageUpdate, ProjectionSource, ProjectionState,
    build_projection_router,
};

struct MyProjection {
    snapshot: Mutex<Option<Arc<HashMap<String, PageEntry>>>>,
    tx: broadcast::Sender<PageUpdate>,
}

impl ProjectionSource for MyProjection {
    fn snapshot(&self) -> Option<Arc<HashMap<String, PageEntry>>> {
        self.snapshot.lock().unwrap().clone()
    }
    fn subscribe(&self) -> broadcast::Receiver<PageUpdate> {
        self.tx.subscribe()
    }
    fn is_ready(&self) -> bool {
        self.snapshot.lock().unwrap().is_some()
    }
}

# fn build(source: Arc<MyProjection>) -> Router {
// `ProjectionState::from_arc` accepts an already-shared source so the
// consumer keeps a handle for delta injection (`tx.send(...)`); use
// `ProjectionState::new(MyProjection { .. })` when the source is
// owned outright.
let state = ProjectionState::from_arc(source);
let app: Router = build_projection_router(state);
// `axum::serve(listener, app).await` — out of scope for this crate.
app
# }
```

Per CHE-0014:R2 the port carries no `Deserialize` bound on any domain
type. Per CHE-0049:R12 the only supported consumption shape is the
generic `P: ProjectionSource` — both `ProjectionState` and
`build_projection_router` are generic in `P`, with the trait
structurally sealed against trait-object use.

## Out of scope (v0.1)

- **Auth.** No middleware, no extractors, no defaults. Attach via
  `extra_routes` (CHE-0049 R2).
- **Transports beyond HTTP/1.1+JSON on the command surface.** The
  default (cqrs) router is HTTP-only — no WebSocket, no SSE, no gRPC
  on the command path (CHE-0049 R3). The `projection` feature adds a
  narrowed WebSocket upgrade for snapshot-delta push only (CHE-0049
  R11); see the [Projection surface](#projection-surface-feature--projection)
  section above.
- **Static content.** No router-built-in cache or asset serving (CHE-0049
  R8).
- **Idempotency persistence.** The `Idempotency-Key` header is extracted
  and threaded; consumer routers persist replay state per CHE-0046 R3.

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE))
- MIT license ([LICENSE-MIT](../../LICENSE-MIT))

at your option.

[axum]: https://docs.rs/axum
