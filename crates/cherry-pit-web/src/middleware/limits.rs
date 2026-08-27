//! SEC-0003 availability layers attached library-side per **CHE-0062**.
//!
//! Two layers discharge SEC-0003 R1/R3 at every ingestion point; the
//! library owns *what* is attached *where*, the consumer owns *what
//! number* via [`LayerLimits`]. No consumer config type crosses the
//! boundary (CHE-0062:R3).
//!
//! - **`max_body_bytes`** — inbound body size via
//!   `RequestBodyLimitLayer` on every surface (SEC-0003:R1).
//! - **`max_inflight_requests`** — in-flight requests, 503-shed (not
//!   queued) via [`http_concurrency_limit`] (SEC-0003:R3).
//!
//! The WS connection cap is deliberately **not** here. It lives on
//! SEC-0012's `WsPolicy` beside the Origin policy, because a surface
//! that mounts a `/ws` upgrade needs both to be safe and carrying them
//! separately let `serve` take the cap and omit the policy — see the
//! reversal recorded in SEC-0012's Consequences (CHE-0062:R1/R2).
//!
//! [`LayerLimits`] omits [`Default`]: a defaulted permissive value is
//! a SEC-0003 footgun. Tests use
//! [`LayerLimits::permissive_for_tests`]; production names both
//! values, [`NonZeroUsize`] hard upper bounds
//! (`NonZeroUsize::MAX` unbounded in practice), each unconditionally
//! honoured per CHE-0062:R4.

use std::num::NonZeroUsize;
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
/// use std::num::NonZeroUsize;
///
/// use cherry_pit_web::LayerLimits;
///
/// let limits = LayerLimits {
///     max_body_bytes: NonZeroUsize::new(1024 * 1024).expect("nonzero"),
///     max_inflight_requests: NonZeroUsize::new(100).expect("nonzero"),
/// };
/// ```
///
/// Consumers build `LayerLimits` from any source — application config,
/// environment, hard-coded defaults. The library does not inspect that
/// source (CHE-0062:R3 — no consumer config type crosses the boundary).
///
/// Both fields are [`NonZeroUsize`]: a cap of zero admits no request at
/// all, which is a disabled layer in fact and is forbidden in intent by
/// CHE-0062:R4. It is unrepresentable here rather than rejected by a
/// runtime guard (SEC-0014:R4).
///
/// The struct is `Copy`: two `NonZeroUsize`s, no heap. Pass by value at
/// the call site; the router builder copies the values into the
/// per-instance semaphores it constructs.
///
/// Adding a future field is a semver-major event for cherry-pit-web
/// (CHE-0062:R6); the crate is internal and `Cargo.lock` is committed
/// per the crate README, so the workspace tolerates this.
#[derive(Debug, Clone, Copy)]
pub struct LayerLimits {
    /// Maximum inbound request body in bytes, applied as a ceiling at
    /// every ingestion point. Bodies exceeding this value cause the
    /// router to short-circuit with `413 Payload Too Large` before
    /// reaching the handler. Route groups may narrow this with their
    /// own nested layer; none may widen it. SEC-0003:R1.
    pub max_body_bytes: NonZeroUsize,

    /// Maximum in-flight HTTP requests. Requests arriving when this
    /// many are already in flight cause the concurrency middleware to
    /// short-circuit with `503 Service Unavailable` immediately
    /// (sheds load; does not queue). SEC-0003:R3.
    pub max_inflight_requests: NonZeroUsize,
}

impl LayerLimits {
    /// Construct a `LayerLimits` whose every field is large enough to
    /// be effectively unbounded under the test harness. Intended **only**
    /// for tests that exercise routes without exercising the limits
    /// themselves.
    ///
    /// The name is deliberately pejorative: production code that calls
    /// this is wrong. Production code names both values informed by
    /// SEC-0003:R1/R3 sizing.
    ///
    /// Values are large but not `NonZeroUsize::MAX` so the layers
    /// themselves still execute (exercising the wiring) rather than
    /// being elided by a fast-path. `1 GiB` body, `1024` in-flight —
    /// both well above any per-test load.
    #[must_use]
    pub fn permissive_for_tests() -> Self {
        const BODY: NonZeroUsize = NonZeroUsize::new(1024 * 1024 * 1024).expect("nonzero");
        const INFLIGHT: NonZeroUsize = NonZeroUsize::new(1024).expect("nonzero");
        Self {
            max_body_bytes: BODY,
            max_inflight_requests: INFLIGHT,
        }
    }
}

/// 503-shedding HTTP concurrency limiter middleware.
///
/// Bounds the number of in-flight HTTP requests at the router level.
/// Returns `503 Service Unavailable` immediately on exhaustion (sheds
/// load) rather than queueing — preserves the "shed, don't queue"
/// accept/shed topology of the original `gh-report` donor
/// implementation this layer supersedes, per SEC-0003:R3.
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
            max_body_bytes: NonZeroUsize::new(1).unwrap(),
            max_inflight_requests: NonZeroUsize::new(2).unwrap(),
        };
        let b = a;
        let c = a;
        assert_eq!(b.max_body_bytes.get(), 1);
        assert_eq!(c.max_inflight_requests.get(), 2);
        let _: String = format!("{a:?}");
    }

    #[test]
    fn permissive_for_tests_is_unbounded_in_practice() {
        let l = LayerLimits::permissive_for_tests();
        assert!(l.max_body_bytes.get() >= 1024 * 1024);
        assert!(l.max_inflight_requests.get() >= 64);
    }
}
