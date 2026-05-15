//! Shared test fixtures for `cherry-pit-web` integration tests.
//!
//! Created during m5-projection-port Phase 4 (sub-mission 4a'') to provide a
//! uniform substrate for the ported test suites (4b sync server, 4c HTTP
//! adapter, 4d WS + proptest). Each fixture is a plain Rust item — no mock
//! frameworks (BC8) and no `quics-*` dep edge (BC10).
//!
//! ## Items
//!
//! - [`MockProjectionSource`] — in-memory `ProjectionSource` impl driven by
//!   tests. Subsumes the inline `TestProjectionSource` previously embedded
//!   in `projection_ws_smoke.rs` (broader name; same shape).
//! - [`spawn_test_server`] — binds an axum [`build_projection_router`] to
//!   `127.0.0.1:0`, returns the bound [`SocketAddr`] + an `abort` handle
//!   (used by 4c for reqwest-driven HTTP tests).
//! - [`assert_envelope_v1`] — asserts the WebSocket envelope contract
//!   (CHE-0049 R13): `value["v"] == 1` literally (BC1) + a caller-supplied
//!   `kind` field.
//!
//! Everything is gated on the `projection` feature because every consumer
//! pulls in `cherry_pit_web::ProjectionSource` / `ProjectionState` /
//! `build_projection_router`, all of which are themselves projection-gated.
//!
//! `#![allow(dead_code)]` — integration-test `mod common` is included by
//! every test file independently; unused items per file are expected.

#![cfg(feature = "projection")]
#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::http::HeaderValue;
use cherry_pit_web::{
    PageEntry, PageUpdate, ProjectionSource, ProjectionState, build_projection_router,
    security_headers,
};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// In-memory `ProjectionSource` for tests.
///
/// Holds a `Mutex<Option<Arc<HashMap<…>>>>` snapshot slot (mutable via
/// [`set_snapshot`](Self::set_snapshot)) and a `broadcast::Sender<PageUpdate>`
/// exposed via [`tx`](Self::tx) so test bodies can inject deltas
/// deterministically. `ready` defaults to `true`; flip via
/// [`set_ready`](Self::set_ready).
///
/// Plain struct — no mock framework, satisfying BC8.
pub struct MockProjectionSource {
    snapshot: Mutex<Option<Arc<HashMap<String, PageEntry>>>>,
    tx: broadcast::Sender<PageUpdate>,
    ready: AtomicBool,
}

impl MockProjectionSource {
    /// New source. `ready = true`, `snapshot = None`, broadcast channel
    /// capacity 64 (matches the inline shape from `projection_ws_smoke.rs`).
    #[must_use]
    pub fn new() -> Arc<Self> {
        let (tx, _) = broadcast::channel(64);
        Arc::new(Self {
            snapshot: Mutex::new(None),
            tx,
            ready: AtomicBool::new(true),
        })
    }

    /// Clone of the broadcast sender. Test bodies use this to inject
    /// `PageUpdate` deltas observed by the WS handler.
    #[must_use]
    pub fn tx(&self) -> broadcast::Sender<PageUpdate> {
        self.tx.clone()
    }

    /// Install a new snapshot slot.
    pub fn set_snapshot(&self, snap: Option<Arc<HashMap<String, PageEntry>>>) {
        *self.snapshot.lock().expect("snapshot mutex poisoned") = snap;
    }

    /// Flip readiness. Default is `true`.
    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::Release);
    }
}

impl ProjectionSource for MockProjectionSource {
    fn snapshot(&self) -> Option<Arc<HashMap<String, PageEntry>>> {
        self.snapshot
            .lock()
            .expect("snapshot mutex poisoned")
            .clone()
    }

    fn subscribe(&self) -> broadcast::Receiver<PageUpdate> {
        self.tx.subscribe()
    }

    fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
}

