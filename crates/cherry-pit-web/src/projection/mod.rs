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

use axum::Router;

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
/// `extra_routes` is a stateless [`Router`] that the consumer merges
/// onto the projection surface — auth probes, status pages, anything
/// outside the projection contract. The merge happens after
/// `.with_state(state)` so `extra_routes` carries its own state (or
/// none); the projection state never leaks into consumer routes.
/// Callers with no extras pass [`Router::new()`]. The parameter
/// realises the CHE-0049 R12 amendment (2026-05-16) extending the
/// CQRS-side R2 `extra_routes` merge-point convention to the
/// projection adapter.
///
/// Per CHE-0049 R11 backpressure is "drop-and-resync": on
/// `broadcast::RecvError::Lagged` the per-socket task closes the WS
/// with code 1001 ("Going Away"). Clients recover by HTTP-fetching the
/// current snapshot and re-attaching a fresh WS for subsequent deltas
/// — the snapshot is the durable checkpoint per CHE-0048:R2.
pub fn build_projection_router<P>(state: ProjectionState<P>, extra_routes: Router) -> Router
where
    P: ProjectionSource,
{
    handlers::build(state).merge(extra_routes)
}
