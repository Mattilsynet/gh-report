//! HTTP integration tests for the projection adapter (m5 Phase 4c).
//!
//! Ported from the donor crate's `server` module per the donor audit at
//! `.ooda/preflight-4c-donor-audit-1778536369.md` (bd `adr-fmt-2z86`).
//! Each test drives a real axum server bound to `127.0.0.1:0` via the
//! `spawn_test_server_secured` substrate so the full security-header
//! stack — donor's `assert_security_headers` set — is composed onto the
//! base `build_projection_router`. No production-code edits; all
//! deviations from donor live in the substrate or in this file's
//! per-test path-prefix rewrites.
//!
//! Profile A+ (audit + feynman orientation):
//! - 35 drop-in tests from the audit § "Portable to 4c" table.
//! - 5 `negotiate_*` tests reframed as `Accept-Encoding` →
//!   `Content-Encoding` round-trips through `build_projection_router`
//!   (the donor exercises a private helper; dest has no `negotiate`
//!   symbol, so the behaviour is exercised through observable HTTP).
//!
//! Path-prefix shift: every donor `/<key>` becomes `/v1/<key>`
//! (CHE-0049 R9; BC2 verified at `handlers.rs:489-492`). Health probes
//! shift `/healthz` → `/v1/healthz`, `/readyz` → `/v1/readyz`.
//!
//! Response-body shift: dest health/readyz bodies carry `"v": 1`
//! (CHE-0049 R13) — `{"v":1,"status":"ok"}`, not the donor's
//! `{"status":"ok"}`. Body asserts adjusted accordingly.

#![cfg(feature = "projection")]

mod common;

use common::{
    MockProjectionSource, assert_etag_yields_304, assert_security_headers, decode_zstd, http_get,
    http_get_with_headers, http_request, mk_snapshot, spawn_test_server_secured,
};

// ===========================================================================
// Basic serving + 404 / 503 / traversal
// ===========================================================================

#[tokio::test]
async fn server_serves_cached_pages() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "report.html",
        "report.html",
        "<html>test</html>",
    )])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_get(s.addr, "/v1/report.html").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "<html>test</html>");
    s.shutdown().await;
}

#[tokio::test]
async fn server_returns_404_for_missing_pages() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>hi</html>",
    )])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_get(s.addr, "/v1/nonexistent.html").await;
    assert_eq!(resp.status(), 404);
    s.shutdown().await;
}

#[tokio::test]
async fn server_returns_503_before_first_collection() {
    let source = MockProjectionSource::new();
    // No snapshot installed — `snapshot()` returns None → 503.
    let s = spawn_test_server_secured(source).await;
    let resp = http_get(s.addr, "/v1/index.html").await;
    assert_eq!(resp.status(), 503);
    s.shutdown().await;
}

#[tokio::test]
async fn server_rejects_directory_traversal() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>ok</html>",
    )])));
    let s = spawn_test_server_secured(source).await;

    // Raw traversal — axum's path resolver rejects.
    let resp = http_get(s.addr, "/v1/../secret.txt").await;
    assert_ne!(resp.status(), 200);

    // Percent-encoded traversal — also rejected by the wildcard route.
    let resp = http_get(s.addr, "/v1/%2e%2e/secret.txt").await;
    assert_ne!(resp.status(), 200);

    s.shutdown().await;
}

#[tokio::test]
async fn server_serves_index_for_root() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[
        ("index.html", "index.html", "<html>dashboard</html>"),
        ("report.html", "report.html", "<html>report</html>"),
    ])));
    let s = spawn_test_server_secured(source).await;

    // Dest divergence (CHE-0049 R9): no root or `/v1/` route — `/v1/{*path}`
    // requires ≥1 segment. The donor's "serve index at root" reframes to
    // "serve index at /v1/index.html".
    let resp = http_get(s.addr, "/v1/index.html").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "<html>dashboard</html>");

    let resp = http_get(s.addr, "/v1/report.html").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "<html>report</html>");

    s.shutdown().await;
}