/// Handle returned by [`spawn_test_server`]. Drop / call
/// [`shutdown`](Self::shutdown) to abort the serving task; the bound
/// `addr` is the address callers connect to.
pub struct TestServer {
    pub addr: SocketAddr,
    handle: JoinHandle<()>,
}

impl TestServer {
    /// Abort the serving task and await its termination. Idempotent in
    /// practice — a second call after `abort` is a no-op.
    pub async fn shutdown(self) {
        self.handle.abort();
        let _ = self.handle.await;
    }
}

/// Bind the projection router to `127.0.0.1:0`, spawn the serving task,
/// and return the bound address + a [`TestServer`] handle.
///
/// Generic over the `ProjectionSource` impl so callers can pass either
/// [`MockProjectionSource`] or a real driver. The returned address is the
/// kernel-assigned ephemeral port — safe to run in parallel with other
/// tests.
pub async fn spawn_test_server<P>(source: Arc<P>) -> TestServer
where
    P: ProjectionSource,
{
    let state = ProjectionState::from_arc(source);
    let app = build_projection_router(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral");
    let addr = listener.local_addr().expect("bound");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    TestServer { addr, handle }
}

/// Assert the WebSocket envelope contract (CHE-0049 R13).
///
/// Two assertions:
/// 1. `value["v"] == 1` *literally* (per BC1: the version field is the
///    integer `1`, not a string, not stringly-typed).
/// 2. `value["type"] == expected_kind` — the discriminant for the
///    envelope variant (e.g. `"connected"`, `"update"`). The wire field
///    name is `"type"` (matching the donor and the dest's outbound
///    payloads at `handlers.rs:336-338` and `state::PageUpdate::new`);
///    the parameter is named `expected_kind` for caller readability.
///
/// `panic!`s on failure with the full JSON value for diagnosis.
pub fn assert_envelope_v1(value: &serde_json::Value, expected_kind: &str) {
    assert_eq!(
        value["v"], 1,
        "envelope must carry CHE-0049 R13 version field `\"v\": 1`; got {value}"
    );
    assert_eq!(
        value["type"], expected_kind,
        "envelope must carry type=\"{expected_kind}\"; got {value}"
    );
}

// ===========================================================================
// HTTP integration substrate (m5 Phase 4c — projection_http tests)
// ===========================================================================

/// Spawn the projection router with the full security-header middleware
/// composed on top.
///
/// `build_projection_router` ships only the `projection_default_csp`
/// middleware; the donor's HTTP integration tests assert the **full**
/// security stack (X-Frame-Options, X-Content-Type-Options, CSP,
/// Referrer-Policy, Permissions-Policy, HSTS). In production the
/// consumer composes [`security_headers`] downstream — this helper
/// reproduces that composition for tests so [`assert_security_headers`]
/// has the headers to inspect.
pub async fn spawn_test_server_secured<P>(source: Arc<P>) -> TestServer
where
    P: ProjectionSource,
{
    let state = ProjectionState::from_arc(source);
    let csp = HeaderValue::from_static(
        "default-src 'self'; style-src 'self'; script-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'",
    );
    let app = build_projection_router(state).layer(axum::middleware::from_fn(move |req, next| {
        let csp = csp.clone();
        security_headers(req, next, csp)
    }));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral");
    let addr = listener.local_addr().expect("bound");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    TestServer { addr, handle }
}

/// Build a snapshot from `(key, filename, body)` triples.
///
/// The `filename` drives Content-Type inference inside [`PageEntry::new`];
/// it may differ from `key` when tests need a fallback-resolution shape
/// (e.g. key `"about/index.html"` with filename `"index.html"`).
#[must_use]
pub fn mk_snapshot(entries: &[(&str, &str, &str)]) -> Arc<HashMap<String, PageEntry>> {
    let mut map = HashMap::new();
    for (key, filename, body) in entries {
        map.insert(
            (*key).to_string(),
            PageEntry::new(filename, body.as_bytes().to_vec()),
        );
    }
    Arc::new(map)
}

/// GET helper (no extra headers). Uses a default `reqwest::Client`.
pub async fn http_get(addr: SocketAddr, path: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("http://{addr}{path}"))
        .send()
        .await
        .expect("reqwest send")
}

