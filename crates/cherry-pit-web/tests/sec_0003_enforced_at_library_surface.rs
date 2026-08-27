//! SEC-0003 — Availability — Bound All Resource Consumption.
//!
//! Library-surface enforcement smoke for `build_projection_router`.
//! Mechanism ADR: CHE-0062 (library-attached availability layers).
//!
//! - **R1** (bounded allocation): body cap → 413 via
//!   `tower_http::limit::RequestBodyLimitLayer`.
//! - **R3** (backpressure): inflight cap → 503 with shedding-not-queueing
//!   semantics (`http_concurrency_limit` middleware); WS upgrade cap →
//!   503 via `Arc<Semaphore>::try_acquire_owned` in `ws_handler`.
//! - **R2** (bounded iteration/recursion): NOT exercised here — R2 is a
//!   structural property of handler code with no library-attached layer
//!   surface. See SEC-0003 for the rule; enforcement sites live in
//!   `gh-report` and `cherry-pit-web` handler implementations.
//!
//! The load-bearing R1/R3 validation lives at gh-report's SEC-0003 test
//! sites (`crates/gh-report/src/infra/server/server.rs:1236,:2164,:2209,
//! :3144,:3559`); this file exercises the cherry-pit-web-side library
//! surface that those sites delegate to once Track 4.3 migrates them.

#![cfg(feature = "projection")]

mod common;

use std::num::NonZeroUsize;
use std::sync::Arc;

use tokio::sync::Notify;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use cherry_pit_web::{LayerLimits, ProjectionState, WsPolicy, build_projection_router};
use common::MockProjectionSource;
use tower::ServiceExt;

/// SEC-0003:R1 (bounded allocation) — body bytes exceeding
/// `LayerLimits.max_body_bytes` get `413 Payload Too Large` before
/// reaching the handler. Mechanism: CHE-0062.
///
/// `RequestBodyLimitLayer` short-circuits with 413 on
/// `Content-Length > max` *before* dispatching to a route, so the
/// method/route mismatch (POST → 405) cannot front-run the layer. We
/// target `/v1/healthz` because it exists and is otherwise GET-only;
/// the body-cap layer firing first is exactly the invariant we need.
#[tokio::test]
async fn body_over_max_returns_413() {
    let source = MockProjectionSource::new();
    let state = ProjectionState::from_arc(source);
    let limits = LayerLimits {
        max_body_bytes: NonZeroUsize::new(1024).unwrap(),
        max_inflight_requests: NonZeroUsize::new(1024).unwrap(),
    };
    let app = build_projection_router(
        state,
        limits,
        WsPolicy::permissive_for_tests(),
        axum::Router::new(),
    );

    let big_body = vec![0u8; 2048];
    let req = Request::builder()
        .method("POST")
        .uri("/v1/healthz")
        .header("content-length", "2048")
        .body(Body::from(big_body))
        .expect("request build");

    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(
        resp.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "body of 2048 > max_body_bytes=1024 must be rejected with 413"
    );
}

/// Body within `max_body_bytes` is processed normally (the response will
/// be a non-413; here `/v1/healthz` rejects POST with 405 but the body
/// limit does not fire).
#[tokio::test]
async fn body_under_max_passes_layer() {
    let source = MockProjectionSource::new();
    let state = ProjectionState::from_arc(source);
    let limits = LayerLimits {
        max_body_bytes: NonZeroUsize::new(4096).unwrap(),
        max_inflight_requests: NonZeroUsize::new(1024).unwrap(),
    };
    let app = build_projection_router(
        state,
        limits,
        WsPolicy::permissive_for_tests(),
        axum::Router::new(),
    );

    let small_body = vec![0u8; 16];
    let req = Request::builder()
        .method("POST")
        .uri("/v1/healthz")
        .body(Body::from(small_body))
        .expect("request build");

    let resp = app.oneshot(req).await.expect("oneshot");
    assert_ne!(
        resp.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "16-byte body must not trip max_body_bytes=4096"
    );
}

/// SEC-0003:R3 (backpressure) — exhausting the inflight semaphore
/// sheds the **data plane** with 503, while the liveness and readiness
/// probes still answer 200. Mechanism: CHE-0062
/// (`http_concurrency_limit`).
///
/// The cap is 1 and a request parked inside `/hold` (merged via
/// `extra_routes`, so it sits inside the concurrency layer per
/// CHE-0062:R7) holds the only permit while the assertions run. The
/// shed assertion targets `/data`, a fast `extra_routes` handler that
/// answers 200 when it is not shed.
#[tokio::test]
async fn inflight_saturation_sheds_data_plane_503_but_probes_answer_200_with_csp() {
    let source = MockProjectionSource::new();
    let state = ProjectionState::from_arc(source);
    let limits = LayerLimits {
        max_body_bytes: NonZeroUsize::new(1024 * 1024).unwrap(),
        max_inflight_requests: NonZeroUsize::new(1).unwrap(),
    };

    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let extra = axum::Router::new()
        .route("/data", axum::routing::get(|| async { "data" }))
        .route("/hold", {
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            axum::routing::get(move || {
                let entered = Arc::clone(&entered);
                let release = Arc::clone(&release);
                async move {
                    entered.notify_one();
                    release.notified().await;
                    "held"
                }
            })
        });
    let app = build_projection_router(state, limits, WsPolicy::permissive_for_tests(), extra);

    let hold = tokio::spawn({
        let app = app.clone();
        async move {
            let req = Request::builder()
                .uri("/hold")
                .body(Body::empty())
                .expect("request build");
            app.oneshot(req).await.expect("oneshot").status()
        }
    });
    entered.notified().await;

    let req = Request::builder()
        .uri("/data")
        .body(Body::empty())
        .expect("request build");
    let resp = app.clone().oneshot(req).await.expect("oneshot");
    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "a saturated inflight cap must shed every data-plane request with 503"
    );

    for probe in ["/v1/healthz", "/v1/readyz"] {
        let req = Request::builder()
            .uri(probe)
            .body(Body::empty())
            .expect("request build");
        let resp = app.clone().oneshot(req).await.expect("oneshot");
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "{probe} must answer while the inflight limiter is saturated"
        );
        assert!(
            resp.headers()
                .get(axum::http::header::CONTENT_SECURITY_POLICY)
                .is_some(),
            "{probe} outside the limiter lost content-security-policy"
        );
    }

    release.notify_one();
    assert_eq!(
        hold.await.expect("hold task"),
        StatusCode::OK,
        "the held request must complete once released"
    );
}

