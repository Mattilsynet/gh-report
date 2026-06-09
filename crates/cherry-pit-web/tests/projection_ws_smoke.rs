//! Smoke integration test for the projection adapter's WebSocket
//! envelope contract (m5-projection-port Phase 3d; CHE-0049 R13).
//!
//! Runs end-to-end against a real axum server bound to `127.0.0.1:0`
//! with a `TestProjectionSource` impl driving snapshot + broadcast.
//! Asserts every WS frame the server emits parses as JSON with `"v": 1`
//! at the outer envelope, demonstrating the surface refuses any raw
//! `EventEnvelope<E>` shape (runtime guard complements `trybuild`
//! coverage in `tests/compile_fail/` per CHE-0028).
//!
//! One test. No proptests. Determinism enforced via:
//! - ephemeral port allocation (no race with other tests),
//! - `tokio::time::timeout` on every await that could deadlock,
//! - oneshot for bound-address handshake,
//! - in-test broadcast `Sender` is the only delta source.

#![cfg(feature = "projection")]

use std::time::Duration;

use cherry_pit_core::CorrelationContext;
use cherry_pit_web::{LayerLimits, PageUpdate, ProjectionState, build_projection_router};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

mod common;
use common::MockProjectionSource;

/// CHE-0049 R13 smoke test — the WS surface emits envelopes carrying
/// `"v": 1` and structurally refuses any raw `EventEnvelope<E>` payload.
///
/// Flow:
/// 1. Spin axum on `127.0.0.1:0`; capture the bound port via oneshot.
/// 2. Connect via `tokio-tungstenite`; expect the initial `connected`
///    envelope to parse as JSON with `"v": 1` and no `EventEnvelope`
///    field shapes.
/// 3. Push one `PageUpdate` through the test source's broadcast Sender;
///    expect a `type=update` envelope, again with `"v": 1` and no
///    `EventEnvelope` shape.
/// 4. Close the WS cleanly.
#[tokio::test(flavor = "current_thread")]
#[expect(
    clippy::too_many_lines,
    reason = "linear end-to-end WS smoke; splitting would obscure the flow"
)]
async fn ws_envelope_carries_v1_and_refuses_event_envelope_shape() {
    let source = MockProjectionSource::new();
    let tx = source.tx();
    let state = ProjectionState::from_arc(source);
    let app = build_projection_router(
        state,
        LayerLimits::permissive_for_tests(),
        axum::Router::new(),
    );

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral");
    let addr = listener.local_addr().expect("bound");

    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let url = format!("ws://{addr}/ws");
    let (mut ws, _resp) = timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async(&url),
    )
    .await
    .expect("connect timeout")
    .expect("ws connect failed");

    let frame = timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("recv timeout")
        .expect("ws closed early")
        .expect("ws read error");
    let text = match frame {
        Message::Text(t) => t.to_string(),
        other => panic!("expected Text frame, got {other:?}"),
    };
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("connected: not JSON");
    assert_eq!(
        parsed["v"], 1,
        "connected envelope must carry CHE-0049 R13 version field `\"v\": 1`; got {parsed}"
    );
    assert_eq!(
        parsed["type"], "connected",
        "first frame must be type=connected"
    );

    for forbidden in [
        "event_id",
        "aggregate_id",
        "caused_by",
        "payload",
        "sequence",
        "events",
        "envelope",
    ] {
        assert!(
            parsed.get(forbidden).is_none(),
            "WS envelope must NOT carry raw EventEnvelope field `{forbidden}`; got {parsed}"
        );
    }

    let update = PageUpdate::new(
        vec!["index.html".into()],
        "test-repo".into(),
        "2026-05-11T00:00:00Z".into(),
        CorrelationContext::none(),
    );
    tx.send(update).expect("broadcast send");

    let frame = timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("recv timeout")
        .expect("ws closed early")
        .expect("ws read error");
    let text = match frame {
        Message::Text(t) => t.to_string(),
        other => panic!("expected Text frame, got {other:?}"),
    };
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("delta: not JSON");

    assert_eq!(
        parsed["v"], 1,
        "delta envelope must carry CHE-0049 R13 version field `\"v\": 1`; got {parsed}"
    );
    assert_eq!(
        parsed["type"], "update",
        "delta envelope must be type=update"
    );
    assert_eq!(
        parsed["pages"][0], "index.html",
        "delta envelope must echo PageUpdate.pages"
    );
    assert_eq!(parsed["repo"], "test-repo");
    assert_eq!(parsed["timestamp"], "2026-05-11T00:00:00Z");

    for forbidden in [
        "event_id",
        "aggregate_id",
        "caused_by",
        "payload",
        "sequence",
        "events",
        "envelope",
    ] {
        assert!(
            parsed.get(forbidden).is_none(),
            "delta envelope must NOT carry raw EventEnvelope field `{forbidden}`; got {parsed}"
        );
    }

    let _ = ws.send(Message::Close(None)).await;
    let _ = timeout(Duration::from_secs(2), ws.next()).await;
    server.abort();
    let _ = server.await;
}
