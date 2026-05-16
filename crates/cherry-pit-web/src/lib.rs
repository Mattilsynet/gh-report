//! # cherry-pit-web
//!
//! HTTP adapter exposing [`cherry_pit_core::CommandGateway`] over axum.
//!
//! Realises **CHE-0049** (cherry-pit-web design) by translating HTTP
//! requests into domain commands, dispatching them via the gateway, and
//! mapping outcomes to HTTP responses with correlation propagation per
//! CHE-0039. Realises **CHE-0050** (`CommandRouter` port) by exposing a
//! consumer-owned wire-deserialize-and-dispatch trait threaded through
//! `AppState` and `build_router` as a third type parameter `R`.
//!
//! The crate is HTTP-only in v0.1 (CHE-0049 R3) — no WebSocket surface
//! on the cqrs router, no built-in auth (R2), no static-content cache
//! (R8). Consumers attach auth via the [`build_router`] `extra_routes`
//! merge point. Under `feature = "projection"`, a second router
//! [`build_projection_router`] mounts a read-side surface with a
//! narrowed WS upgrade for snapshot-delta push only (CHE-0049 R11).
//!
//! ## Public surface (CHE-0049:R14 + CHE-0030:R1/R2)
//!
//! Per CHE-0049:R14 the flat `pub use middleware::{...}` re-export at
//! `lib.rs` covers the five R8 utility primitives —
//! `compute_etag`, `compress_zstd`, `security_headers`,
//! `normalize_request_path`, `sanitize_path_segment` — plus three
//! generic transport helpers ported from the donor crate as part of
//! Track 4.2.A (`SVG_CSP`, `http_trace_layer`, `HttpTraceLayer`). The
//! `middleware` module itself is private (CHE-0030:R2). The deliberate
//! public items beyond these reach consumers through three dedicated
//! public submodules — [`errors`], [`correlation`], and [`path`] —
//! whose surfaces are documented at the module level.
//!
//! Top-level types not in `middleware`:
//!
//! - [`AppState<G, S, R>`] — generic typed state per CHE-0049:R1 +
//!   CHE-0050:R2.
//! - [`build_router`] — axum router mounted at `/v1/` per CHE-0049:R9.
//! - [`CommandRouter`], [`DispatchOutcome`] — the consumer-owned port
//!   per CHE-0050:R1.
//!
//! Under `feature = "projection"`: [`ProjectionSource`],
//! [`ProjectionState`], [`PageEntry`], [`PageUpdate`],
//! [`build_projection_router`], [`ServerConfig`], [`ServerConfigBuilder`],
//! [`ServerError`], [`ValidatedConfig`], [`ConfigError`].
//!
//! [`cherry_pit_core::CommandGateway`]: https://docs.rs/cherry-pit-core

#![forbid(unsafe_code)]

mod command_router;
pub(crate) mod middleware;
#[cfg(feature = "projection")]
mod projection;
mod router;
mod state;

pub mod correlation;
pub mod errors;
pub mod path;

pub use command_router::{CommandRouter, DispatchOutcome};
// CHE-0049:R14 — only the five R8 utilities are flat-re-exported from
// `lib.rs`. Everything else from `middleware` reaches consumers via
// the dedicated public submodules above (`errors`, `correlation`,
// `path`), keeping the `middleware` module itself implementation
// detail per CHE-0030:R2.
pub use middleware::{
    HttpTraceLayer, LayerLimits, NormalizedPath, SVG_CSP, compress_zstd, compute_etag,
    http_trace_layer, normalize_request_path, sanitize_path_segment, security_headers,
};
#[cfg(feature = "projection")]
pub use projection::{
    ConfigError, PageEntry, PageUpdate, ProjectionSource, ProjectionState, ServerConfig,
    ServerConfigBuilder, ServerError, ValidatedConfig, build_projection_router,
};
pub use router::build_router;
pub use state::AppState;
