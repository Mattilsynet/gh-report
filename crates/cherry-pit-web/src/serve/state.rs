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

/// Type-distinct body storage for a [`CachedPage`] (Option 2+3, mem-opt-cachedpage-2026-07-11).
///
/// A page body is in exactly one of two legal states — never both, never
/// neither:
///
/// - [`CachedBody::Compressed`] — the common case. Raw body is dropped once
///   the zstd variant exists; identity/non-zstd clients get bounded
///   decode-on-demand via [`CachedBody::identity_bytes`].
/// - [`CachedBody::RawOnly`] — the size-conditional exception (body above
///   `MAX_PRECOMPRESS_BYTES`, or a non-compressible extension): zstd is
///   absent, so raw is the only copy and MUST be retained.
///
/// `ETag` and `Content-Length` are precomputed and stored on [`CachedPage`]
/// independently of this enum, so the 304 and identity-length paths never
/// need to inspect the body at all (oracle adr-fmt-jw9x0, SEC-0003).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum CachedBody {
    /// Raw body is the only copy (oversized page or non-compressible type).
    RawOnly {
        /// The raw response body.
        body: Bytes,
    },
    /// Zstd-compressed body only; raw was dropped after compression.
    Compressed {
        /// The pre-compressed zstd body, served directly to zstd clients.
        zstd: Bytes,
        /// The exact decoded length of `zstd`, captured at build time from
        /// the original raw body. Bounds decode-on-demand allocation to
        /// `raw_len + 1` bytes regardless of what the zstd stream claims,
        /// so a corrupted/hostile stream cannot trigger unbounded alloc
        /// (SEC-0003 decompression-bomb discipline).
        raw_len: usize,
    },
}

impl CachedBody {
    /// The pre-compressed zstd body, if this page has one.
    #[must_use]
    pub fn zstd(&self) -> Option<&Bytes> {
        match self {
            CachedBody::Compressed { zstd, .. } => Some(zstd),
            CachedBody::RawOnly { .. } => None,
        }
    }

    /// Return identity (uncompressed) bytes, decoding on demand and bounded
    /// to `raw_len + 1` bytes for the [`CachedBody::Compressed`] case.
    ///
    /// Returns `None` if decoding fails, or if the decoded length does not
    /// match the stored `raw_len` exactly (fail-closed on corruption or a
    /// hostile stream — never returns partially-decoded or over-length data).
    #[must_use]
    pub fn identity_bytes(&self) -> Option<Bytes> {
        match self {
            CachedBody::RawOnly { body } => Some(body.clone()),
            CachedBody::Compressed { zstd, raw_len } => decode_bounded(zstd, *raw_len),
        }
    }
}

