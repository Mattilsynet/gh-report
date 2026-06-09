//! SEC-0003 availability layers attached library-side per **CHE-0062**.
//!
//! Three layers discharge the SEC-0003 R1/R2/R3 obligation at every
//! cherry-pit-web ingestion point. The library owns *what layer is
//! attached where*; the consumer owns *what number goes in* via the
//! [`LayerLimits`] value type. No consumer config type crosses the
//! library boundary (CHE-0062:R3).
//!
//! - **`max_body_bytes`** — bounds inbound request body size. Realised
//!   by `tower_http::limit::RequestBodyLimitLayer` attached inside both
//!   [`super::super::build_router`] (cqrs surface) and
//!   [`super::super::projection::build_projection_router`] (read
//!   surface). SEC-0003:R1 ("every allocation sized by external input
//!   has a configurable maximum enforced before allocation").
//! - **`max_inflight_requests`** — bounds in-flight HTTP requests with
//!   **503-shedding semantics** (not queueing). Realised by
//!   [`http_concurrency_limit`], a semaphore-`try_acquire` middleware
//!   that returns `503 Service Unavailable` when the permit budget is
//!   exhausted. Attached on both routers. SEC-0003:R3 ("backpressure
//!   mechanisms exist at every ingestion point to shed load when
//!   capacity is exceeded"). Matches the donor crate `gh-report`'s
//!   `infra::server::http_concurrency_limit` shape byte-for-byte so the
//!   SEC-0003 falsifier tests at
//!   `crates/gh-report/src/infra/server/server.rs:2164,:2209` continue
//!   to observe the same accept/shed topology once Track 4.3 migrates
//!   them onto this router.
//! - **`max_ws_connections`** — bounds concurrent WebSocket upgrades.
//!   Realised inside the WS upgrade handler via
//!   `Arc<Semaphore>::try_acquire_owned` returning `503` on exhaustion;
//!   the owned permit is held for the connection lifetime. Attached on
//!   the projection router only — the cqrs surface is HTTP-only per
//!   CHE-0049:R3 and ignores this field. SEC-0003:R3 route-scoped per
//!   CHE-0049:R3 + R11. Matches the donor's permit discipline at
//!   `server.rs:3144,:3559`.
//!
//! ## No `Default`
//!
//! [`LayerLimits`] deliberately does **not** implement [`Default`]: a
//! defaulted permissive `LayerLimits` is a SEC-0003 footgun (production
//! callers would silently mount layers that bind nothing). Tests reach
//! for [`LayerLimits::permissive_for_tests`] with eyes open; production
//! callers name three values.
//!
//! ## Field semantics
//!
//! All three fields are `usize` carrying a hard upper bound on the
//! resource named by the field. `usize::MAX` is the conventional
//! "effectively unbounded" value; `0` rejects every request the layer
//! sees (`RequestBodyLimitLayer::new(0)` rejects any non-empty body;
//! the semaphores hand out zero permits → unconditional `503`).
//! Per CHE-0062:R4 each field is unconditionally honoured — there is
//! no `Option`-per-field. Disabling a layer is out of scope; the
//! obligation under SEC-0003 R1/R3 is unconditional at every ingestion
//! point.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tokio::sync::Semaphore;

/// Per-layer numeric limits attached by the cherry-pit-web router
/// builders (CHE-0062:R2).
///
/// Construct directly with named fields:
///
/// ```
/// use cherry_pit_web::LayerLimits;
///
/// let limits = LayerLimits {
///     max_body_bytes: 1024 * 1024,
///     max_inflight_requests: 100,
///     max_ws_connections: 64,
/// };
/// ```
///
/// Consumers build `LayerLimits` from any source — application config,
/// environment, hard-coded defaults. The library does not inspect that
/// source (CHE-0062:R3 — no consumer config type crosses the boundary).
///
/// The struct is `Copy`: three `usize`s, no heap. Pass by value at the
/// call site; the router builder copies the values into the per-instance
/// semaphores it constructs.
///
/// Adding a future field is a semver-major event for cherry-pit-web
/// (CHE-0062:R6); the crate is internal and `Cargo.lock` is committed
/// per the crate README, so the workspace tolerates this.
#[derive(Debug, Clone, Copy)]
pub struct LayerLimits {
    /// Maximum inbound request body in bytes. Bodies exceeding this
    /// value cause the router to short-circuit with
    /// `413 Payload Too Large` before reaching the handler. SEC-0003:R1.
    pub max_body_bytes: usize,

