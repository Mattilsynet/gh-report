//! # cherry-pit-web
//!
//! HTTP adapter family over axum. Realises **CHE-0049**: translates
//! HTTP requests into domain commands, dispatches via the gateway, and
//! maps outcomes to responses with correlation propagation (CHE-0039).
//! Realises **CHE-0050** (`CommandRouter` port): a consumer-owned
//! wire-deserialize-and-dispatch trait threaded through `AppState` and
//! [`build_router`] as type parameter `R`.
//!
//! The cqrs router is HTTP-only, no built-in auth (CHE-0049 R2/R3);
//! consumers attach auth via `extra_routes`. [`serve`] provides the
//! CHE-0086 read-serve surface. Under `feature = "projection"`,
//! [`build_projection_router`] mounts a read-side surface with a
//! narrowed WS upgrade for snapshot-delta push only (CHE-0049 R11).
//!
//! ## Public surface (CHE-0049:R14, CHE-0030:R2)
//!
//! `middleware` is private; its primitives reach consumers via a flat
//! `pub use` here. Remaining surface: [`errors`], [`correlation`],
//! [`path`], [`AppState<G, S, R>`] (CHE-0049:R1, CHE-0050:R2),
//! [`build_router`] (CHE-0049:R9),
//! [`CommandRouter`]/[`DispatchOutcome`] (CHE-0050:R1); under
//! `feature = "projection"`: [`ProjectionSource`], [`ProjectionState`],
//! [`PageEntry`], [`PageUpdate`], [`build_projection_router`],
//! [`ServerConfig`], [`ServerConfigBuilder`], [`ServerError`],
//! [`ValidatedConfig`], [`ConfigError`].

#![forbid(unsafe_code)]

mod command_router;
pub(crate) mod middleware;
#[cfg(feature = "projection")]
mod projection;
mod router;
pub mod serve;
mod state;

pub mod correlation;
pub mod errors;
pub mod path;

pub use command_router::{CommandRouter, DispatchOutcome};
pub use middleware::{
    HttpTraceLayer, LayerLimits, NormalizedPath, SVG_CSP, WebSocketOriginPolicy, WsAuthLimits,
    compress_zstd, compute_etag, http_trace_layer, normalize_request_path, sanitize_path_segment,
    security_headers,
};
#[cfg(feature = "projection")]
pub use projection::{
    ConfigError, PageEntry, PageUpdate, ProjectionSource, ProjectionState, ServerConfig,
    ServerConfigBuilder, ServerError, ValidatedConfig, build_projection_router,
};
pub use router::build_router;
pub use serve::{CachedPage, PageUpdateEvent, ServerState};
pub use state::AppState;
