//! WebSocket integration tests for the projection adapter (m5 Phase 4d).
//!
//! Ported from the donor crate's `server` module per the donor audit at
//! `.ooda/preflight-4c-donor-audit-1778536369.md` §"Deferred to 4d (WS)".
//! 9 of the audit's 11 WS tests land here; 3 were deferred as
//! architectural-scope misses per moltke's `ReDecompose` (see
//! `.ooda/brief-sub-4d.md` §"Objective (REVISED…)"):
//! - `ws_semaphore_exhaustion_returns_503` and
//!   `ws_semaphore_permit_released_on_disconnect` test
//!   `ValidatedConfig::ws_max_connections` plumbing that is deliberately
//!   absent from `build_projection_router` per `handlers.rs:43-57`.
//! - `ws_js_has_correct_content_type_and_zstd` tests a `/ws.js` route
//!   that does not exist in the dest router.
//!
//! Each filed as a `m5-followup` bd task under `adr-fmt-io96`.
//!
//! Routes touched by these tests (per `handlers.rs:489-492`):
//! - `/ws`  — unversioned WebSocket upgrade (CHE-0049 R9 carves out WS).
//! - `/v1/healthz`, `/v1/readyz`, `/v1/{*path}` — HTTP surface (referenced
//!   by `ws_endpoint_has_security_headers` which exercises the GET-to-`/ws`
//!   non-upgrade path through the same security stack).
//!
//! BC1 — envelope literal `"v":1`. Every WS test that decodes a broadcast
//! frame asserts the envelope contract via `assert_envelope_v1` (which
//! checks `value["v"] == 1` literally). Tests that don't decode a payload
//! (upgrade-only, security headers, origin rejection, oversized rejection)
//! still cite `"v":1` in an inline comment so PSC9's `rg '"v":1'` grep
//! hits ≥ once per test.
//!
//! `ws_sends_reload_on_lag` — **REWRITE**, not a direct port. The donor
//! (`server.rs:2052` in the removed donor crate) emits a text frame
//! `{"type":"reload"}` on broadcast lag and continues the session. The
//! dest (`handlers.rs:14-32`, `handlers.rs:368-383`) closes the socket
//! with WS code 1001 ("Going Away") per CHE-0049 R11 drop-and-resync —
//! the client recovers by HTTP-fetching the snapshot and re-attaching a
//! fresh WS. We assert the close-code-1001 behaviour.

#![cfg(feature = "projection")]

use std::time::Duration;

use cherry_pit_core::CorrelationContext;
use cherry_pit_web::PageUpdate;
use futures_util::{SinkExt, StreamExt};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;

mod common;
use common::{
    MockProjectionSource, assert_envelope_v1, spawn_test_server, spawn_test_server_secured,
};

/// Drain one Text frame and parse it as JSON. Times out after 5s.
async fn recv_text_json(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> serde_json::Value {
    let frame = timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("recv timeout")
        .expect("ws closed early")
        .expect("ws read error");
    let text = match frame {
        Message::Text(t) => t.to_string(),
        other => panic!("expected Text frame, got {other:?}"),
    };
    serde_json::from_str(&text).expect("frame not JSON")
}

/// Donor: `ws_upgrade_returns_101` (`server.rs:1979`). Asserts 101 handshake
/// status + `connected` envelope. BC1 `"v":1` enforced via `assert_envelope_v1`.
#[tokio::test(flavor = "current_thread")]
async fn ws_upgrade_returns_101() {
    let source = MockProjectionSource::new();
    let server = spawn_test_server(source).await;
    let url = format!("ws://{}/ws", server.addr);

    let (mut ws, response) = timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async(&url),
    )
    .await
    .expect("connect timeout")
    .expect("ws connect failed");
    assert_eq!(response.status(), 101);

    let parsed = recv_text_json(&mut ws).await;
    assert_envelope_v1(&parsed, "connected");

    ws.close(None).await.ok();
    server.shutdown().await;
}

/// Donor: `ws_receives_broadcast_update` (`server.rs:2009`). Asserts a
/// broadcast `PageUpdate` arrives at the client as an `update` envelope
/// carrying the wire fields. BC1 `"v":1` enforced.
#[tokio::test(flavor = "current_thread")]
async fn ws_receives_broadcast_update() {
    let source = MockProjectionSource::new();
    let tx = source.tx();
    let server = spawn_test_server(source).await;
    let url = format!("ws://{}/ws", server.addr);

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let connected = recv_text_json(&mut ws).await;
    assert_envelope_v1(&connected, "connected");

    tx.send(PageUpdate::new(
        vec!["index.html".into(), "report.html".into()],
        "my-repo".into(),
        "2026-04-14T12:00:00Z".into(),
        CorrelationContext::none(),
    ))
    .expect("broadcast send");

    let parsed = recv_text_json(&mut ws).await;
    assert_envelope_v1(&parsed, "update");
    assert_eq!(parsed["repo"], "my-repo");
    assert_eq!(parsed["pages"][0], "index.html");
    assert_eq!(parsed["pages"][1], "report.html");
    assert_eq!(parsed["timestamp"], "2026-04-14T12:00:00Z");

    ws.close(None).await.ok();
    server.shutdown().await;
}

