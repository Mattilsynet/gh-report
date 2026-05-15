//! In-memory content web server.
//!
//! Serves pre-rendered pages from an in-memory cache (`ArcSwap`-based).
//! Pages are rendered into `HashMap<String, CachedPage>` by the upstream
//! pipeline and swapped atomically — no disk I/O on the serving path.
//!
//! Content encoding: **zstd-only**. Clients that do not advertise
//! `Accept-Encoding: zstd` receive uncompressed (identity) responses.
//! No gzip or deflate support — modern browsers universally support zstd.
//!
//! # Trust Model
//!
//! - **No TLS built in** — requires reverse proxy / load balancer (e.g.,
//!   Cloud Run, nginx) for HTTPS termination.
//! - **No authentication built in** — enforce at the ingress layer.
//! - **No rate limiting built in** — enforce at the ingress layer.
//! - **WebSocket** carries only page-update notifications (cache key names
//!   and timestamps), not secrets.
//! - **Path normalization** is defense-in-depth; cache-only serving
//!   prevents filesystem access regardless.
//!
//! # Known Limitations
//!
//! - **Zstd-only content encoding** — intentional design choice. Gzip
//!   and deflate are not supported. Document in README for consumers.
//! - **`If-None-Match` single-value comparison** — RFC 7232 §3.2 allows
//!   multiple `ETag` values in a single header value, but this server compares
//!   only the first value. Multi-value `If-None-Match` always returns 200.
//!
//! # Security Invariants
//!
//! - Binds to the address specified by the caller (default `127.0.0.1`);
//!   container deployments set `BIND_ADDRESS=0.0.0.0` and rely on TLS
//!   termination at the load balancer / reverse proxy layer (e.g., Cloud Run)
//! - `normalize_request_path` rejects path traversal attempts (`../`,
//!   percent-encoded variants, null bytes, backslashes)
//! - Only keys present in the cache are served — no filesystem access
//! - Adds security response headers to all responses (CSP is configurable
//!   via [`ServerConfig::builder()`](super::config::ServerConfig::builder))
//! - WebSocket upgrades validate `Origin` against `Host` to prevent CSWSH
//! - Request body size capped (configurable, default 1 KB — all endpoints read-only)
//! - Non-GET/HEAD requests to content pages return 405 Method Not Allowed
//! - HTTP concurrency bounded by semaphore (defense-in-depth)

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use percent_encoding::percent_decode_str;
use tokio::net::TcpListener;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use tracing::{Span, debug, info, info_span, warn};

use super::config::ValidatedConfig;
use super::error::ServerError;
use super::state::ServerState;

// ===========================================================================
// Path normalisation (security-critical)
// ===========================================================================

/// Result of normalising a raw URI path.
///
/// Carries the clean cache lookup key and whether the original path had
/// a trailing slash — needed to choose the correct fallback strategy.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NormalizedPath {
    /// Clean cache key (e.g., `"about"`, `"index.html"`, `"blog/post"`).
    pub(crate) key: String,
    /// Whether the original request path ended with `/`.
    pub(crate) has_trailing_slash: bool,
}

/// Normalise a raw URI path into a safe cache lookup key.
///
/// Returns `None` (→ 400) if the path contains traversal sequences, null
/// bytes, backslashes, or any component that resolves to `..` after
/// percent-decoding.
///
/// # Algorithm
///
/// 1. Percent-decode the raw path (handles `%2e%2e`, `%2f`, etc.).
/// 2. Reject null bytes (`\0`), backslashes (`\`).
/// 3. Detect trailing slash before filtering (empty final segment).
/// 4. Split on `/`, skip empty components (collapses `//`), reject `..`.
/// 5. Re-join with `/` — the result is a clean relative path suitable
///    for `HashMap` lookup.
/// 6. Map empty result (root `/`) to `index.html`.
///
/// **Double-encoding resilience:** we decode only once. A doubly-encoded
/// `%252e%252e` decodes to the literal `%2e%2e` which contains no `/` or
/// `.` path separators, so it becomes a harmless (non-existent) cache key.
pub(crate) fn normalize_request_path(raw: &str) -> Option<NormalizedPath> {
    // Step 1: percent-decode.
    let decoded = percent_decode_str(raw).decode_utf8().ok()?;

    // Step 2: reject dangerous bytes.
    if decoded.contains('\0') || decoded.contains('\\') {
        return None;
    }

    // Step 3: detect trailing slash before filtering.
    let has_trailing_slash = decoded.ends_with('/') && decoded.len() > 1;

    // Step 4: split, filter empties, reject traversal.
    let mut segments: Vec<&str> = Vec::new();
    for seg in decoded.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        // Defense-in-depth: reject any segment containing ".." — not just
        // exact-match ".." — to prevent bypass via prefixed/suffixed variants
        // like "\x08.." that survive the exact check.
        if seg.contains("..") {
            return None; // traversal attempt
        }
        segments.push(seg);
    }

    // Step 5: re-join.
    let joined = segments.join("/");

    // Step 6: root → index.html.
    if joined.is_empty() {
        Some(NormalizedPath {
            key: "index.html".to_string(),
            has_trailing_slash: true,
        })
    } else {
        Some(NormalizedPath {
            key: joined,
            has_trailing_slash,
        })
    }
}

// ===========================================================================
// Content-encoding negotiation
// ===========================================================================

/// Supported response encodings, in preference order.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Encoding {
    Zstd,
    Identity,
}

/// Negotiate the best encoding from an `Accept-Encoding` header value.
///
/// Parses q-values (quality factors) per RFC 7231 §5.3.4 and selects the
/// highest-quality supported encoding. Only `zstd` and `identity`
/// are supported. Encodings with `q=0` are excluded.
///
/// Default quality is 1.0 when no q-value is specified.
fn negotiate_encoding(accept: &HeaderValue) -> Encoding {
    let Ok(s) = accept.to_str() else {
        return Encoding::Identity;
    };

    let mut zstd_quality: Option<f32> = None;

    for part in s.split(',') {
        let part = part.trim();
        let (name, params) = match part.split_once(';') {
            Some((n, p)) => (n.trim(), Some(p.trim())),
            None => (part, None),
        };

        // Parse q-value, defaulting to 1.0.
        let quality = params
            .and_then(|p| {
                p.split(';').find_map(|param| {
                    let param = param.trim();
                    param
                        .strip_prefix("q=")
                        .and_then(|q| q.trim().parse::<f32>().ok())
                })
            })
            .unwrap_or(1.0);

        if name == "zstd" && quality > 0.0 {
            zstd_quality = Some(quality);
        }
    }

    if zstd_quality.is_some_and(|q| q > 0.0) {
        Encoding::Zstd
    } else {
        Encoding::Identity
    }
}

// ===========================================================================
// WebSocket handler for real-time page update notifications
// ===========================================================================

/// Server-side ping interval for WebSocket keepalive (seconds).
const WS_PING_INTERVAL_SECS: u64 = 30;

/// Maximum time to wait for a Pong response after sending a Ping (seconds).
/// If the client does not respond within this window, the connection is closed.
const WS_PONG_DEADLINE_SECS: u64 = 10;

/// Maximum inbound WebSocket message size (bytes). Client messages are
/// discarded, so 4 KB is sufficient for Pong frames and future commands.
const WS_MAX_MESSAGE_SIZE: usize = 4096;

/// Validate that the WebSocket `Origin` header matches the request `Host`.
///
/// Prevents Cross-Site WebSocket Hijacking (CSWSH) where an attacker's page
/// opens a WebSocket to the service and the browser attaches session cookies
/// (e.g., Cloud Run IAP cookies), leaking organizational metadata via
/// page-update notifications.
///
/// # Algorithm
///
/// 1. Reject non-HTTP/HTTPS schemes (e.g., `ftp://`, `file://`).
/// 2. Extract the host portion from the `Origin` URL (e.g.,
///    `https://reports.example.com` → `reports.example.com`).
/// 3. Normalize ports: strip the default port for the scheme (443 for
///    `https`, 80 for `http`) from both Origin and Host, then compare
///    the `(hostname, port)` tuples.
/// 4. Return `true` if they match, `false` otherwise.
///
/// If no `Origin` header is present, the request is allowed — same-origin
/// WebSocket connections from browsers always include `Origin`, so its
/// absence indicates a non-browser client (curl, monitoring, etc.) which
/// is not subject to CSWSH.
///
/// # Security trade-off
///
/// Allowing absent `Origin` means non-browser clients can connect without
/// restriction. This is intentional: the WebSocket carries only page-update
/// notifications (cache key names and timestamps), not secrets. Production
/// deployments should enforce authentication at the ingress layer (Cloud Run,
/// reverse proxy) to restrict access to authorized clients.
fn validate_ws_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        // No Origin header → non-browser client (not subject to CSWSH).
        return true;
    };
    let Ok(origin_str) = origin.to_str() else {
        // Non-ASCII Origin → reject.
        return false;
    };

    // Step 1: Extract and validate scheme.
    let Some((scheme, after_scheme)) = origin_str.split_once("://") else {
        return false; // Malformed Origin (no scheme).
    };

    // Only allow http and https schemes.
    let default_port = match scheme {
        "https" => "443",
        "http" => "80",
        _ => return false, // Reject ftp://, file://, etc.
    };

    // Step 2: Extract host:port from Origin (strip any trailing path).
    // SAFETY: `split('/').next()` on any `&str` always returns `Some`.
    let origin_authority = after_scheme
        .split('/')
        .next()
        .expect("split always yields at least one element");

    let Some(host_hdr) = headers.get(header::HOST) else {
        // No Host header at all → cannot validate, reject.
        return false;
    };
    let Ok(host_str) = host_hdr.to_str() else {
        return false;
    };

    // Exact match (including port) is the common case.
    if origin_authority == host_str {
        return true;
    }

    // Step 3: Normalize by stripping default ports, then compare.
    // Split "host:port" into (hostname, port). If no port or port equals
    // the scheme's default, normalize to hostname-only for comparison.
    let origin_normalized = normalize_authority(origin_authority, default_port);
    let host_normalized = normalize_authority(host_str, default_port);

    origin_normalized == host_normalized
}