#[tokio::test]
async fn server_returns_correct_content_type() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[
        ("index.html", "index.html", "<html>hi</html>"),
        ("style.css", "style.css", "body { color: red; }"),
    ])));
    let s = spawn_test_server_secured(source).await;

    let resp = http_get(s.addr, "/v1/index.html").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "text/html; charset=utf-8"
    );

    let resp = http_get(s.addr, "/v1/style.css").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "text/css; charset=utf-8"
    );

    s.shutdown().await;
}

#[tokio::test]
async fn cache_swap_serves_new_content() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>v1</html>",
    )])));
    let s = spawn_test_server_secured(source.clone()).await;

    // Verify v1. Dest router has no root route; address index.html explicitly.
    let resp = http_get(s.addr, "/v1/index.html").await;
    assert_eq!(resp.text().await.unwrap(), "<html>v1</html>");

    // Swap snapshot to v2.
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>v2</html>",
    )])));

    let resp = http_get(s.addr, "/v1/index.html").await;
    assert_eq!(
        resp.text().await.unwrap(),
        "<html>v2</html>",
        "cache swap should serve new content immediately"
    );

    s.shutdown().await;
}

// ===========================================================================
// Health / readyz
// ===========================================================================

#[tokio::test]
async fn healthz_returns_200_ok() {
    let source = MockProjectionSource::new();
    let s = spawn_test_server_secured(source).await;
    let resp = http_get(s.addr, "/v1/healthz").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    // Dest carries `"v": 1` envelope (CHE-0049 R13).
    assert_eq!(body, serde_json::json!({"v": 1, "status": "ok"}));
    s.shutdown().await;
}

#[tokio::test]
async fn readyz_returns_503_before_cache() {
    let source = MockProjectionSource::new();
    source.set_ready(false);
    let s = spawn_test_server_secured(source).await;
    let resp = http_get(s.addr, "/v1/readyz").await;
    assert_eq!(resp.status(), 503);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "not_ready");
    s.shutdown().await;
}

#[tokio::test]
async fn readyz_returns_200_with_cache_fallback() {
    // Donor name retained. Dest readyz maps `ProjectionSource::is_ready()`;
    // `MockProjectionSource::new()` defaults `ready = true`, so installing
    // a snapshot is sufficient for the equivalent intent — "ready when
    // the projection layer reports ready".
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>hi</html>",
    )])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_get(s.addr, "/v1/readyz").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ready");
    s.shutdown().await;
}

#[tokio::test]
async fn readyz_returns_200_after_completed_run() {
    let source = MockProjectionSource::new();
    source.set_ready(true);
    let s = spawn_test_server_secured(source).await;
    let resp = http_get(s.addr, "/v1/readyz").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ready");
    s.shutdown().await;
}

// ===========================================================================
// Security headers
// ===========================================================================

#[tokio::test]
async fn server_includes_security_headers_on_cached_page() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "report.html",
        "report.html",
        "<html>secure</html>",
    )])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_get(s.addr, "/v1/report.html").await;
    assert_eq!(resp.status(), 200);
    assert_security_headers(&resp, "/v1/report.html");

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

    s.shutdown().await;
}

#[tokio::test]
async fn healthz_has_security_headers() {
    let source = MockProjectionSource::new();
    let s = spawn_test_server_secured(source).await;
    let resp = http_get(s.addr, "/v1/healthz").await;
    assert_security_headers(&resp, "/v1/healthz");
    s.shutdown().await;
}

#[tokio::test]
async fn readyz_has_security_headers() {
    let source = MockProjectionSource::new();
    let s = spawn_test_server_secured(source).await;
    let resp = http_get(s.addr, "/v1/readyz").await;
    assert_security_headers(&resp, "/v1/readyz");
    s.shutdown().await;
}

// ===========================================================================
// ETag / Cache-Control / If-None-Match
// ===========================================================================

#[tokio::test]
async fn cached_page_includes_etag_and_no_cache() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>hello</html>",
    )])));
    let s = spawn_test_server_secured(source).await;

    let resp = http_get(s.addr, "/v1/index.html").await;
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

    s.shutdown().await;
}

#[tokio::test]
async fn matching_if_none_match_returns_304() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>hello</html>",
    )])));
    let s = spawn_test_server_secured(source).await;
    assert_etag_yields_304(s.addr, "/v1/index.html").await;
    s.shutdown().await;
}