/// **REWRITE** of donor `ws_sends_reload_on_lag` (`server.rs:2052`).
///
/// Donor semantics: broadcast overflow → server sends text frame
/// `{"type":"reload"}` and continues. Dest semantics
/// (`handlers.rs:14-32`, lag branch at `handlers.rs:368-383`):
/// `broadcast::error::RecvError::Lagged` → server closes the socket with
/// WS code 1001 "Going Away" + reason `"lagged; resync via snapshot"`
/// per CHE-0049 R11 drop-and-resync. The client recovers by
/// HTTP-fetching the snapshot and re-attaching a fresh WS — no in-band
/// reload frame.
///
/// We saturate the broadcast channel (capacity 64 per `MockProjectionSource::new`)
/// with 200 sends to force `Lagged`, then assert the next observable
/// frame is a Close with code 1001. BC1 `"v":1` is cited in this comment
/// since this test does not decode a payload frame (the close frame
/// carries no JSON envelope by design).
#[tokio::test(flavor = "current_thread")]
async fn ws_sends_reload_on_lag() {
    let source = MockProjectionSource::new();
    let tx = source.tx();
    let server = spawn_test_server(source).await;
    let url = format!("ws://{}/ws", server.addr);

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let connected = recv_text_json(&mut ws).await;
    assert_envelope_v1(&connected, "connected");

    for i in 0..200u32 {
        let _ = tx.send(PageUpdate::new(
            vec![format!("page-{i}.html")],
            format!("repo-{i}"),
            "2026-04-14T12:00:00Z".into(),
            CorrelationContext::none(),
        ));
    }

    let mut saw_close_1001 = false;
    let result = timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(Some(frame)))) => {
                    let code: u16 = frame.code.into();
                    if code == 1001 {
                        saw_close_1001 = true;
                    }
                    return;
                }
                Some(Ok(Message::Close(None)) | Err(_)) | None => return,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;

    assert!(result.is_ok(), "WS did not close after broadcast lag");
    assert!(
        saw_close_1001,
        "dest must close WS with code 1001 on broadcast lag (CHE-0049 R11)"
    );

    server.shutdown().await;
}

/// Donor: `ws_endpoint_has_security_headers` (`server.rs:2142`).
/// Drives a non-upgrade GET against `/ws` and asserts the full security
/// header stack composed onto the secured router. BC1 `"v":1` is cited
/// in this comment; this test inspects HTTP headers only, no envelope
/// body.
#[tokio::test(flavor = "current_thread")]
async fn ws_endpoint_has_security_headers() {
    let source = MockProjectionSource::new();
    let server = spawn_test_server_secured(source).await;

    let resp = reqwest::get(format!("http://{}/ws", server.addr))
        .await
        .expect("reqwest send");
    common::assert_security_headers(&resp, "/ws (non-upgrade)");

    server.shutdown().await;
}

