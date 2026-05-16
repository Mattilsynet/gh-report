//! HTTP + WebSocket handlers for the projection adapter.
//!
//! This module is gated behind `feature = "projection"` and realises the
//! per-route handlers that [`super::build_projection_router`] mounts.
//! Three concerns live here:
//!
//! - HTTP health probes (`/v1/healthz`, `/v1/readyz`) — static JSON bodies,
//!   zero-allocation hot path.
//! - HTTP snapshot fetch (`/v1/{*path}`) — serves a [`PageEntry`] from the
//!   current snapshot with ETag/304 + zstd negotiation per CHE-0049 R11.
//! - WebSocket upgrade (`/ws`) — per-session subscribe to
//!   [`ProjectionSource::subscribe`] forwarding [`PageUpdate`] deltas. The
//!   envelope is JSON `"v": 1` per CHE-0049 R13.
//!
//! ### CHE-0049 R11 — drop-and-resync backpressure
//!
//! When the per-socket receiver observes
//! [`broadcast::error::RecvError::Lagged`] we close the socket with WS
//! close code **1001 "Going Away"** (RFC 6455 §7.4.1). The chosen close
//! code communicates "the server cannot deliver continuity on this
//! channel; reconnect". The client recovers by following the R11
//! reconnect protocol: HTTP-fetch the current snapshot, then re-attach a
//! fresh WS for subsequent deltas. The snapshot per CHE-0048:R2 supplies
//! the durable checkpoint; no replay-on-reconnect logic is required on
//! the server.
//!
//! The donor crate (module `server::ws_session`) handled lag by
//! sending a `{"type":"reload"}` text frame and continuing the session.
//! Cherry-pit-web's R11 contract upgrades that to a close frame: the
//! envelope contract forbids ambiguity between "delta you missed" and
//! "delta you got" — the only way to guarantee that is to terminate the
//! session and force a snapshot re-fetch.
//!
//! ### A3 — no `Deserialize` bleed
//!
//! The outbound WS frames are constructed from pre-serialised
//! `Arc<str>` payloads on [`PageUpdate::json`] (built by
//! [`PageUpdate::new`]). No outbound DTO derives `Deserialize` and no
//! inbound DTO exists — client frames are discarded after pong handling.
//! Raw [`cherry_pit_core::EventEnvelope`] is structurally unreachable
//! from this surface: the broadcast carries `PageUpdate` only.
//!
//! ### Scope deferred to follow-on work
//!
//! The donor crate's HTTP/WS concurrency semaphores
//! ([`super::config::ValidatedConfig::concurrency_limit`] /
//! [`super::config::ValidatedConfig::ws_max_connections`]) are NOT
//! threaded through [`super::build_projection_router`] at this phase —
//! the Phase-2 signature does not admit a `ValidatedConfig` parameter
//! and changing it would balloon the public surface. The semaphore
//! gates are defence-in-depth; production deployments rate-limit at the
//! ingress layer regardless.
//!
//! Similarly the donor's `Origin`-vs-`Host` CSWSH validator is retained
//! (`validate_ws_origin` below) but the `csp_override` knob is not
//! threaded — the projection surface ships [`DEFAULT_CSP`] only at this
//! phase.

use std::collections::HashMap;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{CloseCode, CloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;

use super::port::ProjectionSource;
use super::state::{PageEntry, ProjectionState};
use crate::middleware::compression::{Encoding, negotiate_encoding};

// ===========================================================================
// Constants
// ===========================================================================

/// Default Content-Security-Policy header value applied to every response
/// emitted by the projection adapter. Matches the donor crate's default.
pub(crate) const DEFAULT_CSP: &str = "default-src 'self'; style-src 'self'; script-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'";

/// Maximum inbound WebSocket message size (bytes). Client frames are
/// discarded after pong handling; 4 KB suffices.
pub(crate) const WS_MAX_MESSAGE_SIZE: usize = 4096;

/// WebSocket close code 1001 "Going Away" — RFC 6455 §7.4.1. Used to
/// signal drop-and-resync on `broadcast::RecvError::Lagged` per
/// CHE-0049 R11.
pub(crate) const WS_CLOSE_GOING_AWAY: CloseCode = 1001;

// ===========================================================================
// HTTP handlers
// ===========================================================================

/// Liveness probe. Always returns 200 — proves the process is alive.
/// Static body, zero allocation.
pub(crate) async fn healthz() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"v":1,"status":"ok"}"#,
    )
}

