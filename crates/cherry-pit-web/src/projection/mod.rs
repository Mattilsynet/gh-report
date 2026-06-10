//! Projection adapter (m5-projection-port; CHE-0049 R11–R14).
//!
//! This module is gated behind `feature = "projection"` and realises the
//! read-side adapter the web layer mounts as a second axum surface
//! alongside the default (`cqrs`) command-gateway surface:
//!
//! - [`port::ProjectionSource`] — port trait consumers implement to
//!   expose snapshot + subscribe APIs to the web layer. No `serde`
//!   decode-bound bleed onto domain types (CHE-0014 R2; closes A3).
//! - [`state::ProjectionState<P>`] — typed state per CHE-0005 R1 +
//!   CHE-0049 R12 (no `Box<dyn …>`, no trait objects).
//! - [`build_projection_router`] — axum router constructor; mounts
//!   `/v1/healthz`, `/v1/readyz`, `/v1/{*path}` HTTP routes per CHE-0049
//!   R9 and an unversioned `/ws` WebSocket upgrade. The WS envelope
//!   carries `"v": 1` per CHE-0049 R13.
//!
//! Phase 3d wires the full handler set + WS upgrade per CHE-0049 R11
//! (drop-and-resync via WS close code 1001 on
//! [`tokio::sync::broadcast::error::RecvError::Lagged`]).

pub(crate) mod config;
pub(crate) mod error;
pub(crate) mod handlers;
pub(crate) mod port;
pub(crate) mod state;

pub use config::{ConfigError, ServerConfig, ServerConfigBuilder, ValidatedConfig};
pub use error::ServerError;
pub use port::ProjectionSource;
pub use state::{PageEntry, PageUpdate, ProjectionState};

use std::sync::Arc;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Extension};
use tokio::sync::Semaphore;
use tower_http::limit::RequestBodyLimitLayer;

use crate::middleware::LayerLimits;
use crate::middleware::WsAuthLimits;
use crate::middleware::limits::http_concurrency_limit;

/// Construct an axum router for the projection adapter.
///
/// Mounts:
///
/// - `GET /v1/healthz` — liveness probe (static `{"v":1,"status":"ok"}`).
/// - `GET /v1/readyz` — readiness mapped from [`ProjectionSource::is_ready`].
/// - `GET /v1/{*path}` — snapshot read with ETag/304 + zstd negotiation.
/// - `GET /ws` — WebSocket upgrade subscribing to [`ProjectionSource::subscribe`].
///
/// The router is **state-typed**; `P: ProjectionSource` is threaded as a
/// generic parameter per CHE-0005 R1 + CHE-0049 R12 — no trait objects.
/// Consumer composition with [`crate::build_router`] (the cqrs surface)
/// is done via [`Router::merge`] in `main`.
///
/// ## Parameters
///
/// - `state` — typed projection state (CHE-0049:R1 + R12).
/// - `limits` — per-layer numeric sizing for the SEC-0003 R1/R3
///   availability layers attached by this builder (CHE-0062:R2). The
///   library owns *what layer is attached where*; the consumer owns
///   *what number goes in*. Three layers are unconditionally attached
///   per CHE-0062:R4:
///     - body cap → `RequestBodyLimitLayer` (413 on exceed, SEC-0003:R1);
///     - inflight cap → [`http_concurrency_limit`] middleware with
///       **503-shedding** semantics (SEC-0003:R3 — does *not* queue);
///     - WS connection cap → `Arc<Semaphore>` extracted as
///       `Extension<Arc<Semaphore>>` by [`handlers::ws_handler`], which
///       calls `try_acquire_owned` on upgrade and returns 503 on
///       exhaustion (SEC-0003:R3 route-scoped per CHE-0049:R3 + R11).
/// - `ws_auth` — typed [`WsAuthLimits`] carrying the WS authentication
///   policy attached at the upgrade boundary (SEC-0012:R1). The default
///   `WsAuthLimits::default()` elects
///   `WebSocketOriginPolicy::Strict` — absent or mismatched `Origin`
///   is rejected with `403 FORBIDDEN` before the handshake completes
///   (SEC-0012:R2). Consumers serving non-browser clients elect
///   `WsAuthLimits::permissive_for_tests()` or construct with
///   `origin_policy = WebSocketOriginPolicy::AllowAbsent`,
///   accepting CWE-346 / CWE-1385 risk per SEC-0012:R3.
/// - `extra_routes` — stateless [`Router`] merged onto the projection
///   surface after `.with_state(state)`. Auth probes, status pages,
///   anything outside the projection contract. The projection state
///   never leaks into consumer routes. Callers with no extras pass
///   [`Router::new()`]. Realises the CHE-0049 R12 amendment
///   (2026-05-16).
///
/// Per CHE-0049 R11 backpressure is "drop-and-resync": on
/// `broadcast::RecvError::Lagged` the per-socket task closes the WS
/// with code 1001 ("Going Away"). Clients recover by HTTP-fetching the
/// current snapshot and re-attaching a fresh WS for subsequent deltas
/// — the snapshot is the durable checkpoint per CHE-0048:R2.
pub fn build_projection_router<P>(
    state: ProjectionState<P>,
    limits: LayerLimits,
    ws_auth: WsAuthLimits,
    extra_routes: Router,
) -> Router
where
    P: ProjectionSource,
{
    let http_semaphore = Arc::new(Semaphore::new(limits.max_inflight_requests));
    let ws_semaphore = Arc::new(Semaphore::new(limits.max_ws_connections));

    handlers::build(state)
        .merge(extra_routes)
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(limits.max_body_bytes))
        .layer(axum::middleware::from_fn(move |request, next| {
            let sem = Arc::clone(&http_semaphore);
            http_concurrency_limit(sem, request, next)
        }))
        .layer(Extension(ws_semaphore))
        .layer(Extension(ws_auth))
}