/// SEC-0003:R3 (backpressure) — a WS upgrade arriving when the
/// connection cap is already met is rejected with 503. Mechanism:
/// CHE-0062 (`Arc<Semaphore>::try_acquire_owned` on the upgrade path).
///
/// The cap is 1 and a live session holds the only permit, so the
/// second `try_acquire_owned` fails and the upgrade is refused before
/// the handshake. We exercise this via a real
/// `tokio_tungstenite::connect_async` against a bound server; the
/// connect-attempt observes the 503 status on the upgrade response.
///
/// `oneshot`-with-fabricated-upgrade-headers is **not** a viable test
/// path: axum runs `WebSocketUpgrade::from_request_parts` as part of
/// the extractor chain *before* the handler body, so any imperfection
/// in the synthetic headers front-runs the semaphore check with a
/// `426 Upgrade Required`. A real client handshake side-steps that.
#[tokio::test]
async fn ws_upgrade_beyond_connection_cap_returns_503() {
    use tokio::net::TcpListener;

    let source = MockProjectionSource::new();
    let state = ProjectionState::from_arc(source);
    let limits = LayerLimits {
        max_body_bytes: NonZeroUsize::new(1024 * 1024).unwrap(),
        max_inflight_requests: NonZeroUsize::new(1024).unwrap(),
    };
    let mut ws_policy = WsPolicy::permissive_for_tests();
    ws_policy.max_connections = NonZeroUsize::new(1).unwrap();
    let app = build_projection_router(state, limits, ws_policy, axum::Router::new());

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let url = format!("ws://{addr}/ws");

    let (_held, held_resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("1st WS connect takes the only permit");
    assert_eq!(
        held_resp.status(),
        101,
        "the 1st upgrade must succeed and hold the permit"
    );

    let result = tokio_tungstenite::connect_async(&url).await;
    match result {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(
                resp.status(),
                503,
                "an upgrade beyond max_connections must be rejected with 503"
            );
        }
        Err(other) => panic!("expected HTTP 503 from upgrade, got: {other}"),
        Ok(_) => panic!("WS upgrade should have been rejected with 503"),
    }

    server.abort();
}

/// SEC-0003:R3 (backpressure) — WS upgrade succeeds when a permit is
/// available, then the permit is released on disconnect (a fresh
/// upgrade succeeds after the prior session ends). Mechanism: CHE-0062.
/// Mirrors the donor's
/// `server.rs:2209::ws_semaphore_permit_released_on_disconnect` smoke.
#[tokio::test]
async fn ws_permit_released_on_disconnect() {
    use tokio::net::TcpListener;

    let source = MockProjectionSource::new();
    let state = ProjectionState::from_arc(source);
    let limits = LayerLimits {
        max_body_bytes: NonZeroUsize::new(1024 * 1024).unwrap(),
        max_inflight_requests: NonZeroUsize::new(1024).unwrap(),
    };
    let mut ws_policy = WsPolicy::permissive_for_tests();
    ws_policy.max_connections = NonZeroUsize::new(1).unwrap();
    let app = build_projection_router(state, limits, ws_policy, axum::Router::new());

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let url = format!("ws://{addr}/ws");

    let (mut ws1, resp1) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("1st WS connect");
    assert_eq!(
        resp1.status(),
        101,
        "1st upgrade must be 101 Switching Protocols"
    );

    let result = tokio_tungstenite::connect_async(&url).await;
    assert!(
        matches!(
            &result,
            Err(tokio_tungstenite::tungstenite::Error::Http(r)) if r.status() == 503
        ),
        "2nd connection must be rejected with 503; got: {result:?}"
    );

    ws1.close(None).await.ok();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let (_ws3, resp3) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("3rd WS connect after permit release");
    assert_eq!(
        resp3.status(),
        101,
        "3rd upgrade must be 101 after permit released"
    );

    server.abort();
}

/// Sanity: with permissive limits, the WS Extension is present and
/// `/v1/healthz` returns 200 — confirms the layers don't accidentally
/// short-circuit valid traffic.
#[tokio::test]
async fn permissive_limits_allow_healthz() {
    let source: Arc<MockProjectionSource> = MockProjectionSource::new();
    let state = ProjectionState::from_arc(source);
    let limits = LayerLimits::permissive_for_tests();
    let app = build_projection_router(
        state,
        limits,
        WsPolicy::permissive_for_tests(),
        axum::Router::new(),
    );

    let req = Request::builder()
        .uri("/v1/healthz")
        .body(Body::empty())
        .expect("request build");

    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
}
