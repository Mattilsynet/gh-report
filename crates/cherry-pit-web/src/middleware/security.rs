//! Security response header middleware (CHE-0049 R8 transport helper).
//!
//! Single axum middleware function that injects a fixed set of defensive
//! response headers: `X-Frame-Options`, `X-Content-Type-Options`,
//! `Content-Security-Policy` (caller-provided), `Referrer-Policy`,
//! `Permissions-Policy`, and `Strict-Transport-Security`.
//!
//! Replaces six individual `SetResponseHeaderLayer::overriding(...)` layers
//! with one async function, collapsing 6 levels of `Service::call()`
//! indirection into a single function call.
//!
//! Ported byte-for-byte from the donor crate per CHE-0049 R14; donor copies
//! remain in that crate until the gh-report migration completes.

use axum::{
    extract::Request,
    http::{HeaderValue, header},
    middleware::Next,
    response::Response,
};

/// Inject all security response headers in a single middleware pass.
///
/// Wire via [`axum::middleware::from_fn`] with a closure that clones the
/// CSP `HeaderValue` per request, e.g.:
///
/// ```no_run
/// // Constructor-only wiring sketch: builds a `Router` and registers a
/// // `from_fn` middleware closure. `security_headers` is referenced but
/// // not awaited; no I/O, no listener bind. `no_run` (DoD-3 Type-A per
/// // WU-3 lib.rs:412 precedent) suffices to compile-check the surface.
/// use axum::{Router, middleware, http::HeaderValue};
/// use cherry_pit_web::security_headers;
///
/// let csp = HeaderValue::from_static("default-src 'self'");
/// let _router: Router = Router::new().layer(middleware::from_fn(move |req, next| {
///     let csp = csp.clone();
///     security_headers(req, next, csp)
/// }));
/// ```
///
/// The `csp` parameter is resolved from the consumer's validated config
/// (or a built-in default) and captured by the closure at wiring time.
///
/// If a downstream handler has already set a response-specific
/// `Content-Security-Policy` (e.g., SVG XSS mitigation sets a restrictive
/// CSP), this middleware preserves that value rather than overwriting it.
pub async fn security_headers(request: Request, next: Next, csp: HeaderValue) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    // Only set CSP if the handler didn't already set a response-specific
    // override (e.g., SVG XSS mitigation sets a restrictive CSP).
    if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
        headers.insert(header::CONTENT_SECURITY_POLICY, csp);
    }
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        axum::http::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    headers.insert(
        header::STRICT_TRANSPORT_SECURITY,
        HeaderValue::from_static("max-age=63072000; includeSubDomains"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        http::{Request as HttpRequest, StatusCode, header},
        middleware,
        routing::get,
    };
    use tower::ServiceExt;

    fn router_with_security_headers(csp: &'static str) -> Router {
        let csp_value = HeaderValue::from_static(csp);
        Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(middleware::from_fn(move |req, next| {
                let csp = csp_value.clone();
                security_headers(req, next, csp)
            }))
    }

    #[tokio::test]
    async fn injects_all_six_headers() {
        let app = router_with_security_headers("default-src 'self'");
        let resp = app
            .oneshot(HttpRequest::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let h = resp.headers();
        assert_eq!(h.get(header::X_FRAME_OPTIONS).unwrap(), "DENY");
        assert_eq!(h.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");
        assert_eq!(
            h.get(header::CONTENT_SECURITY_POLICY).unwrap(),
            "default-src 'self'"
        );
        assert_eq!(h.get(header::REFERRER_POLICY).unwrap(), "no-referrer");
        assert_eq!(
            h.get("permissions-policy").unwrap(),
            "camera=(), microphone=(), geolocation=()"
        );
        assert_eq!(
            h.get(header::STRICT_TRANSPORT_SECURITY).unwrap(),
            "max-age=63072000; includeSubDomains"
        );
    }

    #[tokio::test]
    async fn preserves_handler_set_csp() {
        // Handler sets a restrictive CSP; middleware must not overwrite it.
        let app = Router::new()
            .route(
                "/",
                get(|| async {
                    let mut resp = axum::response::Response::new(Body::from("ok"));
                    resp.headers_mut().insert(
                        header::CONTENT_SECURITY_POLICY,
                        HeaderValue::from_static("sandbox; default-src 'none'"),
                    );
                    resp
                }),
            )
            .layer(middleware::from_fn(|req, next| {
                let csp = HeaderValue::from_static("default-src 'self'");
                security_headers(req, next, csp)
            }));
        let resp = app
            .oneshot(HttpRequest::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.headers().get(header::CONTENT_SECURITY_POLICY).unwrap(),
            "sandbox; default-src 'none'"
        );
    }
}