#[tokio::test]
async fn non_matching_if_none_match_returns_200() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>hello</html>",
    )])));
    let s = spawn_test_server_secured(source).await;

    let resp = http_get_with_headers(
        s.addr,
        "/v1/index.html",
        &[("if-none-match", "W/\"stale-etag\"")],
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert!(resp.headers().get("etag").is_some());

    s.shutdown().await;
}

#[tokio::test]
async fn etag_304_still_includes_no_cache() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "report.html",
        "report.html",
        "<html>report</html>",
    )])));
    let s = spawn_test_server_secured(source).await;
    assert_etag_yields_304(s.addr, "/v1/report.html").await;
    s.shutdown().await;
}

#[tokio::test]
async fn if_none_match_multi_value_returns_200() {
    // RFC 7232 §3.2 allows multiple ETags in If-None-Match. Dest's
    // implementation compares only the full header value (single-value),
    // so multi-value always returns 200 — donor's documented known
    // limitation, retained here.
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>etag test</html>",
    )])));
    let s = spawn_test_server_secured(source).await;

    let first = http_get(s.addr, "/v1/index.html").await;
    assert_eq!(first.status(), 200);
    let etag = first
        .headers()
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let multi_value = format!(r#"W/"old", {etag}"#);
    let resp =
        http_get_with_headers(s.addr, "/v1/index.html", &[("if-none-match", &multi_value)]).await;
    assert_eq!(
        resp.status(),
        200,
        "multi-value If-None-Match should return 200 (known limitation)"
    );

    s.shutdown().await;
}

// ===========================================================================
// Pre-compression (zstd / identity)
// ===========================================================================

#[tokio::test]
async fn compressed_response_has_content_encoding_and_vary() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>compressed test</html>",
    )])));
    let s = spawn_test_server_secured(source).await;

    let resp =
        http_get_with_headers(s.addr, "/v1/index.html", &[("accept-encoding", "zstd")]).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("content-encoding")
            .expect("should have Content-Encoding")
            .to_str()
            .unwrap(),
        "zstd"
    );
    assert_eq!(
        resp.headers()
            .get("vary")
            .expect("should have Vary")
            .to_str()
            .unwrap(),
        "Accept-Encoding"
    );
    // Sanity: body decodes.
    let body = resp.bytes().await.unwrap();
    let decoded = decode_zstd(&body);
    assert_eq!(decoded, b"<html>compressed test</html>");

    s.shutdown().await;
}

#[tokio::test]
async fn identity_response_for_binary_has_no_content_encoding() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "data.bin",
        "data.bin",
        "raw binary stuff",
    )])));
    let s = spawn_test_server_secured(source).await;

    let resp = http_get_with_headers(s.addr, "/v1/data.bin", &[("accept-encoding", "zstd")]).await;
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers().get("content-encoding").is_none(),
        "binary content should not have Content-Encoding"
    );
    assert!(
        resp.headers().get("vary").is_none(),
        "binary content should not have Vary"
    );

    s.shutdown().await;
}

// ===========================================================================
// WS-path non-upgrade
// ===========================================================================

#[tokio::test]
async fn non_ws_get_to_ws_path_returns_error() {
    let source = MockProjectionSource::new();
    let s = spawn_test_server_secured(source).await;
    let resp = http_get(s.addr, "/ws").await;
    assert!(
        resp.status().is_client_error(),
        "non-upgrade GET to /ws should be a client error, got {}",
        resp.status()
    );
    s.shutdown().await;
}

// ===========================================================================
// Method handling — 405 / HEAD
// ===========================================================================

#[tokio::test]
async fn post_to_cached_page_returns_405() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>hi</html>",
    )])));
    let s = spawn_test_server_secured(source).await;

    let resp = http_request(s.addr, reqwest::Method::POST, "/v1/index.html").await;
    assert_eq!(resp.status(), 405);
    let allow = resp
        .headers()
        .get("allow")
        .expect("405 should include Allow header")
        .to_str()
        .unwrap();
    // Dest emits `Allow: GET,HEAD` (no whitespace); donor used `GET, HEAD`.
    assert_eq!(allow, "GET,HEAD");

    s.shutdown().await;
}

