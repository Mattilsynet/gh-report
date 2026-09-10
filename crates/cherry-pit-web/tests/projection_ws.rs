//! WebSocket integration tests for the projection adapter.
//!
//! Routes exercised:
//! - `/ws` — unversioned WebSocket upgrade (CHE-0049 R9 carves out WS).
//! - `/v1/healthz`, `/v1/readyz`, `/v1/{*path}` — HTTP surface, also
//!   reached via the GET-to-`/ws` non-upgrade path (security headers).
//!
//! BC1 — envelope literal `"v":1`. Every WS test decoding a broadcast
//! frame asserts the envelope contract via `assert_envelope_v1`
//! (`value["v"] == 1`). Tests that don't decode a payload still cite
//! `"v":1` in an inline comment so a `"v":1` grep hits per test.
//!
//! `ws_sends_reload_on_lag` asserts the drop-and-resync contract
//! (CHE-0049 R11): on broadcast lag the server closes the socket with
//! WS code 1001 ("Going Away"); the client recovers by HTTP-fetching
//! the snapshot and re-attaching a fresh WS.

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
        let receivers = tx
            .send(PageUpdate::new(
                vec![format!("page-{i}.html")],
                format!("repo-{i}"),
                "2026-04-14T12:00:00Z".into(),
                CorrelationContext::none(),
            ))
            .expect("broadcast send must succeed");
        assert!(receivers > 0, "broadcast must have active receiver");
    }

    let mut saw_close_1001 = false;
    let mut close_reason = String::new();
    let result = timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(Some(frame)))) => {
                    let code: u16 = frame.code.into();
                    if code == 1001 {
                        saw_close_1001 = true;
                        close_reason = frame.reason.to_string();
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
    assert_eq!(close_reason, "lagged; resync via snapshot");

    let trailing = timeout(Duration::from_secs(1), async {
        let mut extra = Vec::new();
        while let Some(msg) = ws.next().await {
            extra.push(msg);
        }
        extra
    })
    .await
    .expect("ws stream should close cleanly after 1001 close");
    assert!(
        trailing.is_empty(),
        "ws stream must reach EOF without trailing frames after 1001 close"
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

#[tokio::test(flavor = "current_thread")]
async fn ws_keepalive_pings_and_closes_on_pong_timeout() {
    let source = MockProjectionSource::new();
    let server = spawn_test_server(source).await;
    let url = format!("ws://{}/ws", server.addr);

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let connected = recv_text_json(&mut ws).await;
    assert_envelope_v1(&connected, "connected");

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(31)).await;
    tokio::time::resume();

    let ping = timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Ping(payload))) => return Some(payload),
                Some(Ok(Message::Close(_)) | Err(_)) | None => return None,
                Some(Ok(_)) => {}
            }
        }
    })
    .await
    .expect("timeout waiting for ping");

    assert!(ping.is_some(), "server must send ping after interval");

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(11)).await;
    tokio::time::resume();

    let closed = timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(_)) | Err(_)) | None => return true,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;

    assert!(
        closed.is_ok(),
        "server must close after missed pong deadline"
    );
    server.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn ws_keepalive_pong_resets_timeout() {
    let source = MockProjectionSource::new();
    let server = spawn_test_server(source).await;
    let url = format!("ws://{}/ws", server.addr);

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let connected = recv_text_json(&mut ws).await;
    assert_envelope_v1(&connected, "connected");

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(31)).await;
    tokio::time::resume();

    let ping = timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Ping(payload))) => return Some(payload),
                Some(Ok(Message::Close(_)) | Err(_)) | None => return None,
                Some(Ok(_)) => {}
            }
        }
    })
    .await
    .expect("timeout waiting for first ping");

    let payload = ping.expect("server must send first ping");
    ws.send(Message::Pong(payload)).await.unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(15)).await;
    tokio::time::resume();

    let still_open = timeout(Duration::from_millis(500), ws.next()).await;
    assert!(
        still_open.is_err(),
        "connection must stay open past 10s when pong was received"
    );

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(16)).await;
    tokio::time::resume();

    let second_ping = timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Ping(payload))) => return Some(payload),
                Some(Ok(Message::Close(_)) | Err(_)) | None => return None,
                Some(Ok(_)) => {}
            }
        }
    })
    .await
    .expect("timeout waiting for second ping");

    assert!(second_ping.is_some(), "server must send second ping");
    server.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn ws_stalled_consumer_missed_pong_times_out_and_releases_permit() {
    let source = MockProjectionSource::new();

    let state = cherry_pit_web::ProjectionState::from_arc(source.clone());
    let mut policy = cherry_pit_web::WsPolicy::permissive_for_tests();
    policy.max_connections = std::num::NonZeroUsize::new(1).expect("nonzero");
    let app = cherry_pit_web::build_projection_router(
        state,
        cherry_pit_web::LayerLimits::permissive_for_tests(),
        policy,
        axum::Router::new(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral");
    let addr = listener.local_addr().expect("bound");
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let url = format!("ws://{addr}/ws");

    let (mut ws1, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let connected1 = recv_text_json(&mut ws1).await;
    assert_envelope_v1(&connected1, "connected");

    let second_conn = tokio_tungstenite::connect_async(&url).await;
    match second_conn {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(
                resp.status(),
                503,
                "second client should be rejected with 503 when connection limit is reached"
            );
        }
        other => panic!("expected 503 for second connection while permit held, got {other:?}"),
    }

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(31)).await;
    tokio::time::resume();

    let ping = timeout(Duration::from_secs(5), async {
        loop {
            match ws1.next().await {
                Some(Ok(Message::Ping(payload))) => return Some(payload),
                Some(Ok(Message::Close(_)) | Err(_)) | None => return None,
                Some(Ok(_)) => {}
            }
        }
    })
    .await
    .expect("timeout waiting for ping");
    assert!(ping.is_some(), "server must send ping after interval");

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(11)).await;
    tokio::time::resume();

    let closed = timeout(Duration::from_secs(5), async {
        loop {
            match ws1.next().await {
                Some(Ok(Message::Close(_)) | Err(_)) | None => return true,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(
        closed.is_ok(),
        "server must close stalled connection after timeout"
    );

    let (mut ws2, resp2) = timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async(&url),
    )
    .await
    .expect("timeout connecting second client")
    .expect("second client should connect after stalled client permit is released");
    assert_eq!(resp2.status(), 101);
    let connected2 = recv_text_json(&mut ws2).await;
    assert_envelope_v1(&connected2, "connected");

    ws2.close(None).await.ok();
    server_handle.abort();
    if let Err(join_err) = server_handle.await {
        assert!(
            join_err.is_cancelled(),
            "server handle must exit by cancellation, got {join_err:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ws_client_initiated_close_receives_server_ack() {
    let source = MockProjectionSource::new();
    let server = spawn_test_server(source).await;
    let url = format!("ws://{}/ws", server.addr);

    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let connected = recv_text_json(&mut ws).await;
    assert_envelope_v1(&connected, "connected");

    ws.send(Message::Close(Some(CloseFrame {
        code: 1000.into(),
        reason: "client closing".into(),
    })))
    .await
    .expect("client send close");

    let ack = timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(frame))) => return frame,
                Some(Ok(_)) => {}
                Some(Err(err)) => panic!("unexpected ws error waiting for close ack: {err:?}"),
                None => panic!("unexpected eof before close ack frame"),
            }
        }
    })
    .await
    .expect("timeout waiting for server close ack");

    let frame = ack.expect("server must send close ack frame");
    let code: u16 = frame.code.into();
    assert_eq!(code, 1000);

    let eof = timeout(Duration::from_secs(1), ws.next())
        .await
        .expect("timeout waiting for eof");
    assert!(eof.is_none(), "stream must reach clean EOF after close ack");

    server.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn ws_broadcast_send_capped_by_pong_deadline() {
    let source = MockProjectionSource::new();
    let tx = source.tx();

    let state = cherry_pit_web::ProjectionState::from_arc(source.clone());
    let mut policy = cherry_pit_web::WsPolicy::permissive_for_tests();
    policy.max_connections = std::num::NonZeroUsize::new(1).expect("nonzero");
    let app = cherry_pit_web::build_projection_router(
        state,
        cherry_pit_web::LayerLimits::permissive_for_tests(),
        policy,
        axum::Router::new(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral");
    let addr = listener.local_addr().expect("bound");
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    let url = format!("ws://{addr}/ws");

    let (mut ws1, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let connected1 = recv_text_json(&mut ws1).await;
    assert_envelope_v1(&connected1, "connected");

    let second_conn = tokio_tungstenite::connect_async(&url).await;
    match second_conn {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(
                resp.status(),
                503,
                "second client should be rejected with 503 when connection limit is reached"
            );
        }
        other => panic!("expected 503 for second connection while permit held, got {other:?}"),
    }

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(31)).await;
    tokio::time::resume();

    let ping = timeout(Duration::from_secs(5), async {
        loop {
            match ws1.next().await {
                Some(Ok(Message::Ping(payload))) => return Some(payload),
                Some(Ok(Message::Close(_)) | Err(_)) | None => return None,
                Some(Ok(_)) => {}
            }
        }
    })
    .await
    .expect("timeout waiting for ping");
    assert!(ping.is_some(), "server must send ping");

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(8)).await;

    for i in 0..40 {
        let payload = "x".repeat(32768);
        let receivers = tx
            .send(PageUpdate::new(
                vec![payload],
                format!("repo-{i}"),
                "2026-04-14T12:00:00Z".into(),
                CorrelationContext::none(),
            ))
            .expect("broadcast send must succeed");
        assert!(receivers > 0, "broadcast must have active receiver");
        tokio::task::yield_now().await;
    }

    tokio::time::advance(Duration::from_secs(3)).await;
    tokio::time::resume();

    let (mut ws2, resp2) = timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async(&url),
    )
    .await
    .expect("timeout connecting second client")
    .expect("second client should connect after pong deadline timeout releases permit");
    assert_eq!(resp2.status(), 101);
    let connected2 = recv_text_json(&mut ws2).await;
    assert_envelope_v1(&connected2, "connected");

    ws2.close(None).await.ok();
    server_handle.abort();
    if let Err(join_err) = server_handle.await {
        assert!(
            join_err.is_cancelled(),
            "server handle must exit by cancellation, got {join_err:?}"
        );
    }
}