/// GET helper with custom headers. Caller supplies `(name, value)`
/// pairs; the builder is constructed with `.no_gzip()` so test bodies
/// observe raw `Content-Encoding` without reqwest auto-decompressing.
pub async fn http_get_with_headers(
    addr: SocketAddr,
    path: &str,
    headers: &[(&str, &str)],
) -> reqwest::Response {
    let client = reqwest::Client::builder()
        .no_gzip()
        .build()
        .expect("reqwest client");
    let mut req = client.get(format!("http://{addr}{path}"));
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    req.send().await.expect("reqwest send")
}

/// Issue an arbitrary-method request (used by the 405 + HEAD tests).
pub async fn http_request(
    addr: SocketAddr,
    method: reqwest::Method,
    path: &str,
) -> reqwest::Response {
    reqwest::Client::new()
        .request(method, format!("http://{addr}{path}"))
        .send()
        .await
        .expect("reqwest send")
}

/// Assert the full donor security-header set on `resp`.
///
/// Mirrors the donor crate's `server::tests::assert_security_headers`. The
/// `endpoint` label surfaces in panic messages for diagnosis.
pub fn assert_security_headers(resp: &reqwest::Response, endpoint: &str) {
    let h = resp.headers();
    assert_eq!(
        h.get("x-frame-options").map(|v| v.to_str().unwrap()),
        Some("DENY"),
        "missing X-Frame-Options on {endpoint}"
    );
    assert_eq!(
        h.get("x-content-type-options").map(|v| v.to_str().unwrap()),
        Some("nosniff"),
        "missing X-Content-Type-Options on {endpoint}"
    );
    assert!(
        h.get("content-security-policy")
            .map(|v| v.to_str().unwrap())
            .is_some_and(|v| v.contains("default-src")),
        "missing or invalid CSP on {endpoint}"
    );
    assert_eq!(
        h.get("referrer-policy").map(|v| v.to_str().unwrap()),
        Some("no-referrer"),
        "missing Referrer-Policy on {endpoint}"
    );
    assert_eq!(
        h.get("permissions-policy").map(|v| v.to_str().unwrap()),
        Some("camera=(), microphone=(), geolocation=()"),
        "missing Permissions-Policy on {endpoint}"
    );
    assert_eq!(
        h.get("strict-transport-security")
            .map(|v| v.to_str().unwrap()),
        Some("max-age=63072000; includeSubDomains"),
        "missing or incorrect Strict-Transport-Security on {endpoint}"
    );
}

/// Decode a zstd-compressed body. Used by encoding-negotiation tests.
#[must_use]
pub fn decode_zstd(compressed: &[u8]) -> Vec<u8> {
    zstd::decode_all(compressed).expect("zstd decode")
}

/// Assert that re-requesting `path` with the response's `ETag` as
/// `If-None-Match` yields `304 Not Modified` carrying `Cache-Control:
/// no-cache`. The first response is consumed solely to capture the
/// `ETag`.
pub async fn assert_etag_yields_304(addr: SocketAddr, path: &str) {
    let first = http_get(addr, path).await;
    assert_eq!(first.status(), 200, "first GET {path}");
    let etag = first
        .headers()
        .get("etag")
        .expect("first GET should include ETag")
        .to_str()
        .expect("ETag is valid ASCII")
        .to_string();
    let second = http_get_with_headers(addr, path, &[("if-none-match", &etag)]).await;
    assert_eq!(second.status(), 304, "second GET with matching ETag");
    assert_eq!(
        second
            .headers()
            .get("cache-control")
            .expect("304 should include Cache-Control")
            .to_str()
            .unwrap(),
        "no-cache",
    );
}