    /// Maximum in-flight HTTP requests. Requests arriving when this
    /// many are already in flight cause the concurrency middleware to
    /// short-circuit with `503 Service Unavailable` immediately
    /// (sheds load; does not queue). SEC-0003:R3.
    pub max_inflight_requests: usize,

    /// Maximum concurrent WebSocket connections accepted by the
    /// projection adapter's `/ws` upgrade. Upgrades arriving when this
    /// many sessions are already attached are rejected with `503`
    /// before the WS handshake completes. SEC-0003:R3 route-scoped per
    /// CHE-0049:R3 + R11.
    pub max_ws_connections: usize,
}

impl LayerLimits {
    /// Construct a `LayerLimits` whose every field is large enough to
    /// be effectively unbounded under the test harness. Intended **only**
    /// for tests that exercise routes without exercising the limits
    /// themselves.
    ///
    /// The name is deliberately pejorative: production code that calls
    /// this is wrong. Production code names three values informed by
    /// SEC-0003:R1/R3 sizing.
    ///
    /// Values are large but not `usize::MAX` so the layers themselves
    /// still execute (exercising the wiring) rather than being elided
    /// by a fast-path. `1 GiB` body, `1024` in-flight, `1024` WS — all
    /// well above any per-test load.
    #[must_use]
    pub fn permissive_for_tests() -> Self {
        Self {
            max_body_bytes: 1024 * 1024 * 1024,
            max_inflight_requests: 1024,
            max_ws_connections: 1024,
        }
    }
}

/// 503-shedding HTTP concurrency limiter middleware.
///
/// Bounds the number of in-flight HTTP requests at the router level.
/// Returns `503 Service Unavailable` immediately on exhaustion (sheds
/// load) rather than queueing — matches the donor crate
/// `gh-report::infra::server::http_concurrency_limit` semantics so
/// SEC-0003:R3 falsifier tests at
/// `crates/gh-report/src/infra/server/server.rs:2164,:2209` observe the
/// same accept/shed topology after Track 4.3 migration.
///
/// `tower::limit::ConcurrencyLimit` is deliberately **not** used: that
/// layer queues, which violates the "shed, don't queue" obligation in
/// CHE-0062:R1.
///
/// Wired via [`axum::middleware::from_fn`] in
/// [`super::super::build_router`] (cqrs surface) and
/// [`super::super::projection::build_projection_router`] (read surface,
/// gated on the `projection` feature). The per-instance
/// `Arc<Semaphore>` is captured by the closure so the permit pool
/// lives for the router's lifetime.
pub(crate) async fn http_concurrency_limit(
    semaphore: Arc<Semaphore>,
    request: Request,
    next: Next,
) -> Response {
    let Ok(_permit) = semaphore.try_acquire() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_limits_is_copy_clone_debug() {
        let a = LayerLimits {
            max_body_bytes: 1,
            max_inflight_requests: 2,
            max_ws_connections: 3,
        };
        let b = a;
        let c = a;
        assert_eq!(b.max_body_bytes, 1);
        assert_eq!(c.max_inflight_requests, 2);
        let _: String = format!("{a:?}");
    }

    #[test]
    fn permissive_for_tests_is_unbounded_in_practice() {
        let l = LayerLimits::permissive_for_tests();
        assert!(l.max_body_bytes >= 1024 * 1024);
        assert!(l.max_inflight_requests >= 64);
        assert!(l.max_ws_connections >= 64);
    }
}