/// Readiness probe. Maps [`ProjectionSource::is_ready`] to 200/503.
pub(crate) async fn readyz<P>(State(state): State<ProjectionState<P>>) -> impl IntoResponse
where
    P: ProjectionSource,
{
    if state.source().is_ready() {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"v":1,"status":"ready"}"#,
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"v":1,"status":"not_ready"}"#,
        )
    }
}

/// Fetch a page from the current snapshot.
///
/// Returns:
/// - **200** with body + Content-Type + `ETag` + zstd-encoded body when
///   the client advertises `Accept-Encoding: zstd`.
/// - **304** when `If-None-Match` matches the page's weak `ETag`.
/// - **405** for non-GET/HEAD methods.
/// - **503** when no snapshot has been published yet.
/// - **404** when the snapshot does not contain the requested key.
///
/// `path` is the captured wildcard segment from `/v1/{*path}`. The
/// projection adapter is read-only; path normalisation is delegated to
/// the wildcard capture (axum rejects traversal sequences before they
/// reach the handler).
pub(crate) async fn snapshot_get<P>(
    State(state): State<ProjectionState<P>>,
    request: axum::http::Request<axum::body::Body>,
) -> Response
where
    P: ProjectionSource,
{
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, "GET, HEAD")],
            "method not allowed",
        )
            .into_response();
    }

    let Some(snapshot) = state.source().snapshot() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "snapshot not yet available",
        )
            .into_response();
    };

    // Strip the `/v1/` prefix; axum delivers `path` already normalised.
    let raw_path = request.uri().path();
    let key = raw_path.strip_prefix("/v1/").unwrap_or(raw_path);

    let Some(page) = resolve_page(&snapshot, key) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    serve_page(page, request.headers(), StatusCode::OK)
}

/// Resolve a request key through the cache.
///
/// Resolution order mirrors the donor's
/// donor crate `server::resolve_cache_key`:
/// 1. Direct match.
/// 2. `{key}/index.html` when no extension or trailing slash.
/// 3. `{key}.html` when no extension and no trailing slash.
fn resolve_page<'a>(snapshot: &'a HashMap<String, PageEntry>, key: &str) -> Option<&'a PageEntry> {
    if let Some(page) = snapshot.get(key) {
        return Some(page);
    }
    let trimmed = key.trim_end_matches('/');
    let trailing_slash = key.ends_with('/') && key.len() > 1;
    let no_ext = !trimmed
        .rsplit('/')
        .next()
        .is_some_and(|last| last.contains('.'));

    if (trailing_slash || no_ext) && trimmed != "index.html" && !trimmed.ends_with("/index.html") {
        let index_key = if trimmed.is_empty() {
            "index.html".to_string()
        } else {
            format!("{trimmed}/index.html")
        };
        if let Some(page) = snapshot.get(&index_key) {
            return Some(page);
        }
    }

    if !trailing_slash && no_ext && !trimmed.is_empty() {
        let html_key = format!("{trimmed}.html");
        if let Some(page) = snapshot.get(&html_key) {
            return Some(page);
        }
    }

    None
}