#[tokio::test]
async fn put_to_cached_page_returns_405() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>hi</html>",
    )])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_request(s.addr, reqwest::Method::PUT, "/v1/index.html").await;
    assert_eq!(resp.status(), 405);
    s.shutdown().await;
}

#[tokio::test]
async fn delete_to_cached_page_returns_405() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>hi</html>",
    )])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_request(s.addr, reqwest::Method::DELETE, "/v1/index.html").await;
    assert_eq!(resp.status(), 405);
    s.shutdown().await;
}

#[tokio::test]
async fn head_to_cached_page_returns_200() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>hi</html>",
    )])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_request(s.addr, reqwest::Method::HEAD, "/v1/index.html").await;
    assert_eq!(resp.status(), 200);
    s.shutdown().await;
}

#[tokio::test]
async fn method_not_allowed_has_security_headers() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>hi</html>",
    )])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_request(s.addr, reqwest::Method::POST, "/v1/index.html").await;
    assert_eq!(resp.status(), 405);
    assert_security_headers(&resp, "POST /v1/index.html");
    s.shutdown().await;
}

#[tokio::test]
async fn options_to_cached_page_returns_405() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>hi</html>",
    )])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_request(s.addr, reqwest::Method::OPTIONS, "/v1/index.html").await;
    assert_eq!(resp.status(), 405);
    s.shutdown().await;
}

#[tokio::test]
async fn patch_to_cached_page_returns_405() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>hi</html>",
    )])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_request(s.addr, reqwest::Method::PATCH, "/v1/index.html").await;
    assert_eq!(resp.status(), 405);
    s.shutdown().await;
}

// ===========================================================================
// Fallback resolution (about → about/index.html → about.html)
// ===========================================================================

#[tokio::test]
async fn get_about_serves_about_index_html() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[
        ("about/index.html", "index.html", "<html>about page</html>"),
        ("index.html", "index.html", "<html>root</html>"),
    ])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_get(s.addr, "/v1/about").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "<html>about page</html>");
    s.shutdown().await;
}

#[tokio::test]
async fn get_about_trailing_slash_serves_about_index_html() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[
        ("about/index.html", "index.html", "<html>about page</html>"),
        ("index.html", "index.html", "<html>root</html>"),
    ])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_get(s.addr, "/v1/about/").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "<html>about page</html>");
    s.shutdown().await;
}

#[tokio::test]
async fn get_about_serves_about_html_when_no_index() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[
        ("about.html", "about.html", "<html>about clean url</html>"),
        ("index.html", "index.html", "<html>root</html>"),
    ])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_get(s.addr, "/v1/about").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "<html>about clean url</html>");
    s.shutdown().await;
}

// ===========================================================================
// MIME types — wasm / css
// ===========================================================================

#[tokio::test]
async fn wasm_has_correct_content_type() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[("app.wasm", "app.wasm", "fake wasm")])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_get(s.addr, "/v1/app.wasm").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/wasm"
    );
    s.shutdown().await;
}

#[tokio::test]
async fn style_css_still_works_directly() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "style.css",
        "style.css",
        "body { margin: 0; }",
    )])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_get(s.addr, "/v1/style.css").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "text/css; charset=utf-8"
    );
    s.shutdown().await;
}

// ===========================================================================
// HEAD body shape
// ===========================================================================

#[tokio::test]
async fn head_returns_empty_body_with_content_length() {
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>hello world</html>",
    )])));
    let s = spawn_test_server_secured(source).await;

    let resp = http_request(s.addr, reqwest::Method::HEAD, "/v1/index.html").await;
    assert_eq!(resp.status(), 200);

    let resp_body = resp.bytes().await.unwrap();
    assert!(
        resp_body.is_empty(),
        "HEAD response body should be empty, got {} bytes",
        resp_body.len()
    );

    s.shutdown().await;
}

