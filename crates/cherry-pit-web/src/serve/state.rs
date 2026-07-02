//! Server state trait, cached page types, and compression utilities.
//!
//! # API Design Choices
//!
//! ## `ArcSwap<Option<HashMap<...>>>`
//!
//! The HTML cache is stored behind [`ArcSwap`] for zero-copy atomic swaps.
//! [`ArcSwap::load()`] is wait-free (no lock contention on the serving
//! hot path), so concurrent readers never block each other or the writer.
//! The entire cache is swapped atomically — no partial-update visibility.
//!
//! ## `broadcast::Sender<PageUpdateEvent>`
//!
//! The WebSocket broadcast channel uses Tokio's `broadcast::Sender` for
//! O(1) per-event serialization cost. [`PageUpdateEvent::json`] contains
//! a pre-serialized JSON payload (`Arc<str>`) built once at broadcast
//! time, avoiding O(N) per-connection serialization. Each WebSocket
//! session receives a clone (cheap `Arc` refcount bump) and forwards
//! the payload directly.
//!
//! ## `HashMap<String, CachedPage>`
//!
//! Simple key-value lookup by cache key (e.g., `"index.html"`,
//! `"section/item.html"`). The entire cache is swapped atomically
//! via `ArcSwap`, so there is no need for concurrent map structures
//! like `DashMap` or `scc::HashMap`.

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use axum::http::HeaderValue;
use bytes::Bytes;
use tracing::warn;

use crate::middleware::compression::{MAX_PRECOMPRESS_BYTES, compress_zstd, compute_etag};

/// Trait abstracting the shared state required by the in-process HTTP server.
///
/// Implementations provide the HTML cache, WebSocket broadcast channel,
/// and readiness logic. Consumers implement the trait on their concrete
/// application state; the router remains statically dispatched over that
/// concrete type.
///
/// Application-specific endpoints (e.g., status, metrics) are
/// registered via `extra_routes` in [`super::runtime::build_router`].
pub trait ServerState: Send + Sync + 'static {
    /// The in-memory HTML page cache. `None` means no content has been
    /// published yet (server returns 503 for page requests).
    fn html_cache(&self) -> &ArcSwap<Option<HashMap<String, CachedPage>>>;

    /// Broadcast sender for notifying WebSocket clients of page updates.
    fn ws_broadcast(&self) -> &tokio::sync::broadcast::Sender<PageUpdateEvent>;

    /// Whether the service is ready to serve traffic.
    ///
    /// Implementations define their own readiness criteria (e.g., a
    /// completed run, a populated cache, or both).
    fn is_ready(&self) -> bool;
}

/// Event broadcast to connected WebSocket clients when pages are updated.
///
/// Sent over `tokio::sync::broadcast` by the producer that refreshed the
/// cache to all connected WebSocket sessions.
/// The client inspects `pages` to decide whether to reload.
///
/// The `json` field contains the pre-serialized JSON payload so that each
/// WebSocket session can forward it directly without re-serializing per
/// connection (O(1) instead of O(N) serializations per broadcast).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PageUpdateEvent {
    /// Cache keys of pages that changed.
    ///
    /// Keys must match the `html_cache` `HashMap` keys. Examples:
    /// `"index.html"`, `"page.html"`, `"section/item.html"`.
    ///
    /// The client compares these against `location.pathname` (with the
    /// leading `/` stripped) to decide whether the current page needs
    /// a reload.
    pub pages: Arc<[Arc<str>]>,
    /// Caller-supplied JSON metadata included in the outbound payload.
    pub metadata: Arc<serde_json::Map<String, serde_json::Value>>,
    /// ISO-8601 timestamp of the evidence that produced this update.
    pub timestamp: Arc<str>,
    /// Pre-serialized JSON payload for zero-cost forwarding to WebSocket
    /// clients. Built once at broadcast time, shared across all receivers.
    pub json: Arc<str>,
}

impl PageUpdateEvent {
    /// Create a new event with no metadata, pre-serializing the JSON payload once.
    #[must_use]
    pub fn new(pages: Vec<String>, timestamp: String) -> Self {
        Self::with_metadata(pages, serde_json::Map::new(), timestamp)
    }