/// Build a full HTTP response from a cached [`PageEntry`].
///
/// Handles ETag/304 negotiation and zstd encoding negotiation. The
/// projection adapter is read-only; this is the only branch that
/// serialises a page body.
fn serve_page(page: &PageEntry, request_headers: &HeaderMap, status: StatusCode) -> Response {
    let has_compressed = page.body_zstd.is_some();

    // Conditional request → 304 Not Modified.
    if let Some(if_none_match) = request_headers.get(header::IF_NONE_MATCH)
        && etag_weak_match(if_none_match, &page.etag)
    {
        let mut resp = Response::new(axum::body::Body::empty());
        *resp.status_mut() = StatusCode::NOT_MODIFIED;
        resp.headers_mut().insert(header::ETAG, page.etag.clone());
        resp.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        if has_compressed {
            resp.headers_mut()
                .insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
        }
        return resp;
    }

    // Negotiate content encoding per RFC 7231 §5.3.4. `negotiate_encoding`
    // is the full q-value parser (honours `q=0` refusals and quality
    // ordering); a strict-superset behavioural-equivalence test against
    // the prior simplified inline check
    // (`s.split(',').any(|p| p.trim().starts_with("zstd"))`) is asserted
    // in `middleware::compression::tests`.
    let wants_zstd = request_headers
        .get(header::ACCEPT_ENCODING)
        .is_some_and(|h| negotiate_encoding(h) == Encoding::Zstd);

    let (body_bytes, content_encoding, content_length): (
        Bytes,
        Option<&'static str>,
        Option<HeaderValue>,
    ) = if wants_zstd && has_compressed {
        (
            page.body_zstd.clone().expect("checked has_compressed"),
            Some("zstd"),
            page.content_length_zstd.clone(),
        )
    } else {
        (page.body.clone(), None, Some(page.content_length.clone()))
    };

    let mut resp = Response::new(axum::body::Body::from(body_bytes));
    *resp.status_mut() = status;
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, page.content_type.clone());
    resp.headers_mut().insert(header::ETAG, page.etag.clone());
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if let Some(cl) = content_length {
        resp.headers_mut().insert(header::CONTENT_LENGTH, cl);
    }
    if let Some(enc) = content_encoding {
        resp.headers_mut()
            .insert(header::CONTENT_ENCODING, HeaderValue::from_static(enc));
    }
    if has_compressed {
        resp.headers_mut()
            .insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    }
    resp
}

/// Weak `ETag` comparison per RFC 7232 §2.3.2.
fn etag_weak_match(client_val: &HeaderValue, server_val: &HeaderValue) -> bool {
    fn strip_weak(v: &[u8]) -> &[u8] {
        v.strip_prefix(b"W/").unwrap_or(v)
    }
    if client_val.as_bytes() == b"*" {
        return true;
    }
    strip_weak(client_val.as_bytes()) == strip_weak(server_val.as_bytes())
}

// ===========================================================================
// WebSocket handler
// ===========================================================================

/// WebSocket upgrade handler.
///
/// Validates `Origin == Host` (CSWSH defence per donor) and delegates to
/// [`ws_session`] on a fresh per-connection task.
///
/// `state` is the typed projection state; cloning is an `Arc` bump.
pub(crate) async fn ws_handler<P>(
    ws: WebSocketUpgrade,
    State(state): State<ProjectionState<P>>,
    headers: HeaderMap,
) -> Response
where
    P: ProjectionSource,
{
    if !validate_ws_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.max_message_size(WS_MAX_MESSAGE_SIZE)
        .on_upgrade(move |socket| ws_session::<P>(socket, state))
}

/// Per-connection WebSocket session.
///
/// Subscribes to the broadcast channel via
/// [`ProjectionSource::subscribe`] and forwards [`PageUpdate`] payloads
/// as JSON text frames. The payload [`PageUpdate::json`] already carries
/// the `"v": 1` envelope per CHE-0049 R13 (built by
/// [`PageUpdate::new`]).
///
/// On [`broadcast::error::RecvError::Lagged`] we close the socket with
/// WS code [`WS_CLOSE_GOING_AWAY`] (1001) per CHE-0049 R11
/// drop-and-resync. The client follows the R11 reconnect path:
/// HTTP-fetch-snapshot, then re-attach WS.
pub(crate) async fn ws_session<P>(socket: WebSocket, state: ProjectionState<P>)
where
    P: ProjectionSource,
{
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.source().subscribe();

    // Initial "connected" envelope. Carries `"v": 1` for symmetry with
    // the delta envelope (CHE-0049 R13).
    if sender
        .send(Message::Text(Utf8Bytes::from_static(
            r#"{"v":1,"type":"connected"}"#,
        )))
        .await
        .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            // Branch 1: client frames (discard text/binary, observe Close).
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                    _ => {} // discard
                }
            }

            // Branch 2: forward broadcast deltas to the client.
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        // Pre-serialised JSON — zero per-connection
                        // serialisation cost. `Arc<str>` clone is a
                        // refcount bump; conversion to Utf8Bytes copies
                        // bytes (axum's `Utf8Bytes` borrows from `str`).
                        let payload: Utf8Bytes = (*event.json).to_owned().into();
                        if sender.send(Message::Text(payload)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_n)) => {
                        // Per CHE-0049 R11 reconnect model: close with
                        // 1001 "Going Away" so the client treats this
                        // as a forced disconnect and follows the
                        // reconnect protocol (HTTP-fetch-snapshot, then
                        // re-attach WS). `n` is intentionally unobserved
                        // — the close code carries the only signal that
                        // matters.
                        let _ = sender
                            .send(Message::Close(Some(CloseFrame {
                                code: WS_CLOSE_GOING_AWAY,
                                reason: Utf8Bytes::from_static("lagged; resync via snapshot"),
                            })))
                            .await;
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    // Best-effort close (ignore errors; client may already be gone).
    let _ = sender.send(Message::Close(None)).await;
}

