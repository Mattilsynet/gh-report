//! Round-trip integration tests for CHE-0049 R5 correlation echo.
//!
//! Drives `correlation_layer` through `tower::ServiceExt::oneshot` and
//! asserts the public observable behaviour:
//!
//! - request with `X-Correlation-ID: <uuid>` → response carries the
//!   same `X-Correlation-ID` header.
//! - request with valid `traceparent` → response carries
//!   `X-Correlation-ID` matching the trace-id.
//! - request without correlation headers → response **omits**
//!   `X-Correlation-ID` (no synthesis; CHE-0039 R2).
//! - handler can read the stashed [`CorrelationContext`] from request
//!   extensions.

use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, StatusCode},
    middleware::from_fn,
    routing::get,
};
use cherry_pit_core::CorrelationContext;
use cherry_pit_web::correlation::correlation_layer;
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

/// Handler that reflects the stashed `CorrelationContext` correlation id
/// (or "none") into the response body, so tests can also assert that
/// extension-stashing works end-to-end.
async fn reflect_handler(request: Request) -> String {
    request
        .extensions()
        .get::<CorrelationContext>()
        .and_then(CorrelationContext::correlation_id)
        .map_or_else(|| "none".to_string(), |id| id.to_string())
}

fn app() -> Router {
    Router::new()
        .route("/probe", get(reflect_handler))
        .layer(from_fn(correlation_layer))
}

async fn body_string(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn x_correlation_id_is_echoed_on_response() {
    let id = Uuid::now_v7();
    let req = Request::builder()
        .uri("/probe")
        .header("x-correlation-id", id.to_string())
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("x-correlation-id"),
        Some(&HeaderValue::from_str(&id.to_string()).unwrap()),
        "echo header must match the inbound correlation id"
    );

    let body = body_string(response).await;
    assert_eq!(body, id.to_string(), "handler must observe the stashed ctx");
}

#[tokio::test]
async fn traceparent_drives_echo_header() {
    let tp = "00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01";
    let expected_id = Uuid::from_u128(0x0af7_6519_16cd_43dd_8448_eb21_1c80_319c);

    let req = Request::builder()
        .uri("/probe")
        .header("traceparent", tp)
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("x-correlation-id"),
        Some(&HeaderValue::from_str(&expected_id.to_string()).unwrap()),
    );

    let body = body_string(response).await;
    assert_eq!(body, expected_id.to_string());
}

#[tokio::test]
async fn no_correlation_headers_no_echo() {
    let req = Request::builder()
        .uri("/probe")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers().get("x-correlation-id").is_none(),
        "absent correlation must not synthesise an echo header (CHE-0039 R2)"
    );

    let body = body_string(response).await;
    assert_eq!(
        body, "none",
        "handler must observe CorrelationContext::none()"
    );
}

#[tokio::test]
async fn malformed_traceparent_does_not_400() {
    // W3C: receivers must not reject malformed traceparent.
    let req = Request::builder()
        .uri("/probe")
        .header("traceparent", "garbage")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("x-correlation-id").is_none());
}