/// Decode `zstd` to identity bytes, bounding the read to `expected_len + 1`
/// bytes so a corrupted or hostile stream cannot force unbounded allocation.
/// Returns `None` unless the decoded length is exactly `expected_len`.
fn decode_bounded(zstd: &[u8], expected_len: usize) -> Option<Bytes> {
    use std::io::Read;
    let decoder = zstd::stream::read::Decoder::new(zstd).ok()?;
    let mut limited = decoder.take(u64::try_from(expected_len).ok()?.saturating_add(1));
    let mut buf = Vec::with_capacity(expected_len);
    limited.read_to_end(&mut buf).ok()?;
    if buf.len() == expected_len {
        Some(Bytes::from(buf))
    } else {
        None
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
/// `ETag` and `Content-Length` are precomputed at build time and stored
/// independently of [`CachedBody`], so the 304 and identity-Content-Length
/// paths never require the raw body to be resident (see [`CachedBody`] docs).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CachedPage {
    /// The response body, in exactly one of its two legal shapes.
    pub body: CachedBody,
    /// Pre-computed `Content-Type` header value.
    pub content_type: HeaderValue,
    /// Pre-computed weak `ETag` derived from the raw body hash (e.g.,
    /// `W/"a1b2c3..."`). Computed once at build time; never requires the
    /// raw body to be resident afterward.
    pub etag: HeaderValue,
    /// Pre-computed `Content-Length` for the raw (identity) body. Computed
    /// once at build time; never requires the raw body to be resident
    /// afterward.
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
    /// Text content types (`.html`, `.css`, `.js`) are pre-compressed with
    /// zstd, and the raw body is dropped ([`CachedBody::Compressed`]).
    /// Non-text types, and any body above `MAX_PRECOMPRESS_BYTES`, retain
    /// the raw body as the only copy ([`CachedBody::RawOnly`]).
    ///
    /// `ETag` and `Content-Length` are computed from the raw body before
    /// it is (possibly) dropped, so both stay available regardless of
    /// which `CachedBody` variant results.
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
        let raw_len = body.len();
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
        let cached_body = match body_zstd {
            Some(zstd) => CachedBody::Compressed {
                zstd: Bytes::from(zstd),
                raw_len,
            },
            None => CachedBody::RawOnly {
                body: Bytes::from(body),
            },
        };
        Self {
            body: cached_body,
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
        assert!(page.body.zstd().is_some(), "HTML should have zstd variant");
        assert!(!page.body.zstd().unwrap().is_empty());
    }

    #[test]
    fn cached_page_compresses_css() {
        let body = "body { color: red; margin: 0; padding: 0; }";
        let page = CachedPage::new("style.css", body.as_bytes().to_vec());
        assert!(page.body.zstd().is_some(), "CSS should have zstd variant");
    }

    #[test]
    fn cached_page_compresses_js() {
        let body = "(function(){ var x = 1; console.log(x); })();";
        let page = CachedPage::new("ws.js", body.as_bytes().to_vec());
        assert!(page.body.zstd().is_some(), "JS should have zstd variant");
        assert_eq!(page.content_type, "text/javascript; charset=utf-8");
    }

    #[test]
    fn cached_page_skips_compression_for_binary() {
        let body = vec![0u8, 1, 2, 3, 4];
        let page = CachedPage::new("data.bin", body);
        assert!(page.body.zstd().is_none(), "binary should not have zstd");
    }

    #[test]
    fn compressed_zstd_round_trips() {
        let original = b"<html><body>Hello, world!</body></html>";
        let page = CachedPage::new("index.html", original.to_vec());

        let compressed = page.body.zstd().expect("zstd should be present");
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
        let identity = page.body.identity_bytes().unwrap();
        assert_eq!(identity, body[..]);
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
        assert!(page.body.zstd().is_none());
        assert!(page.content_length_zstd.is_none());
    }

    #[test]
    fn oversized_page_is_raw_only_variant_and_serves_raw_identity() {
        let oversize_html = vec![b'x'; MAX_PRECOMPRESS_BYTES + 1];
        let page = CachedPage::new("big.html", oversize_html.clone());
        assert!(
            matches!(page.body, CachedBody::RawOnly { .. }),
            "page above MAX_PRECOMPRESS_BYTES must retain raw as the only copy"
        );
        let identity = page
            .body
            .identity_bytes()
            .expect("RawOnly identity_bytes must always succeed");
        assert_eq!(identity, oversize_html[..]);
    }

    #[test]
    fn compressed_page_identity_bytes_round_trip_byte_identical() {
        let original = b"<html><body>Hello, world!</body></html>";
        let page = CachedPage::new("index.html", original.to_vec());
        assert!(
            matches!(page.body, CachedBody::Compressed { .. }),
            "compressible small body must drop raw and retain zstd only"
        );
        let identity = page
            .body
            .identity_bytes()
            .expect("decode-on-demand must succeed for a well-formed zstd body");
        assert_eq!(identity, original[..]);
    }

    #[test]
    fn compressed_page_etag_and_content_length_available_without_raw() {
        let original = b"<html><body>Hello, world!</body></html>";
        let page = CachedPage::new("index.html", original.to_vec());
        assert!(matches!(page.body, CachedBody::Compressed { .. }));
        assert_eq!(page.content_length, original.len().to_string());
        let etag = page.etag.to_str().unwrap();
        assert!(etag.starts_with("W/\""));
    }

    #[test]
    fn bounded_decode_guard_rejects_mismatched_decoded_length() {
        let real = b"<html>some content that compresses fine</html>";
        let zstd_bytes = compress_zstd(real).expect("small body should compress");
        let mismatched = CachedBody::Compressed {
            zstd: Bytes::from(zstd_bytes),
            raw_len: real.len() + 1,
        };
        assert!(
            mismatched.identity_bytes().is_none(),
            "decode must fail closed on a raw_len mismatch, not silently over-allocate"
        );
    }

    #[test]
    fn bounded_decode_guard_caps_allocation_on_oversized_claim() {
        let payload = vec![b'y'; 10_000];
        let zstd_bytes = compress_zstd(&payload).expect("payload within precompress bound");
        let tiny_claim = CachedBody::Compressed {
            zstd: Bytes::from(zstd_bytes),
            raw_len: 4,
        };
        assert!(
            tiny_claim.identity_bytes().is_none(),
            "decode must fail closed when actual decoded size exceeds the stored raw_len bound"
        );
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