    /// Create a new event with caller-supplied metadata.
    #[must_use]
    pub fn with_metadata(
        pages: Vec<String>,
        metadata: serde_json::Map<String, serde_json::Value>,
        timestamp: String,
    ) -> Self {
        let page_values = pages
            .iter()
            .cloned()
            .map(serde_json::Value::String)
            .collect();
        let mut payload = metadata.clone();
        payload.insert(
            "type".to_string(),
            serde_json::Value::String("update".to_string()),
        );
        payload.insert("pages".to_string(), serde_json::Value::Array(page_values));
        payload.insert(
            "timestamp".to_string(),
            serde_json::Value::String(timestamp.clone()),
        );
        let json = serde_json::Value::Object(payload).to_string();
        let pages: Arc<[Arc<str>]> = pages.into_iter().map(Arc::from).collect();
        Self {
            pages,
            metadata: Arc::new(metadata),
            timestamp: Arc::from(timestamp),
            json: Arc::from(json),
        }
    }
}

/// A single cached HTML page ready to serve.
///
/// Content-Type is derived at cache-population time from the file extension,
/// avoiding repeated inference on the hot path. An `ETag` (weak validator) is
/// pre-computed from a SHA-256 hash of the body, enabling 304 Not Modified
/// responses for unchanged content. Text content (HTML, CSS) is pre-compressed
/// with zstd at cache-population time, so serving requests never
/// re-compress identical content.
///
/// Bodies are stored as [`Bytes`] (reference-counted) so that cloning on
/// the serving path is an atomic refcount increment (~1 ns) rather than a
/// full `memcpy` of the body buffer.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CachedPage {
    /// Raw response body (UTF-8 HTML or CSS). Reference-counted for
    /// zero-copy cloning on the serving hot path.
    pub body: Bytes,
    /// Pre-compressed zstd body, if the content type is compressible.
    pub body_zstd: Option<Bytes>,
    /// Pre-computed `Content-Type` header value.
    pub content_type: HeaderValue,
    /// Pre-computed weak `ETag` derived from body hash (e.g., `W/"a1b2c3..."`).
    pub etag: HeaderValue,
    /// Pre-computed `Content-Length` for the raw body (avoids chunked
    /// transfer encoding when serving identity responses).
    pub content_length: HeaderValue,
    /// Pre-computed `Content-Length` for the zstd body (`None` when no
    /// zstd variant exists).
    pub content_length_zstd: Option<HeaderValue>,
}

impl CachedPage {
    /// Create a `CachedPage`, inferring Content-Type from `filename` and
    /// computing a weak `ETag` from the body's SHA-256 hash (truncated to
    /// 16 bytes / 32 hex chars for brevity).
    ///
    /// Text content types (`.html`, `.css`, `.js`) are pre-compressed with zstd.
    /// Non-text types store `None` for the compressed variant.
    ///
    /// Bodies are converted to [`Bytes`] for zero-copy cloning on the
    /// serving path. Content-Length is pre-computed to avoid chunked
    /// transfer encoding overhead.
    ///
    /// Supported extensions: see `content_type_for_ext` for the full table
    /// (~20 static site types including images, fonts, wasm, etc.).
    #[must_use]
    pub fn new(filename: &str, body: Vec<u8>) -> Self {
        let ext = filename
            .rsplit_once('.')
            .and_then(|(before, after)| if before.is_empty() { None } else { Some(after) });
        let content_type = content_type_for_ext(ext);
        let etag = compute_etag(&body);
        let content_length = content_length_header(body.len());
        let compressible = is_compressible_ext(ext);
        let body_zstd = if compressible {
            let compressed = compress_zstd(&body);
            if compressed.is_none() {
                warn!(
                    filename,
                    body_len = body.len(),
                    max_precompress = MAX_PRECOMPRESS_BYTES,
                    "zstd pre-compression skipped or failed; serving uncompressed"
                );
            }
            compressed
        } else {
            None
        };
        let content_length_zstd = body_zstd.as_ref().map(|b| content_length_header(b.len()));
        Self {
            body: Bytes::from(body),
            body_zstd: body_zstd.map(Bytes::from),
            content_type,
            etag,
            content_length,
            content_length_zstd,
        }
    }
}

