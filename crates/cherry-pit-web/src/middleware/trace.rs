//! Structured HTTP tracing layer (CHE-0049 R14 transport helper).
//!
//! A pre-configured [`TraceLayer`] that opens an `http` span per request
//! tagged with `method` and `path`, emits a `request started` debug
//! event on entry, and a `status`/`latency_us`-tagged info event on
//! response. The wiring matches the donor crate's `TraceLayer` block
//! verbatim so consumers migrating off the donor see no observable
//! change in their log shape.
//!
//! Observability only — does not participate in CHE-0049:R5 correlation
//! propagation. Compose with [`crate::correlation::correlation_layer`]
//! when consumer composition wants both.

use std::time::Duration;

use axum::http::{Request, Response};
use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
use tower_http::trace::TraceLayer;
use tracing::{Span, debug, info, info_span};

// ---------------------------------------------------------------------------
// Hook types — zero-sized so the surrounding `TraceLayer` stays `Clone`
// without dragging closure-capture lifetimes into the public return type.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
#[doc(hidden)]
pub struct MakeHttpSpan;

impl<B> tower_http::trace::MakeSpan<B> for MakeHttpSpan {
    fn make_span(&mut self, request: &Request<B>) -> Span {
        info_span!(
            "http",
            method = %request.method(),
            path = %request.uri().path(),
        )
    }
}

#[derive(Clone, Copy)]
#[doc(hidden)]
pub struct OnHttpRequest;

impl<B> tower_http::trace::OnRequest<B> for OnHttpRequest {
    fn on_request(&mut self, _request: &Request<B>, _span: &Span) {
        debug!("request started");
    }
}

#[derive(Clone, Copy)]
#[doc(hidden)]
pub struct OnHttpResponse;

impl<B> tower_http::trace::OnResponse<B> for OnHttpResponse {
    fn on_response(self, response: &Response<B>, latency: Duration, _span: &Span) {
        info!(
            status = response.status().as_u16(),
            latency_us = u64::try_from(latency.as_micros()).unwrap_or(u64::MAX),
            "response",
        );
    }
}

/// Concrete shape of the configured tracing layer. Spelled out so the
/// helper's return type is nameable; callers normally just call
/// [`http_trace_layer`] and attach via [`axum::Router::layer`].
pub type HttpTraceLayer = TraceLayer<
    SharedClassifier<ServerErrorsAsFailures>,
    MakeHttpSpan,
    OnHttpRequest,
    OnHttpResponse,
>;

/// Build a structured `TraceLayer` for HTTP routes.
///
/// Wire via [`axum::Router::layer`]:
///
/// ```no_run
/// use axum::Router;
/// use cherry_pit_web::http_trace_layer;
///
/// let _router: Router = Router::new().layer(http_trace_layer());
/// ```
///
/// The returned layer is `Clone` (the hooks are zero-sized) so the same
/// value composes into multiple routers without re-instantiation.
#[must_use]
pub fn http_trace_layer() -> HttpTraceLayer {
    TraceLayer::new_for_http()
        .make_span_with(MakeHttpSpan)
        .on_request(OnHttpRequest)
        .on_response(OnHttpResponse)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_trace_layer_is_clone() {
        // Hooks are zero-sized; cloning the layer is a refcount bump on
        // the shared classifier. Asserts the helper is usable in
        // multi-router composition without ceremony.
        let layer = http_trace_layer();
        let _cloned = layer.clone();
    }
}
