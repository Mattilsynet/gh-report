//! Proptest integration tests for the projection adapter (m5 Phase 4d).
//!
//! Ported from the donor crate's `server` module per the donor audit at
//! `.ooda/preflight-4c-donor-audit-1778536369.md` §"Proptest". 7
//! proptest cases total:
//!
//! - 4 path proptests (donor `server.rs:3743` block) port directly via
//!   the public `normalize_request_path` surface re-exported at
//!   `cherry_pit_web::normalize_request_path`. These exercise the
//!   percent-decode + traversal-rejection invariants.
//!
//! - 3 origin proptests (donor `server.rs:3806` block) **REFRAMED** as
//!   HTTP-integration proptests. The donor calls `validate_ws_origin`
//!   directly; in the dest that function is `pub(crate)` at
//!   `handlers.rs:398` and promoting it to `pub` would breach the OOS
//!   list (production code is OOS for sub-4d — see
//!   `.ooda/brief-sub-4d.md` §"Out of scope"). Instead we drive each
//!   case through a real `tokio_tungstenite::connect_async` handshake
//!   against an axum server bound to `127.0.0.1:0`, synthesising the
//!   `Origin` header on the upgrade request. The accept/reject
//!   topology is observable on the public WS endpoint and matches the
//!   donor's invariants.
//!
//! Route surface touched: `/v1/{*path}` is referenced by the path
//! invariants only insofar as `normalize_request_path` is the helper
//! used by the v1 snapshot handler (`handlers.rs:140-172`); the WS
//! upgrade lives at `/ws`. BC1 `"v":1` is not asserted here — proptests
//! exercise structural invariants, not envelope shape.
//!
//! Runtime envelope: default 256 cases × ~3 proptests × ~50ms/case for
//! the HTTP-integration ones ≈ ~40s worst case. We narrow the origin
//! block to 64 cases via `#[proptest_config]` to keep wall-clock low
//! while preserving invariant coverage.

#![cfg(feature = "projection")]

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use cherry_pit_web::{
    LayerLimits, PageEntry, PageUpdate, ProjectionSource, ProjectionState, build_projection_router,
    normalize_request_path,
};
use proptest::prelude::*;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

mod common;

// ===========================================================================
// Local, blocking-aware substrate for the origin proptests
// ===========================================================================
//
// `proptest!` runs cases synchronously in the test thread. The
// HTTP-integration cases need to drive `connect_async` to completion
// per case; we wrap each case in a fresh single-threaded
// `tokio::runtime::Runtime` so the proptest body remains a plain `fn`
// that returns a `Result<(), TestCaseError>`. This is the same pattern
// the donor's `proptest_origin` block achieves implicitly (its calls
// are synchronous — they touch a pure function). Spinning a server per
// case is the cost of reframing.

/// Minimal `ProjectionSource` impl for origin-proptest servers. No
/// snapshot, no broadcast traffic — the upgrade path is rejected on
/// `Origin`/`Host` checks before any handler logic runs.
struct StubSource(broadcast::Sender<PageUpdate>);

impl StubSource {
    fn new() -> Arc<Self> {
        let (tx, _) = broadcast::channel(8);
        Arc::new(Self(tx))
    }
}

impl ProjectionSource for StubSource {
    fn snapshot(&self) -> Option<Arc<std::collections::HashMap<String, PageEntry>>> {
        None
    }
    fn subscribe(&self) -> broadcast::Receiver<PageUpdate> {
        self.0.subscribe()
    }
    fn is_ready(&self) -> bool {
        true
    }
}

/// Bind a router with `StubSource` to `127.0.0.1:0` synchronously
/// within an async block. Returns the `SocketAddr` plus a `JoinHandle`
/// the caller drops to shut down.
async fn spawn_stub() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let source = StubSource::new();
    let state = ProjectionState::from_arc(source);
    let app: Router =
        build_projection_router(state, LayerLimits::permissive_for_tests(), Router::new());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, handle)
}

/// Build a WS upgrade request with the given `Origin` and `Host` and
/// return the connect-attempt outcome: `Ok(true)` on 101 upgrade,
/// `Ok(false)` on 403 reject, `Err(msg)` on any other shape (which the
/// caller treats as a test failure — we don't want unobserved HTTP
/// statuses to silently pass).
async fn try_upgrade(
    addr: SocketAddr,
    origin: Option<&str>,
    host_override: Option<&str>,
) -> Result<bool, String> {
    let url = format!("ws://{addr}/ws");
    let host = host_override.map_or_else(|| format!("{addr}"), ToString::to_string);
    let mut builder = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(&url)
        .header("Host", host)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        );
    if let Some(o) = origin {
        builder = builder.header("Origin", o);
    }
    let request = builder.body(()).map_err(|e| format!("build req: {e}"))?;

    match tokio_tungstenite::connect_async(request).await {
        Ok((mut ws, response)) => {
            // 101 path — close the socket so the server task exits the
            // session loop cleanly.
            use futures_util::SinkExt;
            let _ = ws
                .send(tokio_tungstenite::tungstenite::Message::Close(None))
                .await;
            Ok(response.status() == 101)
        }
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            if resp.status() == 403 {
                Ok(false)
            } else {
                Err(format!("unexpected http status: {}", resp.status()))
            }
        }
        Err(other) => Err(format!("connect error: {other}")),
    }
}