/// Strip the default port from an authority string for comparison.
///
/// Handles IPv6 bracket notation: `[::1]:8080` splits into hostname
/// `[::1]` and port `8080`. Plain IPv4/hostname uses `rsplit_once(':')`.
///
/// `"example.com:443"` with `default_port = "443"` → `("example.com", "")`.
/// `"example.com:8080"` with `default_port = "443"` → `("example.com", "8080")`.
/// `"example.com"` → `("example.com", "")`.
/// `"[::1]:8080"` → `("[::1]", "8080")`.
fn normalize_authority<'a>(authority: &'a str, default_port: &str) -> (&'a str, &'a str) {
    // IPv6 bracket notation: [addr]:port
    if authority.starts_with('[')
        && let Some(bracket_end) = authority.find(']')
    {
        let after_bracket = &authority[bracket_end + 1..];
        if let Some(port) = after_bracket.strip_prefix(':') {
            let hostname = &authority[..=bracket_end];
            if port == default_port {
                return (hostname, "");
            }
            return (hostname, port);
        }
        // No port after bracket — return whole thing as hostname.
        return (authority, "");
    }
    // Malformed or no bracket — fall through to rsplit_once.

    match authority.rsplit_once(':') {
        Some((hostname, port)) if port == default_port => (hostname, ""),
        Some((hostname, port)) => (hostname, port),
        None => (authority, ""),
    }
}

/// GET /ws — upgrade to WebSocket for real-time page update notifications.
///
/// Protocol (server → client):
///
/// 1. On connect: `{"type":"connected"}`
/// 2. On page update: `{"type":"update","pages":[...],"repo":"...","timestamp":"..."}`
/// 3. On lag (client too slow): `{"type":"reload"}`
///
/// Server sends Ping frames every 30 s; closes the connection if Pong is
/// not received within 10 s.
///
/// Client → server messages are ignored.
///
/// # Security
///
/// - `Origin` header validated against `Host` to prevent Cross-Site
///   WebSocket Hijacking (CSWSH). Returns 403 on mismatch.
/// - Connection count bounded by `ws_semaphore` (configurable permits). Returns
///   503 Service Unavailable when exhausted.
/// - Max inbound frame size: 4 KB (prevents memory exhaustion).
/// - Ping/pong keepalive evicts unresponsive connections.
/// - No application-level authentication — same trust model as the dashboard
///   pages. Authentication is enforced at the ingress layer (Cloud Run /
///   reverse proxy). The WebSocket carries only page-update notifications
///   (cache key names and timestamps), no secrets or credentials.
/// - No application-level rate limiting on WebSocket connections beyond the
///   semaphore cap. Production deployments must enforce rate limiting at the
///   ingress layer (Cloud Run, reverse proxy, or load balancer) to prevent
///   connection-flooding attacks.
async fn ws_handler<S: ServerState>(
    ws: WebSocketUpgrade,
    State(state): State<Arc<S>>,
    Extension(ws_sem): Extension<Arc<tokio::sync::Semaphore>>,
    headers: HeaderMap,
) -> Response {
    // CSWSH protection: reject cross-origin WebSocket upgrades.
    if !validate_ws_origin(&headers) {
        warn!("rejected WebSocket upgrade: Origin does not match Host");
        return StatusCode::FORBIDDEN.into_response();
    }

    let Ok(permit) = ws_sem.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    ws.max_message_size(WS_MAX_MESSAGE_SIZE)
        .on_upgrade(move |socket| ws_session(socket, state, permit))
}

/// Per-connection WebSocket session.
///
/// Subscribes to the broadcast channel and forwards `PageUpdateEvent`s
/// to the client as JSON text frames. If the client falls behind the
/// broadcast buffer (64 messages), sends a `{"type":"reload"}` signal
/// so it can recover via a full page refresh.
///
/// Implements server-side ping/pong keepalive: sends a Ping every
/// [`WS_PING_INTERVAL_SECS`] seconds and closes the connection if a Pong
/// is not received within [`WS_PONG_DEADLINE_SECS`].
///
/// The `_permit` is held for the connection lifetime and released on drop,
/// freeing one slot in `ws_semaphore`.
async fn ws_session<S: ServerState>(
    socket: WebSocket,
    state: Arc<S>,
    _permit: tokio::sync::OwnedSemaphorePermit,
) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.ws_broadcast().subscribe();

    // Send initial connected message.
    if sender
        .send(Message::Text(r#"{"type":"connected"}"#.into()))
        .await
        .is_err()
    {
        return; // client disconnected immediately
    }

    let mut ping_interval =
        tokio::time::interval(std::time::Duration::from_secs(WS_PING_INTERVAL_SECS));
    ping_interval.tick().await; // consume the immediate first tick

    let mut awaiting_pong = false;
    let pong_deadline = std::time::Duration::from_secs(WS_PONG_DEADLINE_SECS);
    // Initialize to far future — only armed when a Ping is sent.
    let far_future = tokio::time::Instant::now() + std::time::Duration::from_hours(24);
    let pong_timeout = tokio::time::sleep_until(far_future);
    tokio::pin!(pong_timeout);

    loop {
        tokio::select! {
            // Branch 1: Receive from client (Pong, Close, or discard).
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Pong(_))) => {
                        awaiting_pong = false;
                        // Disarm the pong deadline timer.
                        pong_timeout.as_mut().reset(far_future);
                    }
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                    _ => {} // discard text/binary from client
                }
            }

            // Branch 2: Forward broadcast events to client.
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        // Use pre-serialized JSON — zero per-connection
                        // serialization cost (O(1) vs O(N) for N clients).
                        if sender
                            .send(Message::Text((*event.json).into()))
                            .await
                            .is_err()
                        {
                            break; // client disconnected
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        debug!(missed = n, "ws client lagged — sending reload signal");
                        if sender
                            .send(Message::Text(r#"{"type":"reload"}"#.into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }

            // Branch 3: Send periodic ping.
            _ = ping_interval.tick() => {
                if awaiting_pong {
                    // Pong still outstanding from previous ping. The pong
                    // deadline timer (Branch 4) will handle the eviction.
                    // Skip sending another ping — one outstanding is enough.
                    continue;
                }
                if sender.send(Message::Ping(vec![].into())).await.is_err() {
                    break;
                }
                awaiting_pong = true;
                // Arm the pong deadline timer.
                pong_timeout.as_mut().reset(
                    tokio::time::Instant::now() + pong_deadline
                );
            }

            // Branch 4: Pong deadline expired — client is unresponsive.
            () = &mut pong_timeout, if awaiting_pong => {
                debug!("ws client missed pong deadline — closing");
                break;
            }
        }
    }

    // Best-effort close frame (ignore errors — client may already be gone).
    let _ = sender.send(Message::Close(None)).await;
}

// ===========================================================================
// Response building helper
// ===========================================================================

/// SVG-specific Content-Security-Policy that blocks script execution.
///
/// SVG with `image/svg+xml` allows embedded `<script>` execution. This CSP
/// overrides the global policy on SVG responses to prevent XSS while still
/// allowing inline CSS and self-referenced images within the SVG.
const SVG_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src 'self'";

/// Build a full HTTP response from a cached page.
///
/// Handles ETag/304 negotiation, zstd content encoding, Content-Type,
/// Content-Length, Cache-Control, and Vary headers. Optionally skips
/// `If-None-Match` (for error pages where 304-on-404 is semantically wrong).
///
/// If the page's Content-Type is `image/svg+xml`, overrides the CSP header
/// to block script execution (SVG XSS mitigation).
fn serve_page(
    page: &super::state::CachedPage,
    request_headers: &HeaderMap,
    status: StatusCode,
    skip_etag_check: bool,
) -> Response {
    let has_compressed = page.body_zstd.is_some();

    // Check If-None-Match for conditional request (304 Not Modified).
    if !skip_etag_check
        && let Some(if_none_match) = request_headers.get(axum::http::header::IF_NONE_MATCH)
        && etag_weak_match(if_none_match, &page.etag)
    {
        let mut resp = Response::new(axum::body::Body::empty());
        *resp.status_mut() = StatusCode::NOT_MODIFIED;
        resp.headers_mut()
            .insert(axum::http::header::ETAG, page.etag.clone());
        resp.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        );
        if has_compressed {
            resp.headers_mut().insert(
                axum::http::header::VARY,
                HeaderValue::from_static("Accept-Encoding"),
            );
        }
        return resp;
    }

    // Negotiate content encoding.
    let encoding = request_headers
        .get(axum::http::header::ACCEPT_ENCODING)
        .map_or(Encoding::Identity, negotiate_encoding);

    let (body_bytes, content_encoding, content_length) = match encoding {
        Encoding::Zstd => match page.body_zstd.as_ref() {
            Some(b) => (b.clone(), Some("zstd"), page.content_length_zstd.clone()),
            None => (page.body.clone(), None, Some(page.content_length.clone())),
        },
        Encoding::Identity => (page.body.clone(), None, Some(page.content_length.clone())),
    };

    let mut resp = Response::new(axum::body::Body::from(body_bytes));
    *resp.status_mut() = status;
    resp.headers_mut()
        .insert(axum::http::header::CONTENT_TYPE, page.content_type.clone());
    resp.headers_mut()
        .insert(axum::http::header::ETAG, page.etag.clone());
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
    if let Some(cl) = content_length {
        resp.headers_mut()
            .insert(axum::http::header::CONTENT_LENGTH, cl);
    }
    if let Some(enc) = content_encoding {
        resp.headers_mut().insert(
            axum::http::header::CONTENT_ENCODING,
            HeaderValue::from_static(enc),
        );
    }
    if has_compressed {
        resp.headers_mut().insert(
            axum::http::header::VARY,
            HeaderValue::from_static("Accept-Encoding"),
        );
    }

    // SVG XSS mitigation: override CSP to block script execution.
    if page.content_type == "image/svg+xml" {
        resp.headers_mut().insert(
            axum::http::header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(SVG_CSP),
        );
    }

    resp
}

// ===========================================================================
// Fallback resolution
// ===========================================================================

/// Check if a key has a file extension (contains a `.` in the last path segment).
///
/// Only inspects the final segment after the last `/` to avoid false
/// positives from dotted directory names (e.g., `v2.0/about` → `false`).
fn has_extension(key: &str) -> bool {
    key.rsplit('/')
        .next()
        .is_some_and(|last| last.contains('.'))
}

/// Resolve a cache key through the fallback chain.
///
/// Resolution order:
/// 1. Direct match: `cache.get(key)`
/// 2. If `has_trailing_slash` or no extension: `cache.get("{key}/index.html")`
///    (skipped if key already ends with `/index.html` or is `index.html`)
/// 3. If no trailing slash and no extension: `cache.get("{key}.html")`
fn resolve_cache_key<'a>(
    cache: &'a std::collections::HashMap<String, super::state::CachedPage>,
    key: &str,
    has_trailing_slash: bool,
) -> Option<&'a super::state::CachedPage> {
    // 1. Direct match.
    if let Some(page) = cache.get(key) {
        return Some(page);
    }

    let no_ext = !has_extension(key);

    // 2. Directory index fallback.
    if has_trailing_slash || no_ext {
        // Guard against self-referential lookup.
        if key != "index.html" && !key.ends_with("/index.html") {
            let index_key = format!("{key}/index.html");
            if let Some(page) = cache.get(&index_key) {
                return Some(page);
            }
        }
    }

    // 3. Clean URL fallback (e.g., /about → about.html).
    if !has_trailing_slash && no_ext {
        let html_key = format!("{key}.html");
        if let Some(page) = cache.get(&html_key) {
            return Some(page);
        }
    }

    None
}