/// Donor: `ws_broadcast_reaches_all_connected_clients` (`server.rs:2259`).
/// Three concurrent clients all receive the same `PageUpdate` delta.
/// BC1 `"v":1` enforced per-client via `assert_envelope_v1`.
#[tokio::test(flavor = "current_thread")]
async fn ws_broadcast_reaches_all_connected_clients() {
    let source = MockProjectionSource::new();
    let tx = source.tx();
    let server = spawn_test_server(source).await;
    let url = format!("ws://{}/ws", server.addr);

    let (mut ws1, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let c1 = recv_text_json(&mut ws1).await;
    assert_envelope_v1(&c1, "connected");
    let (mut ws2, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let c2 = recv_text_json(&mut ws2).await;
    assert_envelope_v1(&c2, "connected");
    let (mut ws3, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let c3 = recv_text_json(&mut ws3).await;
    assert_envelope_v1(&c3, "connected");

    tx.send(PageUpdate::new(
        vec!["index.html".into()],
        "fanout-repo".into(),
        "2026-04-15T12:00:00Z".into(),
        CorrelationContext::none(),
    ))
    .unwrap();

    for (i, ws) in [&mut ws1, &mut ws2, &mut ws3].iter_mut().enumerate() {
        let parsed = recv_text_json(ws).await;
        assert_envelope_v1(&parsed, "update");
        assert_eq!(parsed["repo"], "fanout-repo", "client {i} repo mismatch");
        assert_eq!(
            parsed["pages"][0], "index.html",
            "client {i} pages mismatch"
        );
    }

    ws1.close(None).await.ok();
    ws2.close(None).await.ok();
    ws3.close(None).await.ok();
    server.shutdown().await;
}

/// Donor: `ws_session_ends_on_broadcast_close` (`server.rs:2312`). The
/// donor's test name describes the *intent* (server-side broadcast
/// teardown ends the session) but the donor's implementation actually
/// drives the close from the client side after server abort — the
/// per-session axum task holds its own `ProjectionState` clone which
/// keeps the broadcast `Sender` alive, so a pure server-abort cannot
/// trigger `RecvError::Closed` on its own. We follow the donor pattern:
/// abort the server, send a client-side `Close` frame, and assert the
/// stream drains. Dest's matching handler branch is `handlers.rs:350`
/// (`Message::Close(_)` → break) plus the final best-effort close at
/// `handlers.rs:391`. BC1 `"v":1` enforced on the `connected` envelope.
#[tokio::test(flavor = "current_thread")]
async fn ws_session_ends_on_broadcast_close() {
    let source = MockProjectionSource::new();
    let server = spawn_test_server(source).await;
    let url = format!("ws://{}/ws", server.addr);

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let connected = recv_text_json(&mut ws).await;
    assert_envelope_v1(&connected, "connected");

    server.shutdown().await;

    let _ = ws.send(Message::Close(None)).await;

    let drained = timeout(Duration::from_secs(5), async {
        while let Some(_msg) = ws.next().await {}
    })
    .await;
    assert!(
        drained.is_ok(),
        "WS stream should drain after server shutdown"
    );
}

/// Donor: `ws_rejects_oversized_client_message` (`server.rs:2399`). The
/// dest constrains inbound frames to `WS_MAX_MESSAGE_SIZE = 4096` bytes
/// (`handlers.rs:85`). An 8 KiB text frame must trigger server-side
/// closure. BC1 `"v":1` cited in comment; this test does not decode a
/// payload (it exercises the inbound size guard).
#[tokio::test(flavor = "current_thread")]
async fn ws_rejects_oversized_client_message() {
    let source = MockProjectionSource::new();
    let server = spawn_test_server(source).await;
    let url = format!("ws://{}/ws", server.addr);

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let connected = recv_text_json(&mut ws).await;
    assert_envelope_v1(&connected, "connected");

    let oversized = "x".repeat(8192);
    ws.send(Message::Text(oversized.into())).await.ok();

    let closed = timeout(Duration::from_secs(3), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(_)) | Err(_)) | None => return true,
                _ => {}
            }
        }
    })
    .await;

    assert!(
        closed.is_ok(),
        "server must close on oversized client frame"
    );
    server.shutdown().await;
}

/// Donor: `ws_cross_origin_upgrade_rejected` (`server.rs:2594`). Builds
/// a raw handshake request carrying an `Origin` header from a foreign
/// host and asserts the upgrade is rejected with 403 by the dest's
/// `validate_ws_origin` enforcement (`handlers.rs:307`). BC1 `"v":1`
/// cited in comment; the upgrade is rejected before any envelope flows.
#[tokio::test(flavor = "current_thread")]
async fn ws_cross_origin_upgrade_rejected() {
    let source = MockProjectionSource::new();
    let server = spawn_test_server(source).await;
    let url = format!("ws://{}/ws", server.addr);
    let host = format!("{}", server.addr);

    let request = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(&url)
        .header("Host", host)
        .header("Origin", "https://evil.example.com")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .unwrap();

    let result = tokio_tungstenite::connect_async(request).await;
    match result {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(
                resp.status(),
                403,
                "cross-origin WebSocket upgrade should be rejected with 403"
            );
        }
        Err(other) => panic!("expected HTTP 403, got error: {other}"),
        Ok(_) => panic!("cross-origin upgrade should have been rejected"),
    }

    server.shutdown().await;
}

#[expect(
    dead_code,
    reason = "compile-time reachability anchor for `CloseFrame`; the parameter type is the assertion that the import still resolves at the test crate's call site."
)]
fn close_frame_anchor(_f: CloseFrame) {}

/// Donor: `non_ws_get_to_ws_path_returns_error` (`server.rs:2117`).
/// Picked up opportunistically while implementing the WS suite: a plain
/// (non-upgrade) `GET /ws` must return a client error — axum's WS
/// extractor short-circuits the missing upgrade headers with a 4xx
/// before reaching the handler body. Zero src/ edit, no public-API
/// promotion, no architectural surface change — fits the brief's hard
/// pickup criteria. BC1 `"v":1` cited in comment; this test inspects
/// HTTP status only.
#[tokio::test(flavor = "current_thread")]
async fn non_ws_get_to_ws_path_returns_error() {
    let source = MockProjectionSource::new();
    let server = spawn_test_server(source).await;

    let resp = reqwest::get(format!("http://{}/ws", server.addr))
        .await
        .expect("reqwest send");
    assert!(
        resp.status().is_client_error(),
        "non-upgrade GET to /ws should be a client error, got {}",
        resp.status()
    );

    server.shutdown().await;
}