/// Look up extension metadata via `match` (O(1) branch table).
fn lookup_ext(ext: Option<&str>) -> Option<(&'static str, bool)> {
    let lower = ext?.to_ascii_lowercase();
    match lower.as_str() {
        "html" => Some(("text/html; charset=utf-8", true)),
        "css" => Some(("text/css; charset=utf-8", true)),
        "js" | "mjs" => Some(("text/javascript; charset=utf-8", true)),
        "json" | "map" => Some(("application/json; charset=utf-8", true)),
        "xml" => Some(("application/xml; charset=utf-8", true)),
        "svg" => Some(("image/svg+xml", true)),
        "txt" => Some(("text/plain; charset=utf-8", true)),
        "png" => Some(("image/png", false)),
        "jpg" | "jpeg" => Some(("image/jpeg", false)),
        "gif" => Some(("image/gif", false)),
        "webp" => Some(("image/webp", false)),
        "avif" => Some(("image/avif", false)),
        "ico" => Some(("image/x-icon", false)),
        "woff" => Some(("font/woff", false)),
        "woff2" => Some(("font/woff2", false)),
        "ttf" => Some(("font/ttf", false)),
        "otf" => Some(("font/otf", false)),
        "wasm" => Some(("application/wasm", false)),
        _ => None,
    }
}

/// Derive `Content-Type` from a pre-extracted file extension.
#[must_use]
pub(crate) fn content_type_for_ext(ext: Option<&str>) -> HeaderValue {
    match lookup_ext(ext) {
        Some((ct, _)) => HeaderValue::from_static(ct),
        None => HeaderValue::from_static("application/octet-stream"),
    }
}

/// Whether the content type warrants pre-compression (text formats only).
#[must_use]
pub(crate) fn is_compressible_ext(ext: Option<&str>) -> bool {
    lookup_ext(ext).is_some_and(|(_, compressible)| compressible)
}