// ===========================================================================
// Cache fallback handler
// ===========================================================================

/// Axum fallback handler that serves pages from the in-memory HTML cache.
///
/// Returns:
/// - **200** with the cached body + Content-Type when the key exists
///   (directly or via fallback chain).
/// - **405** for non-GET/HEAD HTTP methods (this is a read-only service).
/// - **503** when the cache has not been populated yet (no collection run
///   completed).
/// - **400** for paths that fail normalisation (traversal, null bytes, etc.).
/// - **404** for valid paths not present in the cache (serves custom error
///   page if configured).
async fn cache_fallback<S: ServerState>(
    State(state): State<Arc<S>>,
    Extension(error_page_key): Extension<Option<Arc<str>>>,
    request: Request,
) -> Response {
    // Reject non-GET/HEAD methods early — no point normalising paths or
    // looking up the cache for POST/PUT/DELETE/PATCH on a read-only service.
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, "GET, HEAD")],
            "method not allowed",
        )
            .into_response();
    }

    let raw_path = request.uri().path();

    // Normalise the path (security gate).
    let Some(normalized) = normalize_request_path(raw_path) else {
        warn!(path = %raw_path, "rejected path: failed normalisation");
        return (StatusCode::BAD_REQUEST, "bad request").into_response();
    };

    // Load the current cache snapshot.
    let cache_guard = state.html_cache().load();
    let Some(cache) = cache_guard.as_ref() else {
        // No collection has completed yet.
        info!(path = %normalized.key, "cache not populated: returning 503");
        return (StatusCode::SERVICE_UNAVAILABLE, "reports not yet available").into_response();
    };

    // Resolve through fallback chain.
    if let Some(page) = resolve_cache_key(cache, &normalized.key, normalized.has_trailing_slash) {
        debug!(path = %normalized.key, "cache hit: serving page");
        return serve_page(page, request.headers(), StatusCode::OK, false);
    }

    // Custom error page fallback.
    if let Some(ref epk) = error_page_key
        && let Some(error_page) = cache.get(epk.as_ref())
    {
        info!(path = %normalized.key, error_page = %epk, "cache miss: serving custom error page");
        return serve_page(error_page, request.headers(), StatusCode::NOT_FOUND, true);
    }

    info!(path = %normalized.key, "cache miss: page not found");
    (StatusCode::NOT_FOUND, "not found").into_response()
}

/// Weak `ETag` comparison per RFC 7232 §2.3.2.
///
/// Handles the `*` wildcard: `If-None-Match: *` matches any `ETag`.
/// Otherwise strips `W/` prefix (if present) from both values before
/// comparing the opaque-tag portion.
fn etag_weak_match(client_val: &HeaderValue, server_val: &HeaderValue) -> bool {
    fn strip_weak(v: &[u8]) -> &[u8] {
        v.strip_prefix(b"W/").unwrap_or(v)
    }

    // RFC 7232 §3.2: "If-None-Match: *" matches any current representation.
    if client_val.as_bytes() == b"*" {
        return true;
    }

    strip_weak(client_val.as_bytes()) == strip_weak(server_val.as_bytes())
}

/// Start the in-memory report web server and run until the provided shutdown
/// signal completes.
///
/// Serves pages from the in-memory `html_cache` on the state. Binds to
/// `bind_address` on the given port. Container deployments typically pass
/// `"0.0.0.0"`; the default for local development is `"127.0.0.1"`.
///
/// # Arguments
///
/// * `port` — TCP port number (use `0` for ephemeral port).
/// * `bind_address` — IP address to bind to (e.g., `"127.0.0.1"`, `"0.0.0.0"`).
/// * `shutdown` — A future that resolves when the server should shut down.
/// * `state` — Shared application state implementing [`ServerState`].
/// * `config` — Server configuration (concurrency limits, body size limits).
/// * `addr_tx` — Optional oneshot channel to receive the bound `SocketAddr`.
///   Useful for tests using ephemeral ports. Pass `None` if not needed.
/// * `extra_routes` — Optional additional routes (e.g., webhook handler) to
///   merge into the router. Extra routes bring their own body-limit layers;
///   built-in routes apply the `config.max_request_body_bytes` limit via
///   per-route layer.
///
/// # Errors
///
/// Returns [`ServerError`] if the server cannot bind to the requested address.
///
/// # Panics
///
/// Panics if `listener.local_addr()` fails when `addr_tx` is `Some`
/// (listener not bound).
pub(crate) async fn start<S: ServerState>(
    port: u16,
    bind_address: &str,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    state: Arc<S>,
    config: &ValidatedConfig,
    addr_tx: Option<tokio::sync::oneshot::Sender<SocketAddr>>,
    extra_routes: Option<Router<Arc<S>>>,
) -> Result<(), ServerError> {
    // Warn that the server binds to all interfaces.
    if bind_address != "127.0.0.1" && bind_address != "::1" && bind_address != "localhost" {
        warn!(
            bind = %bind_address,
            "server is binding to a non-localhost address; \
             ensure reports are safe for the target network"
        );
    }

    let app = build_router(state, config, extra_routes);

    // Parse the bind address.
    let addr: SocketAddr =
        format!("{bind_address}:{port}")
            .parse()
            .map_err(|e| ServerError::InvalidAddress {
                address: format!("{bind_address}:{port}"),
                source: e,
            })?;

    info!(%addr, "content server listening (in-memory cache)");

    // Bind and serve.
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| ServerError::BindFailed {
            address: addr,
            source: e,
        })?;

    // Send the actual bound address (resolves ephemeral port 0).
    if let Some(tx) = addr_tx {
        let _ = tx.send(listener.local_addr().expect("listener bound successfully"));
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(ServerError::RuntimeFailed)?;

    info!("content server stopped");
    Ok(())
}

/// Default Content-Security-Policy header value.
const DEFAULT_CSP: &str = "default-src 'self'; style-src 'self'; script-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'";

/// Build the [`Router`] with security headers, health endpoints, and tracing.
///
/// Extracted so that tests exercise the exact same router configuration as
/// production.
///
/// # Layers (outermost → innermost)
///
/// 1. **Security headers** — injected on every response.
/// 2. **HTTP concurrency limit** — bounds in-flight requests via semaphore.
///    Returns 503 when limit is reached.
/// 3. **Tracing** — structured request/response logging.
///
/// Built-in routes have a `RequestBodyLimitLayer` (default 1 KB) applied
/// via `.layer()` (covers both matched routes and the cache fallback).
/// Extra routes (e.g., webhook handler) bring their own body-limit layer,
/// enabling different limits per route group.
///
/// # Panics
///
/// Panics if `extra_routes` contains routes that conflict with built-in
/// paths (`/healthz`, `/readyz`, `/ws`). Axum's
/// `Router::merge()` panics on overlapping route definitions.
pub(crate) fn build_router<S: ServerState>(
    state: Arc<S>,
    config: &ValidatedConfig,
    extra_routes: Option<Router<Arc<S>>>,
) -> Router {
    // Create the per-instance concurrency semaphore from config.
    let http_semaphore = Arc::new(tokio::sync::Semaphore::new(config.concurrency_limit()));
    let ws_semaphore = Arc::new(tokio::sync::Semaphore::new(config.ws_max_connections()));
    let body_limit = config.max_request_body_bytes();
    let csp: HeaderValue = HeaderValue::from_str(config.csp_override().unwrap_or(DEFAULT_CSP))
        .expect("CSP validated by builder");
    let error_page_key: Option<Arc<str>> = config.error_page_key().map(Into::into);

    // Built-in routes + fallback with a body limit (e.g., 1 KB for
    // read-only endpoints). Using `.layer()` (not `.route_layer()`) so the
    // body limit also covers the cache fallback handler.
    // Extra routes (e.g., webhook) are merged separately and bring their
    // own body-limit layer, enabling different limits per route group.
    let builtin_routes = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz::<S>))
        .route("/ws", get(ws_handler::<S>))
        .fallback(cache_fallback::<S>)
        .layer(RequestBodyLimitLayer::new(body_limit));

    // Start with built-in routes (including fallback), then merge extra
    // routes which bring their own body-limit layer.
    let mut router = Router::new().merge(builtin_routes);

    if let Some(extra) = extra_routes {
        router = router.merge(extra);
    }

    router
        .with_state(state)
        .layer(Extension(error_page_key))
        .layer(Extension(ws_semaphore))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::extract::Request| {
                    info_span!(
                        "http",
                        method = %request.method(),
                        path = %request.uri().path(),
                    )
                })
                .on_request(|_req: &axum::extract::Request, _span: &Span| {
                    debug!("request started");
                })
                .on_response(
                    |response: &axum::http::Response<_>,
                     latency: std::time::Duration,
                     _span: &Span| {
                        info!(
                            status = response.status().as_u16(),
                            latency_us = u64::try_from(latency.as_micros()).unwrap_or(u64::MAX),
                            "response",
                        );
                    },
                ),
        )
        // HTTP concurrency limit: per-instance semaphore replaces the
        // static LazyLock for test isolation and configurability.
        .layer(middleware::from_fn(move |request, next| {
            let sem = Arc::clone(&http_semaphore);
            http_concurrency_limit(sem, request, next)
        }))
        // Security headers: single middleware replaces 6 separate
        // SetResponseHeaderLayer wrappers, reducing tower Service nesting
        // and virtual dispatch overhead on every request.
        .layer(middleware::from_fn(move |req, next| {
            let csp = csp.clone();
            security_headers(req, next, csp)
        }))
}

/// Inject all security response headers in a single middleware pass.
///
/// Replaces six individual `SetResponseHeaderLayer::overriding(...)` layers
/// with one async function, collapsing 6 levels of `Service::call()`
/// indirection into a single function call.
///
/// The `csp` parameter is resolved from [`ValidatedConfig::csp_override()`] (or the
/// built-in default) and captured by the closure in `build_router`.
async fn security_headers(request: Request, next: Next, csp: HeaderValue) -> Response {
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

/// Per-instance HTTP concurrency limiter.
///
/// Bounds the number of in-flight HTTP requests being processed
/// simultaneously. This is defense-in-depth against resource exhaustion —
/// the primary rate limiting should be at the ingress layer (Cloud Run,
/// reverse proxy). Returns 503 Service Unavailable when the limit is
/// reached, shedding load immediately rather than queuing.
async fn http_concurrency_limit(
    semaphore: Arc<tokio::sync::Semaphore>,
    request: Request,
    next: Next,
) -> Response {
    let Ok(_permit) = semaphore.try_acquire() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };

    next.run(request).await
}

