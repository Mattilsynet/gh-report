//! Structured HTTP tracing layer (CHE-0049 R14 transport helper).
//!
//! A pre-configured [`TraceLayer`] that opens an `http` span per request
//! tagged with `method` and `path`, emits a `request started` debug
//! event on entry, and a `status`-tagged info event on response. Normal
//! responses also carry `latency_us`; a 101 Switching Protocols response
//! (WebSocket upgrade) omits it, because `tower_http` measures the
//! "latency" of a protocol switch as the full connection lifetime, not
//! request latency, and that duration otherwise pollutes latency metrics
//! downstream.
//!
//! Observability only — does not participate in CHE-0049:R5 correlation
//! propagation. Compose with [`crate::correlation::correlation_layer`]
//! when consumer composition wants both.

use std::time::Duration;

use axum::http::{Request, Response, StatusCode};
use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
use tower_http::trace::TraceLayer;
use tracing::{Span, debug, info, info_span};

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
        let status = response.status();
        if status == StatusCode::SWITCHING_PROTOCOLS {
            info!(status = status.as_u16(), "response");
        } else {
            info!(
                status = status.as_u16(),
                latency_us = u64::try_from(latency.as_micros()).unwrap_or(u64::MAX),
                "response",
            );
        }
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
    use tower_http::trace::OnResponse;

    #[test]
    fn http_trace_layer_is_clone() {
        let layer = http_trace_layer();
        let _cloned = layer.clone();
    }

    #[test]
    fn on_response_switching_protocols_omits_latency_us() {
        let response = Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .body(())
            .expect("building a 101 response");

        let events = capture_response_events(|| {
            OnHttpResponse.on_response(&response, Duration::from_secs(301), &Span::none());
        });

        assert!(events.contains("status=101"), "got: {events}");
        assert!(!events.contains("latency_us"), "got: {events}");
    }

    #[test]
    fn on_response_ok_still_includes_latency_us() {
        let response = Response::builder()
            .status(StatusCode::OK)
            .body(())
            .expect("building a 200 response");

        let events = capture_response_events(|| {
            OnHttpResponse.on_response(&response, Duration::from_millis(42), &Span::none());
        });

        assert!(events.contains("status=200"), "got: {events}");
        assert!(events.contains("latency_us=42000"), "got: {events}");
    }

    fn capture_response_events(f: impl FnOnce()) -> String {
        use std::fmt::Write;
        use std::sync::{Arc, Mutex};
        use tracing::field::{Field, Visit};
        use tracing::span;
        use tracing::subscriber::Interest;
        use tracing::{Event, Level, Metadata, Subscriber};

        #[derive(Clone, Default)]
        struct CaptureSubscriber {
            events: Arc<Mutex<String>>,
        }

        impl Subscriber for CaptureSubscriber {
            fn enabled(&self, metadata: &Metadata<'_>) -> bool {
                metadata.level() <= &Level::INFO
            }

            fn new_span(&self, _span: &span::Attributes<'_>) -> span::Id {
                span::Id::from_u64(1)
            }

            fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}

            fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}

            fn event(&self, event: &Event<'_>) {
                let mut visitor = EventVisitor::default();
                event.record(&mut visitor);
                let mut events = self.events.lock().unwrap();
                writeln!(
                    events,
                    "level={} {}",
                    event.metadata().level(),
                    visitor.fields
                )
                .unwrap();
            }

            fn enter(&self, _span: &span::Id) {}

            fn exit(&self, _span: &span::Id) {}

            fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
                if self.enabled(metadata) {
                    Interest::always()
                } else {
                    Interest::never()
                }
            }

            fn max_level_hint(&self) -> Option<tracing::metadata::LevelFilter> {
                Some(tracing::metadata::LevelFilter::INFO)
            }
        }

        #[derive(Default)]
        struct EventVisitor {
            fields: String,
        }

        impl Visit for EventVisitor {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                write!(self.fields, "{}={value:?} ", field.name()).unwrap();
            }
        }

        let subscriber = CaptureSubscriber::default();
        let events = Arc::clone(&subscriber.events);
        tracing::subscriber::with_default(subscriber, f);
        events.lock().unwrap().clone()
    }
}