// ===========================================================================
// Path invariants (direct port — `normalize_request_path` is public)
// ===========================================================================

proptest! {
    /// Donor `server.rs:3745` — `normalize_request_path` never panics on
    /// arbitrary Unicode input.
    #[test]
    fn path_never_panics(input in "\\PC{0,500}") {
        let _ = normalize_request_path(&input);
    }

    /// Donor `server.rs:3752` — output key never contains `..`, null
    /// bytes, or backslashes.
    #[test]
    fn path_output_key_never_contains_dangerous_sequences(input in "\\PC{0,500}") {
        if let Some(result) = normalize_request_path(&input) {
            prop_assert!(
                !result.key.contains(".."),
                "key contains '..': {:?}", result.key
            );
            prop_assert!(
                !result.key.contains('\0'),
                "key contains null byte: {:?}", result.key
            );
            prop_assert!(
                !result.key.contains('\\'),
                "key contains backslash: {:?}", result.key
            );
        }
    }

    /// Donor `server.rs:3771` — inputs whose percent-decoded form
    /// contains `..` are rejected.
    #[test]
    fn path_rejects_traversal_after_decode(
        prefix in "[a-z]{0,5}",
        suffix in "[a-z]{0,5}",
    ) {
        let input = format!("/{prefix}/../{suffix}");
        prop_assert!(normalize_request_path(&input).is_none());
    }

    /// Donor `server.rs:3781` — output key never starts with a slash.
    #[test]
    fn path_output_key_never_starts_with_slash(input in "\\PC{0,500}") {
        if let Some(result) = normalize_request_path(&input) {
            prop_assert!(
                !result.key.starts_with('/'),
                "key starts with '/': {:?}", result.key
            );
        }
    }
}

// ===========================================================================
// Origin invariants (REFRAMED — drive HTTP upgrades end-to-end)
// ===========================================================================

/// Strategy mirroring donor `server.rs:3800` — random Origin and Host
/// header combinations.
fn origin_host_strategy() -> impl Strategy<Value = (Option<String>, Option<String>)> {
    let origin = proptest::option::of("[a-z]{3,8}://[a-z0-9.:\\[\\]]{1,30}(/[a-z]{0,10})?");
    let host = proptest::option::of("[a-z0-9.:\\[\\]]{1,30}");
    (origin, host)
}

/// Tighter case count for HTTP-integration proptests — each case spins
/// a server and drives a full TCP+WS handshake. 64 cases per origin
/// proptest × 3 proptests ≈ 192 server lifecycles; ~10–20 s wall-clock
/// total. Donor's pure-function block was 256 cases; we trade case
/// breadth for end-to-end coverage.
const ORIGIN_CASES: u32 = 64;

proptest! {
    #![proptest_config(ProptestConfig { cases: ORIGIN_CASES, .. ProptestConfig::default() })]

    /// Donor `server.rs:3808` (reframed) — server never panics on
    /// arbitrary header combinations. We assert that `try_upgrade`
    /// returns *some* defined outcome (either accept or 403 reject);
    /// any other status would be reported as `Err(...)` and fail the
    /// case.
    #[test]
    fn origin_never_panics((origin, host) in origin_host_strategy()) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build rt");
        let outcome = rt.block_on(async {
            let (addr, handle) = spawn_stub().await;
            let r = try_upgrade(addr, origin.as_deref(), host.as_deref()).await;
            handle.abort();
            r
        });
        prop_assert!(outcome.is_ok(), "unexpected outcome: {outcome:?}");
    }

    /// Donor `server.rs:3825` (reframed) — with no `Origin` header the
    /// upgrade is accepted (non-browser client, not subject to CSWSH).
    #[test]
    fn origin_no_origin_always_true(_host in "[a-z0-9.]{1,20}") {
        // The `Host` header on the upgrade is filled by the client
        // library to the bind address; the donor's `host` strategy
        // input is unused here because the dest's `validate_ws_origin`
        // short-circuits on absent Origin before reading Host.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build rt");
        let accepted = rt.block_on(async {
            let (addr, handle) = spawn_stub().await;
            let r = try_upgrade(addr, None, None).await;
            handle.abort();
            r
        });
        prop_assert_eq!(accepted, Ok(true), "no-Origin upgrade must be accepted");
    }

    /// Donor `server.rs:3835` (reframed) — cross-origin upgrades are
    /// rejected with 403. We synthesise distinct hostnames and check
    /// `try_upgrade` returns `Ok(false)`.
    #[test]
    fn origin_cross_origin_rejected(
        origin_host in "[a-z]{3,8}\\.[a-z]{2,4}",
        _filler in "[a-z]{3,8}\\.[a-z]{2,4}",
    ) {
        let origin = format!("https://{origin_host}");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build rt");
        let rejected = rt.block_on(async {
            let (addr, handle) = spawn_stub().await;
            // Host header defaults to the bind address (127.0.0.1:PORT);
            // the `Origin` carries a distinct random hostname so the
            // Origin-vs-Host comparison must fail.
            let r = try_upgrade(addr, Some(&origin), None).await;
            handle.abort();
            r
        });
        prop_assert_eq!(rejected, Ok(false), "cross-origin upgrade must be rejected with 403");
    }
}