// ===========================================================================
// Health handlers
// ===========================================================================

/// Zero-allocation liveness probe. Returns a static JSON body — no
/// `serde_json::json!()` construction, no heap allocation per call.
///
/// Also suitable as a Kubernetes `startupProbe` target: always returns
/// 200, proving the process is alive and listening.
async fn healthz() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"status":"ok"}"#,
    )
}

/// Readiness probe. Returns static JSON bodies for the common ready/
/// not-ready states to avoid per-call allocation.
async fn readyz<S: ServerState>(State(state): State<Arc<S>>) -> impl IntoResponse {
    if state.is_ready() {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"status":"ready"}"#,
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"status":"not_ready","reason":"no reports published yet"}"#,
        )
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::super::config::ServerConfig;
    use super::super::state::{CachedPage, PageUpdateEvent};
    use super::*;
    use arc_swap::ArcSwap;
    use std::collections::HashMap;

    // ── MockServerState for testing ─────────────────────────────

    /// Minimal `ServerState` implementation for testing the server layer
    /// in isolation from any domain-specific state.
    struct MockServerState {
        html_cache: ArcSwap<Option<HashMap<String, CachedPage>>>,
        ws_broadcast: tokio::sync::broadcast::Sender<PageUpdateEvent>,
        /// Test-controllable readiness flag, independent of cache state.
        is_ready_override: std::sync::atomic::AtomicBool,
    }

    impl MockServerState {
        fn new() -> Arc<Self> {
            let (ws_broadcast, _) = tokio::sync::broadcast::channel::<PageUpdateEvent>(64);
            Arc::new(Self {
                html_cache: ArcSwap::from_pointee(None),
                ws_broadcast,
                is_ready_override: std::sync::atomic::AtomicBool::new(false),
            })
        }
    }

    impl ServerState for MockServerState {
        fn html_cache(&self) -> &ArcSwap<Option<HashMap<String, CachedPage>>> {
            &self.html_cache
        }

        fn ws_broadcast(&self) -> &tokio::sync::broadcast::Sender<PageUpdateEvent> {
            &self.ws_broadcast
        }

        fn is_ready(&self) -> bool {
            self.is_ready_override
                .load(std::sync::atomic::Ordering::Acquire)
                || self.html_cache.load().is_some()
        }
    }

    // ── Helper: wait for server to accept connections ────────────

    /// Poll the server with exponential backoff until it accepts a TCP
    /// connection.  Replaces `sleep(50ms)` which is racy under CI load.
    ///
    /// Times out after 5 seconds and panics — long enough for any
    /// reasonable server startup, short enough to fail fast in CI.
    async fn wait_for_server(addr: std::net::SocketAddr) {
        let timeout = std::time::Duration::from_secs(5);
        tokio::time::timeout(timeout, async {
            let mut delay = std::time::Duration::from_millis(1);
            let cap = std::time::Duration::from_secs(1);
            loop {
                if tokio::net::TcpStream::connect(addr).await.is_ok() {
                    return;
                }
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(cap);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("server at {addr} did not become ready within {timeout:?}"));
    }

    // ── Helper: create state with populated cache ────────────────

    fn state_with_cache(pages: &[(&str, &str)]) -> Arc<MockServerState> {
        let state = MockServerState::new();
        let mut map = HashMap::new();
        for (name, body) in pages {
            map.insert(
                (*name).to_string(),
                CachedPage::new(name, body.as_bytes().to_vec()),
            );
        }
        state.html_cache.store(Arc::new(Some(map)));
        state
    }

    fn state_no_cache() -> Arc<MockServerState> {
        MockServerState::new()
    }

    fn default_config() -> ValidatedConfig {
        ServerConfig::builder().build().unwrap()
    }

    // ── normalize_request_path unit tests ───────────────────────

    #[test]
    fn normalize_root_to_index() {
        let result = normalize_request_path("/").unwrap();
        assert_eq!(result.key, "index.html");
        assert!(result.has_trailing_slash);
    }

    #[test]
    fn normalize_simple_path() {
        let result = normalize_request_path("/report.html").unwrap();
        assert_eq!(result.key, "report.html");
        assert!(!result.has_trailing_slash);
    }

    #[test]
    fn normalize_nested_path() {
        let result = normalize_request_path("/owners/acme.html").unwrap();
        assert_eq!(result.key, "owners/acme.html");
        assert!(!result.has_trailing_slash);
    }

    #[test]
    fn normalize_collapses_double_slashes() {
        assert_eq!(
            normalize_request_path("//report.html").unwrap().key,
            "report.html"
        );
        assert_eq!(
            normalize_request_path("/owners///acme.html").unwrap().key,
            "owners/acme.html"
        );
    }

    #[test]
    fn normalize_strips_dot_segments() {
        assert_eq!(
            normalize_request_path("/./report.html").unwrap().key,
            "report.html"
        );
    }

    #[test]
    fn normalize_rejects_dotdot() {
        assert_eq!(normalize_request_path("/../secret.txt"), None);
        assert_eq!(normalize_request_path("/foo/../bar"), None);
        assert_eq!(normalize_request_path("/.."), None);
    }

    #[test]
    fn normalize_rejects_encoded_dotdot() {
        // %2e = '.', %2f = '/'
        assert_eq!(normalize_request_path("/%2e%2e/secret.txt"), None);
        assert_eq!(normalize_request_path("/%2e%2e%2fsecret.txt"), None);
    }

    #[test]
    fn normalize_rejects_null_byte() {
        assert_eq!(normalize_request_path("/foo%00bar"), None);
    }

    #[test]
    fn normalize_rejects_backslash() {
        assert_eq!(normalize_request_path("/foo%5Cbar"), None);
        assert_eq!(normalize_request_path("/foo\\bar"), None);
    }

    #[test]
    fn normalize_double_encoded_is_harmless() {
        // %252e%252e → decodes to literal "%2e%2e" (no dots), safe cache key.
        let result = normalize_request_path("/%252e%252e/secret.txt").unwrap();
        assert!(!result.key.contains(".."));
    }

    #[test]
    fn normalize_empty_path() {
        let result = normalize_request_path("").unwrap();
        assert_eq!(result.key, "index.html");
    }

    #[test]
    fn normalize_rejects_invalid_utf8() {
        // %FF is not valid UTF-8 start byte in isolation.
        assert_eq!(normalize_request_path("/%FF"), None);
    }

    // ── Cache-based serving tests ───────────────────────────────

    #[tokio::test]
    async fn server_serves_cached_pages() {
        let state = state_with_cache(&[("report.html", "<html>test</html>")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/report.html"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "<html>test</html>");

        handle.abort();
    }

    #[tokio::test]
    async fn server_returns_404_for_missing_pages() {
        let state = state_with_cache(&[("index.html", "<html>hi</html>")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/nonexistent.html"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);

        handle.abort();
    }

    #[tokio::test]
    async fn server_returns_503_before_first_collection() {
        let state = state_no_cache();
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/index.html"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 503);

        handle.abort();
    }

    #[tokio::test]
    async fn server_rejects_directory_traversal() {
        let state = state_with_cache(&[("index.html", "<html>ok</html>")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        // Raw traversal.
        let resp = reqwest::get(format!("http://{addr}/../secret.txt"))
            .await
            .unwrap();
        assert_ne!(resp.status(), 200);

        // Encoded traversal.
        let resp = reqwest::get(format!("http://{addr}/%2e%2e/secret.txt"))
            .await
            .unwrap();
        assert_ne!(resp.status(), 200);

        handle.abort();
    }

    #[tokio::test]
    async fn server_serves_index_for_root() {
        let state = state_with_cache(&[
            ("index.html", "<html>dashboard</html>"),
            ("report.html", "<html>report</html>"),
        ]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        // GET / should return index.html.
        let resp = reqwest::get(format!("http://{addr}/")).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = resp.text().await.unwrap();
        assert_eq!(body, "<html>dashboard</html>");

        // GET /report.html still works.
        let resp = reqwest::get(format!("http://{addr}/report.html"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "<html>report</html>");

        handle.abort();
    }

    #[tokio::test]
    async fn server_returns_correct_content_type() {
        let state = state_with_cache(&[
            ("index.html", "<html>hi</html>"),
            ("style.css", "body { color: red; }"),
        ]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/index.html"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "text/html; charset=utf-8");

        let resp = reqwest::get(format!("http://{addr}/style.css"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "text/css; charset=utf-8");

        handle.abort();
    }

    #[tokio::test]
    async fn cache_swap_serves_new_content() {
        let state = state_with_cache(&[("index.html", "<html>v1</html>")]);
        let app = build_router(Arc::clone(&state), &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        // Verify v1.
        let resp = reqwest::get(format!("http://{addr}/")).await.unwrap();
        assert_eq!(resp.text().await.unwrap(), "<html>v1</html>");

        // Swap cache to v2.
        let mut map = HashMap::new();
        map.insert(
            "index.html".to_string(),
            CachedPage::new("index.html", b"<html>v2</html>".to_vec()),
        );
        state.html_cache.store(Arc::new(Some(map)));

        // Verify v2 is served immediately.
        let resp = reqwest::get(format!("http://{addr}/")).await.unwrap();
        assert_eq!(
            resp.text().await.unwrap(),
            "<html>v2</html>",
            "cache swap should serve new content immediately"
        );

        handle.abort();
    }

    // ── Health / readyz / status endpoint tests ────────────────

    #[tokio::test]
    async fn healthz_returns_200_ok() {
        let state = state_no_cache();
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/healthz"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body, serde_json::json!({"status": "ok"}));

        handle.abort();
    }

    #[tokio::test]
    async fn readyz_returns_503_before_cache() {
        let state = state_no_cache();
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/readyz")).await.unwrap();
        assert_eq!(resp.status(), 503);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], "not_ready");

        handle.abort();
    }

    #[tokio::test]
    async fn readyz_returns_200_with_cache_fallback() {
        let state = state_with_cache(&[("index.html", "<html>hi</html>")]);
        let app = build_router(Arc::clone(&state), &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/readyz")).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], "ready");

        handle.abort();
    }

    #[tokio::test]
    async fn readyz_returns_200_after_completed_run() {
        let state = state_no_cache();

        // Simulate readiness without a completed run.
        state
            .is_ready_override
            .store(true, std::sync::atomic::Ordering::Release);

        let app = build_router(Arc::clone(&state), &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/readyz")).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], "ready");

        handle.abort();
    }

    #[tokio::test]
    async fn start_with_graceful_shutdown() {
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<SocketAddr>();
        let shutdown = async {
            shutdown_rx.await.ok();
        };

        let state = state_no_cache();
        let handle = tokio::spawn(async move {
            start(
                0,
                "127.0.0.1",
                shutdown,
                state,
                &default_config(),
                Some(addr_tx),
                None,
            )
            .await
        });

        let addr = addr_rx.await.expect("should receive bound address");
        wait_for_server(addr).await;

        let _ = shutdown_tx.send(());
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }

    // ── Security headers ────────────────────────────────────────

    fn assert_security_headers(resp: &reqwest::Response, endpoint: &str) {
        assert_eq!(
            resp.headers()
                .get("x-frame-options")
                .map(|v| v.to_str().unwrap()),
            Some("DENY"),
            "missing X-Frame-Options on {endpoint}"
        );
        assert_eq!(
            resp.headers()
                .get("x-content-type-options")
                .map(|v| v.to_str().unwrap()),
            Some("nosniff"),
            "missing X-Content-Type-Options on {endpoint}"
        );
        assert!(
            resp.headers()
                .get("content-security-policy")
                .map(|v| v.to_str().unwrap())
                .is_some_and(|v| v.contains("default-src")),
            "missing or invalid CSP on {endpoint}"
        );
        assert_eq!(
            resp.headers()
                .get("referrer-policy")
                .map(|v| v.to_str().unwrap()),
            Some("no-referrer"),
            "missing Referrer-Policy on {endpoint}"
        );
        assert_eq!(
            resp.headers()
                .get("permissions-policy")
                .map(|v| v.to_str().unwrap()),
            Some("camera=(), microphone=(), geolocation=()"),
            "missing Permissions-Policy on {endpoint}"
        );
        assert_eq!(
            resp.headers()
                .get("strict-transport-security")
                .map(|v| v.to_str().unwrap()),
            Some("max-age=63072000; includeSubDomains"),
            "missing or incorrect Strict-Transport-Security on {endpoint}"
        );
    }

    #[tokio::test]
    async fn server_includes_security_headers_on_cached_page() {
        let state = state_with_cache(&[("report.html", "<html>secure</html>")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/report.html"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_security_headers(&resp, "/report.html");

        let csp = resp
            .headers()
            .get("content-security-policy")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(
            csp,
            "default-src 'self'; style-src 'self'; script-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'"
        );
        assert!(!csp.contains("unsafe-inline"));

        handle.abort();
    }

    #[tokio::test]
    async fn healthz_has_security_headers() {
        let state = state_no_cache();
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/healthz"))
            .await
            .unwrap();
        assert_security_headers(&resp, "/healthz");

        handle.abort();
    }

    #[tokio::test]
    async fn readyz_has_security_headers() {
        let state = state_no_cache();
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/readyz")).await.unwrap();
        assert_security_headers(&resp, "/readyz");

        handle.abort();
    }

    // ── ETag / Cache-Control tests ──────────────────────────────

    #[tokio::test]
    async fn cached_page_includes_etag_and_no_cache() {
        let state = state_with_cache(&[("index.html", "<html>hello</html>")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/index.html"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let etag = resp
            .headers()
            .get("etag")
            .expect("response should include ETag header")
            .to_str()
            .unwrap();
        assert!(etag.starts_with("W/\""), "ETag should be weak: {etag}");
        assert!(etag.ends_with('"'), "ETag should end with quote: {etag}");

        let cc = resp
            .headers()
            .get("cache-control")
            .expect("response should include Cache-Control header")
            .to_str()
            .unwrap();
        assert_eq!(cc, "no-cache");

        handle.abort();
    }

    #[tokio::test]
    async fn matching_if_none_match_returns_304() {
        let state = state_with_cache(&[("index.html", "<html>hello</html>")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/index.html"))
            .await
            .unwrap();
        let etag = resp
            .headers()
            .get("etag")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://{addr}/index.html"))
            .header("If-None-Match", &etag)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 304);

        handle.abort();
    }

    #[tokio::test]
    async fn non_matching_if_none_match_returns_200() {
        let state = state_with_cache(&[("index.html", "<html>hello</html>")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://{addr}/index.html"))
            .header("If-None-Match", "W/\"stale-etag\"")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert!(resp.headers().get("etag").is_some());

        handle.abort();
    }

    #[tokio::test]
    async fn etag_304_still_includes_no_cache() {
        let state = state_with_cache(&[("report.html", "<html>report</html>")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/report.html"))
            .await
            .unwrap();
        let etag = resp
            .headers()
            .get("etag")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://{addr}/report.html"))
            .header("If-None-Match", &etag)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 304);

        let cc = resp
            .headers()
            .get("cache-control")
            .expect("304 should include Cache-Control")
            .to_str()
            .unwrap();
        assert_eq!(cc, "no-cache");

        handle.abort();
    }

    #[test]
    fn etag_weak_match_identical() {
        let a = HeaderValue::from_static("W/\"abc123\"");
        let b = HeaderValue::from_static("W/\"abc123\"");
        assert!(etag_weak_match(&a, &b));
    }

    #[test]
    fn etag_weak_match_strips_w_prefix() {
        let client = HeaderValue::from_static("\"abc123\"");
        let server = HeaderValue::from_static("W/\"abc123\"");
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
    fn etag_weak_match_strong_client_weak_server() {
        let client = HeaderValue::from_static("W/\"abc123\"");
        let server = HeaderValue::from_static("\"abc123\"");
        assert!(etag_weak_match(&client, &server));
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

    // ── Encoding negotiation tests ──────────────────────────────

    #[test]
    fn negotiate_prefers_zstd() {
        let hdr = HeaderValue::from_static("gzip, deflate, zstd");
        assert_eq!(negotiate_encoding(&hdr), Encoding::Zstd);
    }

    #[test]
    fn negotiate_identity_when_no_zstd() {
        let hdr = HeaderValue::from_static("gzip, deflate");
        assert_eq!(negotiate_encoding(&hdr), Encoding::Identity);
    }

    #[test]
    fn negotiate_identity_for_unknown() {
        let hdr = HeaderValue::from_static("deflate");
        assert_eq!(negotiate_encoding(&hdr), Encoding::Identity);
    }

    #[test]
    fn negotiate_rejects_q_zero() {
        let hdr = HeaderValue::from_static("zstd;q=0, gzip");
        assert_eq!(negotiate_encoding(&hdr), Encoding::Identity);
    }

    #[test]
    fn negotiate_rejects_q_zero_with_preceding_params() {
        let hdr = HeaderValue::from_static("zstd;level=1;q=0, gzip");
        assert_eq!(negotiate_encoding(&hdr), Encoding::Identity);
    }

    // ── Pre-compression integration tests ───────────────────────

    #[tokio::test]
    async fn compressed_response_has_content_encoding_and_vary() {
        let state = state_with_cache(&[("index.html", "<html>compressed test</html>")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let client = reqwest::Client::builder().no_gzip().build().unwrap();
        let resp = client
            .get(format!("http://{addr}/index.html"))
            .header("Accept-Encoding", "zstd")
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);

        let ce = resp
            .headers()
            .get("content-encoding")
            .expect("should have Content-Encoding")
            .to_str()
            .unwrap();
        assert_eq!(ce, "zstd");

        let vary = resp
            .headers()
            .get("vary")
            .expect("should have Vary")
            .to_str()
            .unwrap();
        assert_eq!(vary, "Accept-Encoding");

        handle.abort();
    }

    #[tokio::test]
    async fn identity_response_for_binary_has_no_content_encoding() {
        let state = state_with_cache(&[("data.bin", "raw binary stuff")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let client = reqwest::Client::builder().no_gzip().build().unwrap();
        let resp = client
            .get(format!("http://{addr}/data.bin"))
            .header("Accept-Encoding", "zstd")
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        assert!(
            resp.headers().get("content-encoding").is_none(),
            "binary content should not have Content-Encoding"
        );
        assert!(
            resp.headers().get("vary").is_none(),
            "binary content should not have Vary"
        );

        handle.abort();
    }

    // ── WebSocket tests ─────────────────────────────────────────

    fn msg_text(msg: tokio_tungstenite::tungstenite::Message) -> String {
        msg.into_text()
            .expect("expected a text WebSocket message")
            .to_string()
    }

    #[tokio::test]
    async fn ws_upgrade_returns_101() {
        use futures_util::StreamExt;

        let state = state_no_cache();
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let url = format!("ws://{addr}/ws");
        let (mut ws, response) = tokio_tungstenite::connect_async(&url).await.unwrap();

        assert_eq!(response.status(), 101);

        let text = msg_text(ws.next().await.unwrap().unwrap());
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["type"], "connected");

        ws.close(None).await.ok();

        handle.abort();
    }

    #[tokio::test]
    async fn ws_receives_broadcast_update() {
        use futures_util::StreamExt;

        let state = state_no_cache();
        let app = build_router(Arc::clone(&state), &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let url = format!("ws://{addr}/ws");
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

        let _ = ws.next().await.unwrap().unwrap();

        state
            .ws_broadcast
            .send(PageUpdateEvent::new(
                vec!["index.html".into(), "report.html".into()],
                "my-repo".into(),
                "2026-04-14T12:00:00Z".into(),
            ))
            .unwrap();

        let text = msg_text(ws.next().await.unwrap().unwrap());
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["type"], "update");
        assert_eq!(parsed["repo"], "my-repo");
        assert_eq!(parsed["pages"][0], "index.html");
        assert_eq!(parsed["pages"][1], "report.html");
        assert_eq!(parsed["timestamp"], "2026-04-14T12:00:00Z");

        ws.close(None).await.ok();

        handle.abort();
    }

    #[tokio::test]
    async fn ws_sends_reload_on_lag() {
        use futures_util::StreamExt;

        let state = MockServerState::new();
        let app = build_router(Arc::clone(&state), &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let url = format!("ws://{addr}/ws");
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

        let _ = ws.next().await.unwrap().unwrap();

        for i in 0..70 {
            state
                .ws_broadcast
                .send(PageUpdateEvent::new(
                    vec![format!("page-{i}.html")],
                    format!("repo-{i}"),
                    "2026-04-14T12:00:00Z".into(),
                ))
                .ok();
        }

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let mut saw_reload = false;
        let timeout = tokio::time::sleep(std::time::Duration::from_secs(3));
        tokio::pin!(timeout);
        loop {
            tokio::select! {
                msg = ws.next() => {
                    match msg {
                        Some(Ok(m)) => {
                            let text = msg_text(m);
                            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text)
                                && parsed["type"] == "reload" {
                                saw_reload = true;
                                break;
                            }
                        }
                        _ => break,
                    }
                }
                () = &mut timeout => break,
            }
        }

        assert!(
            saw_reload,
            "should have received a reload message after broadcast overflow"
        );

        ws.close(None).await.ok();

        handle.abort();
    }

    #[tokio::test]
    async fn non_ws_get_to_ws_path_returns_error() {
        let state = state_no_cache();
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/ws")).await.unwrap();
        assert!(
            resp.status().is_client_error(),
            "non-upgrade GET to /ws should be a client error, got {}",
            resp.status()
        );

        handle.abort();
    }

    #[tokio::test]
    async fn ws_endpoint_has_security_headers() {
        let state = state_no_cache();
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/ws")).await.unwrap();
        assert_security_headers(&resp, "/ws (non-upgrade)");

        handle.abort();
    }

    // ── WebSocket semaphore tests ───────────────────────────────

    #[tokio::test]
    async fn ws_semaphore_exhaustion_returns_503() {
        use futures_util::StreamExt;

        let state = MockServerState::new();
        let config = ServerConfig::builder()
            .ws_max_connections(2)
            .build()
            .unwrap();
        let app = build_router(Arc::clone(&state), &config, None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let url = format!("ws://{addr}/ws");

        let (mut ws1, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let _ = ws1.next().await;
        let (mut ws2, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let _ = ws2.next().await;

        let result = tokio_tungstenite::connect_async(&url).await;
        match result {
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
                assert_eq!(
                    resp.status(),
                    503,
                    "3rd WebSocket connection should be rejected with 503"
                );
            }
            Err(other) => panic!("expected HTTP 503 error, got: {other}"),
            Ok(_) => panic!("3rd connection should have been rejected"),
        }

        ws1.close(None).await.ok();
        ws2.close(None).await.ok();
        handle.abort();
    }

    #[tokio::test]
    async fn ws_semaphore_permit_released_on_disconnect() {
        use futures_util::StreamExt;

        let state = MockServerState::new();
        let config = ServerConfig::builder()
            .ws_max_connections(1)
            .build()
            .unwrap();
        let app = build_router(Arc::clone(&state), &config, None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let url = format!("ws://{addr}/ws");

        let (mut ws1, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let _ = ws1.next().await;

        let result = tokio_tungstenite::connect_async(&url).await;
        assert!(
            matches!(
                &result,
                Err(tokio_tungstenite::tungstenite::Error::Http(r)) if r.status() == 503
            ),
            "2nd connection should be rejected with 503, got: {result:?}"
        );

        ws1.close(None).await.ok();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let (mut ws2, resp2) = tokio_tungstenite::connect_async(&url).await.unwrap();
        assert_eq!(resp2.status(), 101);
        let text = msg_text(ws2.next().await.unwrap().unwrap());
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["type"], "connected");

        ws2.close(None).await.ok();
        handle.abort();
    }

    // ── WebSocket multi-client fanout test ───────────────────────

    #[tokio::test]
    async fn ws_broadcast_reaches_all_connected_clients() {
        use futures_util::StreamExt;

        let state = state_no_cache();
        let app = build_router(Arc::clone(&state), &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let url = format!("ws://{addr}/ws");

        let (mut ws1, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let _ = ws1.next().await;
        let (mut ws2, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let _ = ws2.next().await;
        let (mut ws3, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let _ = ws3.next().await;

        state
            .ws_broadcast
            .send(PageUpdateEvent::new(
                vec!["index.html".into()],
                "fanout-repo".into(),
                "2026-04-15T12:00:00Z".into(),
            ))
            .unwrap();

        for (i, ws) in [&mut ws1, &mut ws2, &mut ws3].iter_mut().enumerate() {
            let text = msg_text(ws.next().await.unwrap().unwrap());
            let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(parsed["type"], "update", "client {i} should get update");
            assert_eq!(parsed["repo"], "fanout-repo", "client {i} repo mismatch");
            assert_eq!(
                parsed["pages"][0], "index.html",
                "client {i} pages mismatch"
            );
        }

        ws1.close(None).await.ok();
        ws2.close(None).await.ok();
        ws3.close(None).await.ok();
        handle.abort();
    }

    // ── WebSocket graceful shutdown test ─────────────────────────

    #[tokio::test]
    async fn ws_session_ends_on_broadcast_close() {
        use futures_util::StreamExt;
        use tokio::net::TcpListener as TokioTcpListener;

        let state = state_no_cache();
        let app = build_router(Arc::clone(&state), &default_config(), None);

        let listener = TokioTcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let url = format!("ws://{addr}/ws");
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let _ = ws.next().await;

        drop(state);

        handle.abort();
        let _ = handle.await;

        ws.close(None).await.ok();

        let timeout = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while let Some(_msg) = ws.next().await {}
        })
        .await;

        assert!(
            timeout.is_ok(),
            "WebSocket stream should drain after close + server abort"
        );
    }

    // ── WebSocket /ws.js content-type integration test ──────────

    #[tokio::test]
    async fn ws_js_has_correct_content_type_and_zstd() {
        let js_body = "(function(){})();";
        let state = state_with_cache(&[("ws.js", js_body)]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let client = reqwest::Client::builder().no_gzip().build().unwrap();
        let resp = client
            .get(format!("http://{addr}/ws.js"))
            .header("Accept-Encoding", "zstd")
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);

        let ct = resp
            .headers()
            .get("content-type")
            .expect("ws.js should have Content-Type")
            .to_str()
            .unwrap();
        assert_eq!(ct, "text/javascript; charset=utf-8");

        let ce = resp
            .headers()
            .get("content-encoding")
            .expect("ws.js should have Content-Encoding: zstd")
            .to_str()
            .unwrap();
        assert_eq!(ce, "zstd");

        handle.abort();
    }

    // ── WebSocket max message size test ──────────────────────────

    #[tokio::test]
    async fn ws_rejects_oversized_client_message() {
        use futures_util::{SinkExt, StreamExt};

        let state = state_no_cache();
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let url = format!("ws://{addr}/ws");

        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let _ = ws.next().await;

        let oversized = "x".repeat(8192);
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            oversized.into(),
        ))
        .await
        .ok();

        let timeout_result = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                match ws.next().await {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_)) | Err(_)) | None => {
                        return true;
                    }
                    _ => {}
                }
            }
        })
        .await;

        assert!(
            timeout_result.is_ok(),
            "server should close connection after oversized message"
        );

        handle.abort();
    }

    // ── Origin validation unit tests ────────────────────────────

    #[test]
    fn origin_validation_same_origin_matches() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(validate_ws_origin(&headers));
    }

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
    fn origin_validation_cross_origin_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_static("https://evil.com"));
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(!validate_ws_origin(&headers));
    }

    #[test]
    fn origin_validation_no_origin_header_allowed() {
        let mut headers = HeaderMap::new();
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
    fn origin_validation_file_scheme_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("file://example.com"),
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

    #[test]
    fn origin_validation_http_default_port_stripped() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://example.com:80"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(validate_ws_origin(&headers));
    }

    // ── WebSocket CSWSH integration test ────────────────────────

    #[tokio::test]
    async fn ws_cross_origin_upgrade_rejected() {
        let state = state_no_cache();
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let url = format!("ws://{addr}/ws");
        let request = tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(&url)
            .header("Host", format!("{addr}"))
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

        handle.abort();
    }

    // ── HTTP method filtering tests ─────────────────────────────

    #[tokio::test]
    async fn post_to_cached_page_returns_405() {
        let state = state_with_cache(&[("index.html", "<html>hi</html>")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/index.html"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 405);

        let allow = resp
            .headers()
            .get("allow")
            .expect("405 should include Allow header")
            .to_str()
            .unwrap();
        assert_eq!(allow, "GET, HEAD");

        handle.abort();
    }

    #[tokio::test]
    async fn put_to_cached_page_returns_405() {
        let state = state_with_cache(&[("index.html", "<html>hi</html>")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let client = reqwest::Client::new();
        let resp = client
            .put(format!("http://{addr}/index.html"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 405);

        handle.abort();
    }

    #[tokio::test]
    async fn delete_to_cached_page_returns_405() {
        let state = state_with_cache(&[("index.html", "<html>hi</html>")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let client = reqwest::Client::new();
        let resp = client
            .delete(format!("http://{addr}/index.html"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 405);

        handle.abort();
    }

    #[tokio::test]
    async fn head_to_cached_page_returns_200() {
        let state = state_with_cache(&[("index.html", "<html>hi</html>")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let client = reqwest::Client::new();
        let resp = client
            .head(format!("http://{addr}/index.html"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        handle.abort();
    }

    #[tokio::test]
    async fn method_not_allowed_has_security_headers() {
        let state = state_with_cache(&[("index.html", "<html>hi</html>")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/index.html"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 405);
        assert_security_headers(&resp, "POST /index.html");

        handle.abort();
    }

    #[tokio::test]
    async fn options_to_cached_page_returns_405() {
        let state = state_with_cache(&[("index.html", "<html>hi</html>")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let client = reqwest::Client::new();
        let resp = client
            .request(
                reqwest::Method::OPTIONS,
                format!("http://{addr}/index.html"),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 405);

        handle.abort();
    }

    #[tokio::test]
    async fn patch_to_cached_page_returns_405() {
        let state = state_with_cache(&[("index.html", "<html>hi</html>")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let client = reqwest::Client::new();
        let resp = client
            .patch(format!("http://{addr}/index.html"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 405);

        handle.abort();
    }

    // ── NormalizedPath trailing slash tests ──────────────────────

    #[test]
    fn normalized_path_trailing_slash_about() {
        let result = normalize_request_path("/about/").unwrap();
        assert_eq!(result.key, "about");
        assert!(result.has_trailing_slash);
    }

    #[test]
    fn normalized_path_no_trailing_slash_about() {
        let result = normalize_request_path("/about").unwrap();
        assert_eq!(result.key, "about");
        assert!(!result.has_trailing_slash);
    }

    // ── Fallback resolution chain unit tests ────────────────────

    #[test]
    fn resolve_direct_match() {
        let mut cache = HashMap::new();
        cache.insert(
            "about.html".to_string(),
            CachedPage::new("about.html", b"<html>about</html>".to_vec()),
        );
        assert!(resolve_cache_key(&cache, "about.html", false).is_some());
    }

    #[test]
    fn resolve_directory_index_with_trailing_slash() {
        let mut cache = HashMap::new();
        cache.insert(
            "about/index.html".to_string(),
            CachedPage::new("about/index.html", b"<html>about</html>".to_vec()),
        );
        assert!(resolve_cache_key(&cache, "about", true).is_some());
    }

    #[test]
    fn resolve_directory_index_without_trailing_slash_no_ext() {
        let mut cache = HashMap::new();
        cache.insert(
            "about/index.html".to_string(),
            CachedPage::new("about/index.html", b"<html>about</html>".to_vec()),
        );
        // No trailing slash, no extension → tries both about/index.html and about.html
        assert!(resolve_cache_key(&cache, "about", false).is_some());
    }

    #[test]
    fn resolve_clean_url_html_fallback() {
        let mut cache = HashMap::new();
        cache.insert(
            "about.html".to_string(),
            CachedPage::new("about.html", b"<html>about</html>".to_vec()),
        );
        // No trailing slash, no extension, no about/index.html → tries about.html
        assert!(resolve_cache_key(&cache, "about", false).is_some());
    }

    #[test]
    fn resolve_no_self_loop_on_index_html() {
        let mut cache = HashMap::new();
        cache.insert(
            "index.html".to_string(),
            CachedPage::new("index.html", b"<html>root</html>".to_vec()),
        );
        // Should find index.html directly, not try index.html/index.html
        assert!(resolve_cache_key(&cache, "index.html", false).is_some());
    }

    #[test]
    fn resolve_no_self_loop_nested_index() {
        let mut cache = HashMap::new();
        cache.insert(
            "blog/index.html".to_string(),
            CachedPage::new("blog/index.html", b"<html>blog</html>".to_vec()),
        );
        // Direct match — should not try blog/index.html/index.html
        assert!(resolve_cache_key(&cache, "blog/index.html", false).is_some());
    }

    #[test]
    fn resolve_miss_returns_none() {
        let cache = HashMap::new();
        assert!(resolve_cache_key(&cache, "nonexistent", false).is_none());
    }

    // ── Fallback resolution integration tests ───────────────────

    #[tokio::test]
    async fn get_about_serves_about_index_html() {
        let state = state_with_cache(&[
            ("about/index.html", "<html>about page</html>"),
            ("index.html", "<html>root</html>"),
        ]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/about")).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "<html>about page</html>");

        handle.abort();
    }

    #[tokio::test]
    async fn get_about_trailing_slash_serves_about_index_html() {
        let state = state_with_cache(&[
            ("about/index.html", "<html>about page</html>"),
            ("index.html", "<html>root</html>"),
        ]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/about/")).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "<html>about page</html>");

        handle.abort();
    }

    #[tokio::test]
    async fn get_about_serves_about_html_when_no_index() {
        let state = state_with_cache(&[
            ("about.html", "<html>about clean url</html>"),
            ("index.html", "<html>root</html>"),
        ]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/about")).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "<html>about clean url</html>");

        handle.abort();
    }

    // ── Custom error page tests ─────────────────────────────────

    #[tokio::test]
    async fn custom_404_page_served_on_miss() {
        let state = state_with_cache(&[
            ("index.html", "<html>root</html>"),
            ("404.html", "<html>custom not found</html>"),
        ]);
        let config = ServerConfig::builder()
            .error_page_key("404.html")
            .build()
            .unwrap();
        let app = build_router(state, &config, None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/nonexistent"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "text/html; charset=utf-8");
        assert_eq!(resp.text().await.unwrap(), "<html>custom not found</html>");

        handle.abort();
    }

    #[tokio::test]
    async fn custom_404_page_suppresses_304() {
        let state = state_with_cache(&[
            ("index.html", "<html>root</html>"),
            ("404.html", "<html>custom not found</html>"),
        ]);
        let config = ServerConfig::builder()
            .error_page_key("404.html")
            .build()
            .unwrap();
        let app = build_router(state, &config, None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        wait_for_server(addr).await;

        // First request to get the ETag of the error page.
        let resp = reqwest::get(format!("http://{addr}/nonexistent"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
        let etag = resp
            .headers()
            .get("etag")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // Second request with If-None-Match — should still get 404, not 304.
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://{addr}/also-nonexistent"))
            .header("If-None-Match", &etag)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 404, "error page should suppress 304");

        handle.abort();
    }

    // ── SVG XSS mitigation tests ────────────────────────────────

    #[tokio::test]
    async fn svg_response_has_restrictive_csp() {
        let svg_body = r#"<svg xmlns="http://www.w3.org/2000/svg"><circle r="10"/></svg>"#;
        let state = state_with_cache(&[("logo.svg", svg_body)]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/logo.svg"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let csp = resp
            .headers()
            .get("content-security-policy")
            .unwrap()
            .to_str()
            .unwrap();
        // Verify exact SVG CSP — must NOT be the global DEFAULT_CSP.
        assert_eq!(
            csp, SVG_CSP,
            "SVG should use restrictive SVG_CSP, not global DEFAULT_CSP"
        );

        handle.abort();
    }

    // ── Extra routes inheritance tests (PUBLISH BLOCKER) ────────

    #[tokio::test]
    async fn extra_routes_inherit_security_headers() {
        use axum::routing::get as get_route;

        let state = state_with_cache(&[("index.html", "<html>ok</html>")]);
        let extra = Router::new().route("/custom", get_route(|| async { "custom response" }));
        let app = build_router(state, &default_config(), Some(extra));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/custom")).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_security_headers(&resp, "/custom (extra route)");

        handle.abort();
    }

    #[tokio::test]
    async fn extra_routes_inherit_concurrency_limit() {
        use axum::routing::get as get_route;

        let state = state_with_cache(&[("index.html", "<html>ok</html>")]);
        let config = ServerConfig::builder()
            .concurrency_limit(1)
            .build()
            .unwrap();
        // Extra route that holds the semaphore for 500ms.
        let extra = Router::new().route(
            "/slow",
            get_route(|| async {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                "slow"
            }),
        );
        let app = build_router(Arc::clone(&state), &config, Some(extra));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        wait_for_server(addr).await;

        // First request occupies the single permit.
        let client = reqwest::Client::new();
        let slow_handle = tokio::spawn({
            let client = client.clone();
            let url = format!("http://{addr}/slow");
            async move { client.get(&url).send().await.unwrap() }
        });

        // Give the slow request time to acquire the permit.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Second request should be rejected with 503.
        let resp = client
            .get(format!("http://{addr}/index.html"))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            503,
            "concurrent request should get 503 when concurrency_limit=1"
        );

        // Clean up.
        let slow_resp = slow_handle.await.unwrap();
        assert_eq!(slow_resp.status(), 200);
        handle.abort();
    }

    #[tokio::test]
    async fn extra_routes_builtin_routes_keep_body_limit() {
        use axum::routing::post as post_route;

        let state = state_with_cache(&[("index.html", "<html>ok</html>")]);
        let config = ServerConfig::builder()
            .max_request_body_bytes(1024)
            .build()
            .unwrap();

        // Extra route with a larger body limit (1 MB).
        let extra = Router::new()
            .route(
                "/upload",
                post_route(|body: axum::body::Bytes| async move {
                    format!("received {} bytes", body.len())
                }),
            )
            .layer(RequestBodyLimitLayer::new(1024 * 1024));

        let app = build_router(Arc::clone(&state), &config, Some(extra));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        wait_for_server(addr).await;

        let large_body = vec![b'x'; 2048]; // 2KB > 1KB limit

        // Built-in route should reject large body.
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/index.html"))
            .body(large_body.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            413,
            "built-in route should reject body > max_request_body_bytes"
        );

        // Extra route should accept large body (its own 1MB limit).
        let resp = client
            .post(format!("http://{addr}/upload"))
            .body(large_body)
            .send()
            .await
            .unwrap();
        assert!(
            resp.status() != 413,
            "extra route should accept body within its own limit, got {}",
            resp.status()
        );

        handle.abort();
    }

    #[test]
    #[should_panic(expected = "Overlapping method route")]
    fn extra_routes_shadowing_panics() {
        use axum::routing::get as get_route;

        let state = state_with_cache(&[("index.html", "<html>ok</html>")]);
        let extra = Router::new().route("/healthz", get_route(|| async { "shadowed" }));
        // This should panic because /healthz conflicts with the built-in route.
        let _app = build_router(state, &default_config(), Some(extra));
    }

    // ── Concurrency limit real shedding test ────────────────────

    #[tokio::test]
    async fn concurrency_limit_sheds_load_real() {
        use axum::routing::get as get_route;

        let state = state_with_cache(&[("index.html", "<html>ok</html>")]);
        let config = ServerConfig::builder()
            .concurrency_limit(1)
            .build()
            .unwrap();
        // Extra route that holds the semaphore.
        let extra = Router::new().route(
            "/hold",
            get_route(|| async {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                "held"
            }),
        );
        let app = build_router(Arc::clone(&state), &config, Some(extra));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        wait_for_server(addr).await;

        let client = reqwest::Client::new();

        // Hold a request to occupy the single permit.
        let hold_handle = tokio::spawn({
            let client = client.clone();
            let url = format!("http://{addr}/hold");
            async move { client.get(&url).send().await.unwrap() }
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Concurrent requests should get 503.
        let futures: Vec<_> = (0..3)
            .map(|_| {
                let client = client.clone();
                let url = format!("http://{addr}/index.html");
                tokio::spawn(async move { client.get(&url).send().await.unwrap() })
            })
            .collect();

        let mut got_503 = false;
        for f in futures {
            let resp = f.await.unwrap();
            if resp.status() == 503 {
                got_503 = true;
            }
        }
        assert!(got_503, "at least one concurrent request should get 503");

        let hold_resp = hold_handle.await.unwrap();
        assert_eq!(hold_resp.status(), 200);
        handle.abort();
    }

    // ── If-None-Match multi-value (known limitation) ────────────

    #[tokio::test]
    async fn if_none_match_multi_value_returns_200() {
        // RFC 7232 §3.2 allows multiple ETags in If-None-Match.
        // Our implementation compares only the full header value
        // (single-value), so multi-value always returns 200.
        // This is a documented known limitation.
        let state = state_with_cache(&[("index.html", "<html>etag test</html>")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        wait_for_server(addr).await;

        // First request to get the ETag.
        let resp = reqwest::get(format!("http://{addr}/index.html"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let etag = resp
            .headers()
            .get("etag")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // Send multi-value If-None-Match: W/"old", <actual-etag>
        let client = reqwest::Client::new();
        let multi_value = format!(r#"W/"old", {etag}"#);
        let resp = client
            .get(format!("http://{addr}/index.html"))
            .header("if-none-match", &multi_value)
            .send()
            .await
            .unwrap();
        // Known limitation: multi-value comparison not supported → 200.
        assert_eq!(
            resp.status(),
            200,
            "multi-value If-None-Match should return 200 (known limitation)"
        );

        handle.abort();
    }

    // ── MIME type integration tests ─────────────────────────────

    #[tokio::test]
    async fn wasm_has_correct_content_type() {
        let state = state_with_cache(&[("app.wasm", "fake wasm")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/app.wasm"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "application/wasm");

        handle.abort();
    }

    #[tokio::test]
    async fn style_css_still_works_directly() {
        let state = state_with_cache(&[("style.css", "body { margin: 0; }")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/style.css"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "text/css; charset=utf-8");

        handle.abort();
    }

    #[tokio::test]
    async fn oversized_body_returns_413() {
        let state = state_with_cache(&[("index.html", "<html>hi</html>")]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let oversized_body = "x".repeat(2048);
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/index.html"))
            .body(oversized_body)
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            413,
            "oversized body should return 413 Payload Too Large"
        );

        handle.abort();
    }

    // ── is_ready four-combination matrix ────────────────────────

    #[test]
    fn is_ready_neither_cache_nor_override() {
        let state = MockServerState::new();
        assert!(!state.is_ready());
    }

    #[test]
    fn is_ready_cache_only() {
        let state = MockServerState::new();
        let mut map = HashMap::new();
        map.insert(
            "index.html".to_string(),
            CachedPage::new("index.html", b"<html>hi</html>".to_vec()),
        );
        state.html_cache.store(Arc::new(Some(map)));
        assert!(state.is_ready());
    }

    #[test]
    fn is_ready_override_only() {
        let state = MockServerState::new();
        state
            .is_ready_override
            .store(true, std::sync::atomic::Ordering::Release);
        assert!(state.is_ready());
    }

    #[test]
    fn is_ready_both() {
        let state = MockServerState::new();
        let mut map = HashMap::new();
        map.insert(
            "index.html".to_string(),
            CachedPage::new("index.html", b"<html>hi</html>".to_vec()),
        );
        state.html_cache.store(Arc::new(Some(map)));
        state
            .is_ready_override
            .store(true, std::sync::atomic::Ordering::Release);
        assert!(state.is_ready());
    }

    // ── CSP override tests ──────────────────────────────────────

    #[tokio::test]
    async fn custom_csp_override_appears_in_response() {
        let state = state_with_cache(&[("index.html", "<html>csp</html>")]);
        let config = ServerConfig::builder()
            .csp_override("default-src 'self' 'unsafe-inline'")
            .build()
            .unwrap();
        let app = build_router(state, &config, None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/index.html"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let csp = resp
            .headers()
            .get("content-security-policy")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(csp, "default-src 'self' 'unsafe-inline'");

        handle.abort();
    }

    #[tokio::test]
    async fn default_csp_preserved_when_no_override() {
        let state = state_with_cache(&[("index.html", "<html>csp</html>")]);
        let config = default_config();
        let app = build_router(state, &config, None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/index.html"))
            .await
            .unwrap();
        let csp = resp
            .headers()
            .get("content-security-policy")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(csp, super::DEFAULT_CSP);

        handle.abort();
    }

    // ── start() error handling tests ────────────────────────────

    #[tokio::test]
    async fn start_with_invalid_bind_address() {
        let state = state_no_cache();
        let shutdown = async {};
        let result = start(
            0,
            "999.999.999.999",
            shutdown,
            state,
            &default_config(),
            None,
            None,
        )
        .await;
        assert!(
            matches!(
                result,
                Err(crate::infra::server::error::ServerError::InvalidAddress { .. })
            ),
            "invalid bind address should return InvalidAddress, got: {result:?}"
        );
    }

    // ── Config-driven ws_max_connections test ────────────────────

    #[tokio::test]
    async fn ws_max_connections_config_honored() {
        use futures_util::StreamExt;

        let state = MockServerState::new();
        let config = ServerConfig::builder()
            .ws_max_connections(2)
            .build()
            .unwrap();
        let app = build_router(Arc::clone(&state), &config, None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let url = format!("ws://{addr}/ws");

        let (mut ws1, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let _ = ws1.next().await;
        let (mut ws2, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let _ = ws2.next().await;

        // Third connection should be rejected.
        let result = tokio_tungstenite::connect_async(&url).await;
        match result {
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
                assert_eq!(resp.status(), 503, "3rd WS conn should get 503");
            }
            Err(other) => panic!("expected HTTP 503, got: {other}"),
            Ok(_) => panic!("3rd connection should have been rejected"),
        }

        ws1.close(None).await.ok();
        ws2.close(None).await.ok();
        handle.abort();
    }

    // ── Concurrency limit test ──────────────────────────────────

    #[tokio::test]
    async fn concurrency_limit_sheds_load() {
        // Verify the concurrency semaphore is created from config by checking
        // that a config with limit=1 still serves requests (basic sanity) and
        // that the router doesn't panic on construction.
        let state = state_with_cache(&[("index.html", "<html>ok</html>")]);
        let config = ServerConfig::builder()
            .concurrency_limit(1)
            .build()
            .unwrap();
        let app = build_router(state, &config, None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        // Sequential request succeeds with concurrency_limit=1.
        let resp = reqwest::get(format!("http://{addr}/index.html"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        handle.abort();
    }

    // ── HEAD response tests ─────────────────────────────────────

    #[tokio::test]
    async fn head_returns_empty_body_with_content_length() {
        let body_content = "<html>hello world</html>";
        let state = state_with_cache(&[("index.html", body_content)]);
        let app = build_router(state, &default_config(), None);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let client = reqwest::Client::builder().no_gzip().build().unwrap();
        let resp = client
            .head(format!("http://{addr}/index.html"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        // HEAD response body should be empty.
        let resp_body = resp.bytes().await.unwrap();
        assert!(
            resp_body.is_empty(),
            "HEAD response body should be empty, got {} bytes",
            resp_body.len()
        );

        handle.abort();
    }

    // ── normalize_authority IPv6 tests (Finding 9.1) ────────────

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

    // ── has_extension edge cases (Finding 9.3) ──────────────────

    #[test]
    fn has_extension_dotted_directory_no_ext() {
        // "v2.0/about" — dot is in the directory part, not the filename.
        assert!(!has_extension("v2.0/about"));
    }

    #[test]
    fn has_extension_trailing_dot() {
        // "file." has a dot in the last segment.
        assert!(has_extension("file."));
    }

    #[test]
    fn has_extension_hidden_file() {
        // ".hidden" has a dot in the last segment.
        assert!(has_extension(".hidden"));
    }

    #[test]
    fn has_extension_no_dot() {
        assert!(!has_extension("about"));
    }

    #[test]
    fn has_extension_normal_file() {
        assert!(has_extension("style.css"));
    }

    // ── etag_weak_match wildcard (Finding 3.10) ─────────────────

    #[test]
    fn etag_weak_match_wildcard() {
        let client = HeaderValue::from_static("*");
        let server = HeaderValue::from_static("W/\"abc123\"");
        assert!(etag_weak_match(&client, &server));
    }

    // ── Proptest: normalize_request_path ─────────────────────────

    mod proptest_path {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// normalize_request_path never panics on arbitrary Unicode input.
            #[test]
            fn never_panics(input in "\\PC{0,500}") {
                let _ = normalize_request_path(&input);
            }

            /// If normalize_request_path returns Some, the key never contains
            /// path traversal sequences, null bytes, or backslashes.
            #[test]
            fn output_key_never_contains_dangerous_sequences(input in "\\PC{0,500}") {
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

            /// If the input percent-decodes to contain "..", it must be rejected.
            #[test]
            fn rejects_traversal_after_decode(
                prefix in "[a-z]{0,5}",
                suffix in "[a-z]{0,5}",
            ) {
                let input = format!("/{prefix}/../{suffix}");
                prop_assert!(normalize_request_path(&input).is_none());
            }

            /// Output key never starts with a slash.
            #[test]
            fn output_key_never_starts_with_slash(input in "\\PC{0,500}") {
                if let Some(result) = normalize_request_path(&input) {
                    prop_assert!(
                        !result.key.starts_with('/'),
                        "key starts with '/': {:?}", result.key
                    );
                }
            }
        }
    }

    // ── Proptest: validate_ws_origin ─────────────────────────────

    mod proptest_origin {
        use super::*;
        use proptest::prelude::*;

        /// Strategy: random Origin and Host header combinations.
        fn origin_host_strategy() -> impl Strategy<Value = (Option<String>, Option<String>)> {
            let origin = proptest::option::of("[a-z]{3,8}://[a-z0-9.:\\[\\]]{1,30}(/[a-z]{0,10})?");
            let host = proptest::option::of("[a-z0-9.:\\[\\]]{1,30}");
            (origin, host)
        }

        proptest! {
            /// validate_ws_origin never panics on arbitrary header combinations.
            #[test]
            fn never_panics((origin, host) in origin_host_strategy()) {
                let mut headers = HeaderMap::new();
                if let Some(ref o) = origin
                    && let Ok(v) = HeaderValue::from_str(o)
                {
                    headers.insert(header::ORIGIN, v);
                }
                if let Some(ref h) = host
                    && let Ok(v) = HeaderValue::from_str(h)
                {
                    headers.insert(header::HOST, v);
                }
                let _ = validate_ws_origin(&headers);
            }

            /// If no Origin header, validate_ws_origin returns true.
            #[test]
            fn no_origin_always_true(host in "[a-z0-9.]{1,20}") {
                let mut headers = HeaderMap::new();
                if let Ok(v) = HeaderValue::from_str(&host) {
                    headers.insert(header::HOST, v);
                }
                prop_assert!(validate_ws_origin(&headers));
            }

            /// Cross-origin requests are rejected: Origin host != Host header.
            #[test]
            fn cross_origin_rejected(
                origin_host in "[a-z]{3,8}\\.[a-z]{2,4}",
                host in "[a-z]{3,8}\\.[a-z]{2,4}",
            ) {
                // Only test when hosts are genuinely different.
                prop_assume!(origin_host != host);
                let origin = format!("https://{origin_host}");
                let mut headers = HeaderMap::new();
                headers.insert(header::ORIGIN, HeaderValue::from_str(&origin).unwrap());
                headers.insert(header::HOST, HeaderValue::from_str(&host).unwrap());
                prop_assert!(!validate_ws_origin(&headers));
            }
        }
    }
}