/// Pre-compute a `Content-Length` header value from a byte count.
#[must_use]
pub(crate) fn content_length_header(len: usize) -> HeaderValue {
    HeaderValue::from_str(&len.to_string()).expect("numeric Content-Length is valid ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_page_compresses_html() {
        let body = "<html><body>Hello, world!</body></html>";
        let page = CachedPage::new("index.html", body.as_bytes().to_vec());
        assert!(page.body_zstd.is_some(), "HTML should have zstd variant");
        assert!(!page.body_zstd.as_ref().unwrap().is_empty());
    }

    #[test]
    fn cached_page_compresses_css() {
        let body = "body { color: red; margin: 0; padding: 0; }";
        let page = CachedPage::new("style.css", body.as_bytes().to_vec());
        assert!(page.body_zstd.is_some(), "CSS should have zstd variant");
    }

    #[test]
    fn cached_page_compresses_js() {
        let body = "(function(){ var x = 1; console.log(x); })();";
        let page = CachedPage::new("ws.js", body.as_bytes().to_vec());
        assert!(page.body_zstd.is_some(), "JS should have zstd variant");
        assert_eq!(page.content_type, "text/javascript; charset=utf-8");
    }

    #[test]
    fn cached_page_skips_compression_for_binary() {
        let body = vec![0u8, 1, 2, 3, 4];
        let page = CachedPage::new("data.bin", body);
        assert!(page.body_zstd.is_none(), "binary should not have zstd");
    }

    #[test]
    fn compressed_zstd_round_trips() {
        let original = b"<html><body>Hello, world!</body></html>";
        let page = CachedPage::new("index.html", original.to_vec());

        let compressed = page.body_zstd.expect("zstd should be present");
        let decompressed = zstd::stream::decode_all(&compressed[..]).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn cached_page_has_content_length() {
        let body = "<html>hello</html>";
        let page = CachedPage::new("index.html", body.as_bytes().to_vec());
        assert_eq!(page.content_length, body.len().to_string());
        assert!(page.content_length_zstd.is_some());
    }

    #[test]
    fn cached_page_binary_has_no_zstd_content_length() {
        let page = CachedPage::new("data.bin", vec![0, 1, 2]);
        assert_eq!(page.content_length, "3");
        assert!(page.content_length_zstd.is_none());
    }

    #[test]
    fn mime_type_json() {
        let page = CachedPage::new("data.json", b"{}".to_vec());
        assert_eq!(page.content_type, "application/json; charset=utf-8");
    }

    #[test]
    fn mime_type_xml() {
        let page = CachedPage::new("feed.xml", b"<xml/>".to_vec());
        assert_eq!(page.content_type, "application/xml; charset=utf-8");
    }

    #[test]
    fn mime_type_svg() {
        let page = CachedPage::new("logo.svg", b"<svg/>".to_vec());
        assert_eq!(page.content_type, "image/svg+xml");
    }

    #[test]
    fn mime_type_png() {
        let page = CachedPage::new("img.png", vec![0x89, 0x50]);
        assert_eq!(page.content_type, "image/png");
    }

    #[test]
    fn mime_type_jpg() {
        let page = CachedPage::new("photo.jpg", vec![0xFF, 0xD8]);
        assert_eq!(page.content_type, "image/jpeg");
    }

    #[test]
    fn mime_type_jpeg() {
        let page = CachedPage::new("photo.jpeg", vec![0xFF, 0xD8]);
        assert_eq!(page.content_type, "image/jpeg");
    }

    #[test]
    fn mime_type_gif() {
        let page = CachedPage::new("anim.gif", b"GIF89a".to_vec());
        assert_eq!(page.content_type, "image/gif");
    }

    #[test]
    fn mime_type_webp() {
        let page = CachedPage::new("photo.webp", vec![0; 4]);
        assert_eq!(page.content_type, "image/webp");
    }

    #[test]
    fn mime_type_avif() {
        let page = CachedPage::new("photo.avif", vec![0; 4]);
        assert_eq!(page.content_type, "image/avif");
    }

    #[test]
    fn mime_type_ico() {
        let page = CachedPage::new("favicon.ico", vec![0; 4]);
        assert_eq!(page.content_type, "image/x-icon");
    }

    #[test]
    fn mime_type_woff() {
        let page = CachedPage::new("font.woff", vec![0; 4]);
        assert_eq!(page.content_type, "font/woff");
    }

    #[test]
    fn mime_type_woff2() {
        let page = CachedPage::new("font.woff2", vec![0; 4]);
        assert_eq!(page.content_type, "font/woff2");
    }

    #[test]
    fn mime_type_ttf() {
        let page = CachedPage::new("font.ttf", vec![0; 4]);
        assert_eq!(page.content_type, "font/ttf");
    }

    #[test]
    fn mime_type_otf() {
        let page = CachedPage::new("font.otf", vec![0; 4]);
        assert_eq!(page.content_type, "font/otf");
    }

    #[test]
    fn mime_type_wasm() {
        let page = CachedPage::new("app.wasm", vec![0; 4]);
        assert_eq!(page.content_type, "application/wasm");
    }

    #[test]
    fn mime_type_txt() {
        let page = CachedPage::new("readme.txt", b"hello".to_vec());
        assert_eq!(page.content_type, "text/plain; charset=utf-8");
    }

    #[test]
    fn mime_type_map() {
        let page = CachedPage::new("style.css.map", b"{}".to_vec());
        assert_eq!(page.content_type, "application/json; charset=utf-8");
    }

    #[test]
    fn mime_type_mjs() {
        let page = CachedPage::new("module.mjs", b"export default 1;".to_vec());
        assert_eq!(page.content_type, "text/javascript; charset=utf-8");
    }

    #[test]
    fn mime_type_extensionless_fallback() {
        let page = CachedPage::new("LICENSE", b"MIT".to_vec());
        assert_eq!(page.content_type, "application/octet-stream");
    }

    #[test]
    fn mime_type_double_extension_uses_last() {
        let page = CachedPage::new("archive.tar.gz", vec![0; 4]);
        assert_eq!(page.content_type, "application/octet-stream");
    }

    #[test]
    fn lookup_ext_is_case_insensitive() {
        assert_eq!(
            content_type_for_ext(Some("HTML")),
            content_type_for_ext(Some("html")),
        );
        assert_eq!(
            content_type_for_ext(Some("Css")),
            content_type_for_ext(Some("css")),
        );
        assert_eq!(
            content_type_for_ext(Some("JSON")),
            content_type_for_ext(Some("json")),
        );
        assert!(is_compressible_ext(Some("SVG")));
        assert!(!is_compressible_ext(Some("PNG")));
    }

    #[test]
    fn compressible_svg_json_xml() {
        assert!(is_compressible_ext(Some("svg")));
        assert!(is_compressible_ext(Some("json")));
        assert!(is_compressible_ext(Some("xml")));
        assert!(is_compressible_ext(Some("txt")));
        assert!(is_compressible_ext(Some("map")));
        assert!(is_compressible_ext(Some("mjs")));
    }

    #[test]
    fn not_compressible_images_fonts_wasm() {
        assert!(!is_compressible_ext(Some("png")));
        assert!(!is_compressible_ext(Some("jpg")));
        assert!(!is_compressible_ext(Some("gif")));
        assert!(!is_compressible_ext(Some("webp")));
        assert!(!is_compressible_ext(Some("woff2")));
        assert!(!is_compressible_ext(Some("wasm")));
    }

    #[test]
    fn cached_page_body_is_bytes() {
        let body = b"<html>test</html>";
        let page = CachedPage::new("index.html", body.to_vec());
        let cloned = page.body.clone();
        assert_eq!(cloned, body[..]);
    }

    #[test]
    fn dotfile_has_no_extension() {
        let page = CachedPage::new(".gitignore", b"node_modules".to_vec());
        assert_eq!(page.content_type, "application/octet-stream");
    }

    #[test]
    fn no_extension_returns_octet_stream() {
        let page = CachedPage::new("Makefile", b"all:".to_vec());
        assert_eq!(page.content_type, "application/octet-stream");
    }

    #[test]
    fn compute_etag_format() {
        let etag = compute_etag(b"hello world");
        let s = etag.to_str().unwrap();
        assert!(s.starts_with("W/\""));
        assert!(s.ends_with('"'));
    }

    #[test]
    fn compress_zstd_produces_output() {
        let compressed = compress_zstd(b"<html>hello world</html>").unwrap();
        assert!(!compressed.is_empty());
    }

    #[test]
    fn compress_zstd_rejects_oversize_input() {
        let oversize = vec![0u8; MAX_PRECOMPRESS_BYTES + 1];
        assert!(
            compress_zstd(&oversize).is_none(),
            "inputs above MAX_PRECOMPRESS_BYTES must return None"
        );
    }

    #[test]
    fn compress_zstd_accepts_at_limit() {
        let at_limit = vec![0u8; MAX_PRECOMPRESS_BYTES];
        assert!(
            compress_zstd(&at_limit).is_some(),
            "inputs at exactly MAX_PRECOMPRESS_BYTES must compress"
        );
    }

    #[test]
    fn cached_page_oversize_html_serves_identity_only() {
        let oversize_html = vec![b'x'; MAX_PRECOMPRESS_BYTES + 1];
        let page = CachedPage::new("big.html", oversize_html);
        assert!(page.body_zstd.is_none());
        assert!(page.content_length_zstd.is_none());
    }

    #[test]
    fn content_length_header_format() {
        let hdr = content_length_header(42);
        assert_eq!(hdr.to_str().unwrap(), "42");
    }

    #[test]
    fn page_update_event_json_structure() {
        let event = PageUpdateEvent::new(
            vec!["index.html".into(), "page.html".into()],
            "2026-04-15T12:00:00Z".into(),
        );
        let parsed: serde_json::Value = serde_json::from_str(&event.json).unwrap();
        assert_eq!(parsed["type"], "update");
        assert_eq!(parsed["pages"][0], "index.html");
        assert_eq!(parsed["pages"][1], "page.html");
        assert_eq!(parsed["timestamp"], "2026-04-15T12:00:00Z");
    }
}
