//! Projection adapter (m5-projection-port; CHE-0049 R11–R14).
//!
//! Gated behind `feature = "projection"`; realises the read-side adapter
//! the web layer mounts as a second axum surface alongside the default
//! (`cqrs`) command-gateway surface:
//!
//! - [`port::ProjectionSource`] — port trait consumers implement to
//!   expose snapshot + subscribe APIs. No `serde` decode-bound bleed
//!   onto domain types (CHE-0014 R2; closes A3).
//! - [`state::ProjectionState<P>`] — typed state per CHE-0005 R1 +
//!   CHE-0049 R12 (no `Box<dyn …>`, no trait objects).
//! - [`build_projection_router`] — axum router constructor; mounts
//!   `/v1/healthz`, `/v1/readyz`, `/v1/{*path}` HTTP routes per CHE-0049
//!   R9 and an unversioned `/ws` upgrade carrying `"v": 1` per R13.
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
/// Mounts `GET /v1/healthz` (liveness), `GET /v1/readyz`
/// ([`ProjectionSource::is_ready`]), `GET /v1/{*path}` (snapshot read,
/// ETag/304 + zstd), `GET /ws` (upgrade to
/// [`ProjectionSource::subscribe`]).
///
/// State-typed: `P: ProjectionSource` is generic (CHE-0005 R1 +
/// CHE-0049 R12, no trait objects). Compose with
/// [`crate::build_router`] via [`Router::merge`].
///
/// `limits` sizes three layers per CHE-0062:R4 / SEC-0003 R1/R3: body
/// cap (413), in-flight cap (503-shedding), WS cap (503 via
/// [`handlers::ws_handler`]).
///
/// `ws_auth` carries the WS auth policy (SEC-0012:R1): default
/// `Strict` rejects absent/mismatched `Origin` with 403 (SEC-0012:R2);
/// `AllowAbsent` accepts CWE-346/1385 risk (SEC-0012:R3).
///
/// `extra_routes` is a stateless [`Router`] merged after
/// `.with_state(state)`; pass [`Router::new()`] for none.
///
/// Backpressure (CHE-0049 R11) is drop-and-resync: on
/// `broadcast::RecvError::Lagged` the socket closes with code 1001;
/// clients re-fetch the snapshot (CHE-0048:R2) and re-attach WS.
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