/// Validate `Origin == Host` for CSWSH defence. Mirrors the donor
/// crate's `server::validate_ws_origin` semantics: absent
/// `Origin` is permitted (non-browser client); non-HTTP(S) schemes are
/// rejected; default ports are normalised before comparison.
pub(crate) fn validate_ws_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true; // non-browser client; not subject to CSWSH
    };
    let Ok(origin_str) = origin.to_str() else {
        return false;
    };
    let Some((scheme, after_scheme)) = origin_str.split_once("://") else {
        return false;
    };
    let default_port = match scheme {
        "https" => "443",
        "http" => "80",
        _ => return false,
    };
    let origin_authority = after_scheme
        .split('/')
        .next()
        .expect("split always yields at least one element");
    let Some(host_hdr) = headers.get(header::HOST) else {
        return false;
    };
    let Ok(host_str) = host_hdr.to_str() else {
        return false;
    };
    if origin_authority == host_str {
        return true;
    }
    normalize_authority(origin_authority, default_port)
        == normalize_authority(host_str, default_port)
}

fn normalize_authority<'a>(authority: &'a str, default_port: &str) -> (&'a str, &'a str) {
    // IPv6 bracketed form: `[::1]:8080`.
    if let Some(close_bracket) = authority.find(']') {
        let (host, rest) = authority.split_at(close_bracket + 1);
        let port = rest.strip_prefix(':').unwrap_or("");
        let port = if port == default_port { "" } else { port };
        return (host, port);
    }
    if let Some((host, port)) = authority.rsplit_once(':') {
        let port = if port == default_port { "" } else { port };
        (host, port)
    } else {
        (authority, "")
    }
}

// ===========================================================================
// Default CSP middleware
// ===========================================================================

/// Apply [`DEFAULT_CSP`] to every response unless an inner handler set
/// `Content-Security-Policy` directly. Mirrors the donor's
/// `security_headers` but ports only the CSP knob — `X-Frame-Options`,
/// `X-Content-Type-Options`, `Referrer-Policy`, `Permissions-Policy`,
/// and HSTS are already injected by [`crate::middleware::security_headers`]
/// which the consumer composes with [`super::build_projection_router`]
/// downstream.
pub(crate) async fn projection_default_csp(
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    if !response
        .headers()
        .contains_key(header::CONTENT_SECURITY_POLICY)
    {
        response.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(DEFAULT_CSP),
        );
    }
    response
}

// ===========================================================================
// Router builder (called from `super::build_projection_router`)
// ===========================================================================

