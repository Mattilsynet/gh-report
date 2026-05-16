//! Correlation propagation middleware.
//!
//! Realises **CHE-0049 R5** as an axum `from_fn` layer: extracts
//! [`CorrelationContext`] from inbound request headers (via
//! [`correlation::extract_correlation`]), stashes it in the
//! request extensions so handlers and error responders can read it,
//! then on the way out injects an `X-Correlation-ID` response header
//! when (and only when) a correlation id is present.
//!
//! ## Why `from_fn` and not a typed extractor
//!
//! A free function over `&HeaderMap` is the simplest testable surface
//! and intentionally does **not** widen [`crate::AppState`]'s `(G, S)`
//! bounds — correlation extraction is type-system-orthogonal to the
//! gateway and store. Wiring it as middleware (rather than as a
//! per-handler extractor) means future routes opt in for free without
//! re-threading the value through every signature.
//!
//! ## Echo policy
//!
//! - `correlation_id` present  → response carries `X-Correlation-ID:
//!   <uuid>`. The canonical fallback header doubles as the canonical
//!   echo header (see CHE-0049 R5 contract).
//! - `correlation_id` absent  → response omits the header entirely.
//!   Synthesising a value would violate **CHE-0039 R2** (forgetting
//!   correlation is a conscious omission).
//!
//! `traceparent` is **not** echoed: W3C trace context is request-side
//! only; the response-side surface is `tracestate`, which is out of
//! scope for v0.1.

pub(crate) mod compression;
pub(crate) mod correlation;
pub(crate) mod error;
pub(crate) mod path;
pub(crate) mod security;
pub(crate) mod trace;

use axum::{
    extract::Request,
    http::{HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};
use cherry_pit_core::CorrelationContext;

pub use compression::{compress_zstd, compute_etag};
pub use correlation::{IdempotencyKey, extract_correlation, extract_idempotency_key};
pub use error::{
    ErrorBody, ErrorEnvelope, map_bus_error, map_dispatch_error, map_store_error,
    post_persist_cancellation_response,
};
pub use path::{PathSegmentError, normalize_request_path, sanitize_path_segment};
pub use security::{SVG_CSP, security_headers};
pub use trace::{HttpTraceLayer, http_trace_layer};

/// Response header used to echo the active correlation id, per
/// CHE-0049 R5.
pub(crate) const ECHO_HEADER: HeaderName = HeaderName::from_static("x-correlation-id");

/// Middleware that extracts correlation, stashes it in extensions, and
/// echoes it on the response.
///
/// Wire via [`axum::middleware::from_fn`]:
///
/// ```
/// use axum::{Router, middleware};
/// use cherry_pit_web::correlation::correlation_layer;
///
/// // The layer is `Clone + Send + 'static` and applies to any router.
/// let _router: Router = Router::new().layer(middleware::from_fn(correlation_layer));
/// ```
pub async fn correlation_layer(mut request: Request, next: Next) -> Response {
    let ctx = extract_correlation(request.headers());
    // Stash for downstream handlers / error responders. Cloning is
    // cheap (two `Option<Uuid>`s) and we want both this layer and any
    // handler to be able to read the context independently.
    request.extensions_mut().insert(ctx.clone());

    let mut response = next.run(request).await;
    if let Some(corr) = ctx.correlation_id() {
        // `Uuid::to_string()` produces 36 ASCII chars — always a valid
        // header value. Conversion is infallible in practice; on the
        // theoretical failure we silently skip the echo rather than
        // mangle the response.
        if let Ok(value) = HeaderValue::from_str(&corr.to_string()) {
            response.headers_mut().insert(ECHO_HEADER, value);
        }
    }
    response
}

/// Read the [`CorrelationContext`] previously stashed by
/// [`correlation_layer`] from request extensions, or
/// [`CorrelationContext::none()`] if the layer is not active (e.g.
/// during isolated handler tests).
///
/// # Example
///
/// ```
/// use axum::{body::Body, extract::Request};
/// use cherry_pit_core::CorrelationContext;
/// use cherry_pit_web::correlation::correlation_from_extensions;
///
/// // Without the middleware in front, the helper returns
/// // `CorrelationContext::none()` (CHE-0039 R2: never synthesise).
/// let req = Request::new(Body::empty());
/// assert_eq!(correlation_from_extensions(&req), CorrelationContext::none());
/// ```
#[must_use]
pub fn correlation_from_extensions(request: &Request) -> CorrelationContext {
    request
        .extensions()
        .get::<CorrelationContext>()
        .cloned()
        .unwrap_or_else(CorrelationContext::none)
}