// ===========================================================================
// Encoding negotiation — 5 negotiate_* tests reframed as HTTP integration
// (Re-task addendum at .ooda/preflight-4c-donor-audit-1778536369.md:170-186)
// ===========================================================================
//
// Donor exercises `negotiate_encoding(&HeaderValue) -> Encoding` directly
// (a private helper at server.rs:164). Dest has no `negotiate` symbol but
// the behaviour is reachable via `Accept-Encoding` header observation
// through `build_projection_router`. Each test asserts the observable
// `Content-Encoding` header (or its absence) on the response.

#[tokio::test]
async fn negotiate_prefers_zstd() {
    // donor: `Accept-Encoding: gzip, deflate, zstd` → server picks zstd.
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>prefers zstd</html>",
    )])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_get_with_headers(
        s.addr,
        "/v1/index.html",
        &[("accept-encoding", "gzip, deflate, zstd")],
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("content-encoding")
            .expect("zstd should be selected")
            .to_str()
            .unwrap(),
        "zstd"
    );
    s.shutdown().await;
}

#[tokio::test]
async fn negotiate_identity_when_no_zstd() {
    // donor: `Accept-Encoding: gzip, deflate` (no zstd) → identity.
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>identity</html>",
    )])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_get_with_headers(
        s.addr,
        "/v1/index.html",
        &[("accept-encoding", "gzip, deflate")],
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers().get("content-encoding").is_none(),
        "no zstd in Accept-Encoding → no Content-Encoding"
    );
    s.shutdown().await;
}

#[tokio::test]
async fn negotiate_identity_for_unknown() {
    // donor: `Accept-Encoding: deflate` (unknown to dest) → identity.
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>identity for unknown</html>",
    )])));
    let s = spawn_test_server_secured(source).await;
    let resp =
        http_get_with_headers(s.addr, "/v1/index.html", &[("accept-encoding", "deflate")]).await;
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers().get("content-encoding").is_none(),
        "unknown encoding → no Content-Encoding"
    );
    s.shutdown().await;
}

// NOTE on q-value tests below.
//
// History: the dest snapshot handler originally used a simpler
// predicate (split-by-`,`, `trim().starts_with("zstd")`) that wrongly
// accepted `zstd;q=0` as a request for zstd. Track 4.2.A (push-item-6)
// replaced that predicate with the full RFC 7231 §5.3.4 q-value parser
// ported from gh-report; the two `negotiate_rejects_q_zero*` tests
// below now assert the donor-aligned correct behaviour.

#[tokio::test]
async fn negotiate_rejects_q_zero() {
    // donor: `negotiate_rejects_q_zero` — `Accept-Encoding: zstd;q=0, gzip`.
    // Track 4.2.A (push-item-6) replaced the prior simplified inline
    // `starts_with("zstd")` predicate with the full RFC 7231 §5.3.4
    // q-value parser ported from gh-report. The q=0 refusal is now
    // honoured: the response MUST be identity-encoded.
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>q-zero refusal</html>",
    )])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_get_with_headers(
        s.addr,
        "/v1/index.html",
        &[("accept-encoding", "zstd;q=0, gzip")],
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers().get("content-encoding").is_none(),
        "q=0 must suppress zstd encoding; got {:?}",
        resp.headers().get("content-encoding"),
    );
    s.shutdown().await;
}

#[tokio::test]
async fn negotiate_rejects_q_zero_with_preceding_params() {
    // donor: `negotiate_rejects_q_zero_with_preceding_params` —
    // `Accept-Encoding: zstd;level=1;q=0, gzip`. Same Track 4.2.A
    // upgrade: the parser now traverses every parameter and honours
    // the trailing `q=0`.
    let source = MockProjectionSource::new();
    source.set_snapshot(Some(mk_snapshot(&[(
        "index.html",
        "index.html",
        "<html>q-zero with params</html>",
    )])));
    let s = spawn_test_server_secured(source).await;
    let resp = http_get_with_headers(
        s.addr,
        "/v1/index.html",
        &[("accept-encoding", "zstd;level=1;q=0, gzip")],
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers().get("content-encoding").is_none(),
        "q=0 with preceding params must suppress zstd encoding; got {:?}",
        resp.headers().get("content-encoding"),
    );
    s.shutdown().await;
}