/// Build the projection router with `/v1/healthz`, `/v1/readyz`,
/// `/v1/{*path}` HTTP routes and `/ws` WS upgrade.
///
/// Per CHE-0049 R9 HTTP routes carry the `/v1/` URL prefix. Per R13
/// `/ws` is unversioned; the envelope `"v": 1` carries the contract
/// version instead.
pub(crate) fn build<P>(state: ProjectionState<P>) -> Router
where
    P: ProjectionSource,
{
    Router::new()
        .route("/v1/healthz", get(healthz))
        .route("/v1/readyz", get(readyz::<P>))
        .route("/ws", get(ws_handler::<P>))
        .route("/v1/{*path}", get(snapshot_get::<P>))
        .with_state(state)
        .layer(axum::middleware::from_fn(projection_default_csp))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Header-only unit tests; full WS lifecycle lives in the
    // `projection_ws_smoke` integration test.

    fn make_headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn validate_ws_origin_allows_absent_origin() {
        assert!(validate_ws_origin(&HeaderMap::new()));
    }

    #[test]
    fn validate_ws_origin_allows_exact_match() {
        let h = make_headers(&[("origin", "https://example.com"), ("host", "example.com")]);
        assert!(validate_ws_origin(&h));
    }

    #[test]
    fn validate_ws_origin_allows_default_port_normalisation() {
        let h = make_headers(&[
            ("origin", "https://example.com:443"),
            ("host", "example.com"),
        ]);
        assert!(validate_ws_origin(&h));
    }

    #[test]
    fn validate_ws_origin_rejects_mismatched_host() {
        let h = make_headers(&[("origin", "https://evil.com"), ("host", "example.com")]);
        assert!(!validate_ws_origin(&h));
    }

    #[test]
    fn validate_ws_origin_rejects_non_http_scheme() {
        let h = make_headers(&[("origin", "file://local"), ("host", "example.com")]);
        assert!(!validate_ws_origin(&h));
    }

    #[test]
    fn etag_match_handles_weak_prefix() {
        let server = HeaderValue::from_static(r#"W/"abc""#);
        let client = HeaderValue::from_static(r#""abc""#);
        assert!(etag_weak_match(&client, &server));
    }

    #[test]
    fn etag_match_handles_wildcard() {
        let server = HeaderValue::from_static(r#"W/"abc""#);
        let client = HeaderValue::from_static("*");
        assert!(etag_weak_match(&client, &server));
    }

    #[test]
    fn resolve_page_direct_match() {
        let mut snapshot = HashMap::new();
        snapshot.insert(
            "index.html".to_string(),
            PageEntry::new("index.html", b"<html/>".to_vec()),
        );
        assert!(resolve_page(&snapshot, "index.html").is_some());
    }

    #[test]
    fn resolve_page_directory_index_fallback() {
        let mut snapshot = HashMap::new();
        snapshot.insert(
            "blog/index.html".to_string(),
            PageEntry::new("index.html", b"<html/>".to_vec()),
        );
        assert!(resolve_page(&snapshot, "blog").is_some());
        assert!(resolve_page(&snapshot, "blog/").is_some());
    }

    #[test]
    fn resolve_page_clean_url_fallback() {
        let mut snapshot = HashMap::new();
        snapshot.insert(
            "about.html".to_string(),
            PageEntry::new("about.html", b"<html/>".to_vec()),
        );
        assert!(resolve_page(&snapshot, "about").is_some());
    }

    #[test]
    fn resolve_page_returns_none_for_unknown_key() {
        let snapshot: HashMap<String, PageEntry> = HashMap::new();
        assert!(resolve_page(&snapshot, "missing").is_none());
    }

    // ── Ported from donor crate `server` tests (Phase 4b') ───
    //
    // Names retained byte-for-byte from donor. Donor's `resolve_cache_key`
    // ↔ `resolve_page` (here); donor's `trailing_slash: bool` parameter is
    // derived from the key in `resolve_page`, so tests pass the key with
    // or without the trailing `/` to express the intended branch.

    // ── etag_weak_match: extended coverage ──────────────────────

    #[test]
    fn etag_weak_match_identical() {
        let a = HeaderValue::from_static("W/\"abc123\"");
        let b = HeaderValue::from_static("W/\"abc123\"");
        assert!(etag_weak_match(&a, &b));
    }

    #[test]
    fn etag_weak_match_strong_client_weak_server() {
        let client = HeaderValue::from_static("W/\"abc123\"");
        let server = HeaderValue::from_static("\"abc123\"");
        assert!(etag_weak_match(&client, &server));
    }

    #[test]
    fn etag_weak_match_different_values() {
        let a = HeaderValue::from_static("W/\"abc\"");
        let b = HeaderValue::from_static("W/\"def\"");
        assert!(!etag_weak_match(&a, &b));
    }

    #[test]
    fn etag_weak_match_empty_values() {
        let a = HeaderValue::from_static("");
        let b = HeaderValue::from_static("");
        assert!(etag_weak_match(&a, &b));
    }

    #[test]
    fn etag_weak_match_w_prefix_only() {
        let a = HeaderValue::from_static("W/");
        let b = HeaderValue::from_static("W/");
        assert!(etag_weak_match(&a, &b));
    }

    #[test]
    fn etag_weak_match_malformed_no_closing_quote() {
        let a = HeaderValue::from_static("W/\"abc");
        let b = HeaderValue::from_static("W/\"abc");
        assert!(etag_weak_match(&a, &b));
    }

    #[test]
    fn etag_weak_match_malformed_vs_well_formed() {
        let a = HeaderValue::from_static("W/\"abc");
        let b = HeaderValue::from_static("W/\"abc\"");
        assert!(!etag_weak_match(&a, &b));
    }

    // ── origin_validation: extended coverage ────────────────────

    #[test]
    fn origin_validation_same_origin_with_port() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com:8080"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("example.com:8080"));
        assert!(validate_ws_origin(&headers));
    }

    #[test]
    fn origin_validation_origin_has_default_port_host_omits() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com:443"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(validate_ws_origin(&headers));
    }

    #[test]
    fn origin_validation_no_host_header_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com"),
        );
        assert!(!validate_ws_origin(&headers));
    }

    #[test]
    fn origin_validation_http_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:8080"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("localhost:8080"));
        assert!(validate_ws_origin(&headers));
    }

    #[test]
    fn origin_validation_subdomain_mismatch_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://sub.example.com"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(!validate_ws_origin(&headers));
    }

    #[test]
    fn origin_validation_differing_non_default_ports_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com:9999"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("example.com:8080"));
        assert!(!validate_ws_origin(&headers));
    }

    #[test]
    fn origin_validation_ftp_scheme_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("ftp://example.com"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(!validate_ws_origin(&headers));
    }

    #[test]
    fn origin_validation_no_scheme_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_static("example.com"));
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(!validate_ws_origin(&headers));
    }

    #[test]
    fn origin_validation_origin_with_trailing_path() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com/path"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(validate_ws_origin(&headers));
    }

    // ── resolve_page: extended coverage (donor's `resolve_cache_key`) ──
    //
    // Donor signature: `resolve_cache_key(cache, key, trailing_slash: bool)`.
    // Here: `resolve_page(snapshot, key)` derives the trailing-slash bit
    // from `key.ends_with('/')`. Tests pass the appropriate key form.

    #[test]
    fn resolve_directory_index_without_trailing_slash_no_ext() {
        let mut snapshot = HashMap::new();
        snapshot.insert(
            "about/index.html".to_string(),
            PageEntry::new("about/index.html", b"<html>about</html>".to_vec()),
        );
        // No trailing slash, no extension → tries both about/index.html and about.html
        assert!(resolve_page(&snapshot, "about").is_some());
    }

    #[test]
    fn resolve_no_self_loop_on_index_html() {
        let mut snapshot = HashMap::new();
        snapshot.insert(
            "index.html".to_string(),
            PageEntry::new("index.html", b"<html>root</html>".to_vec()),
        );
        // Should find index.html directly, not try index.html/index.html
        assert!(resolve_page(&snapshot, "index.html").is_some());
    }

    #[test]
    fn resolve_no_self_loop_nested_index() {
        let mut snapshot = HashMap::new();
        snapshot.insert(
            "blog/index.html".to_string(),
            PageEntry::new("blog/index.html", b"<html>blog</html>".to_vec()),
        );
        // Direct match — should not try blog/index.html/index.html
        assert!(resolve_page(&snapshot, "blog/index.html").is_some());
    }

    // ── normalize_authority: IPv6 coverage (donor Finding 9.1) ──

    #[test]
    fn normalize_authority_ipv6_loopback_no_port() {
        let (host, port) = normalize_authority("[::1]", "443");
        assert_eq!(host, "[::1]");
        assert_eq!(port, "");
    }

    #[test]
    fn normalize_authority_ipv6_with_non_default_port() {
        let (host, port) = normalize_authority("[::1]:8080", "443");
        assert_eq!(host, "[::1]");
        assert_eq!(port, "8080");
    }

    #[test]
    fn normalize_authority_ipv6_with_default_port_stripped() {
        let (host, port) = normalize_authority("[::1]:443", "443");
        assert_eq!(host, "[::1]");
        assert_eq!(port, "");
    }

    #[test]
    fn normalize_authority_ipv6_full_address() {
        let (host, port) = normalize_authority("[2001:db8::1]:9090", "443");
        assert_eq!(host, "[2001:db8::1]");
        assert_eq!(port, "9090");
    }
}
