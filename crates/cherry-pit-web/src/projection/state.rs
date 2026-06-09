//! Typed state and migrated domain types for the projection adapter.
//!
//! Per CHE-0005 R1 + CHE-0049 R12, [`ProjectionState`] is generic in
//! `P: ProjectionSource` — typed state, no boxed/arced trait objects.
//! The adapter is held as `Arc<P>` so the state is cheap to clone across
//! handler tasks.
//!
//! Phase 3c migrates the donor crate's `state::{CachedPage, PageUpdateEvent}`
//! into [`PageEntry`] and [`PageUpdate`] (rename per m5 brief);
//! `PageUpdate` adds [`CorrelationContext`] per CHE-0039 R2. No `serde`
//! decode bound is added to either type — A3 closed, CHE-0014 R2
//! preserved. Both types carry `#[non_exhaustive]` so additive field
//! growth (e.g. trace IDs) does not break `SemVer`.

use std::sync::Arc;

use axum::http::HeaderValue;
use bytes::Bytes;
use cherry_pit_core::CorrelationContext;
use sha2::{Digest, Sha256};
use tracing::warn;

use super::port::ProjectionSource;

/// A single cached HTML page ready to serve.
///
/// Content-Type is derived at cache-population time from the file
/// extension, avoiding repeated inference on the hot path. An `ETag`
/// (weak validator) is pre-computed from a SHA-256 hash of the body,
/// enabling 304 Not Modified responses for unchanged content. Text
/// content (HTML, CSS, JS) is pre-compressed with zstd at
/// cache-population time, so serving requests never re-compress
/// identical content.
///
/// Bodies are stored as [`Bytes`] (reference-counted) so cloning on the
/// serving path is an atomic refcount increment rather than a full
/// `memcpy` of the body buffer.
///
/// No `serde` decode bound (CHE-0014 R2; A3). `#[non_exhaustive]` so
/// additive field growth is non-breaking.
///
/// Migrated from the donor crate's `state::CachedPage` (m5 Phase 3c).
#[derive(Clone)]
#[non_exhaustive]
pub struct PageEntry {
    /// Raw response body (UTF-8 HTML or CSS). Reference-counted for
    /// zero-copy cloning on the serving hot path.
    pub body: Bytes,
    /// Pre-compressed zstd body, if the content type is compressible.
    pub body_zstd: Option<Bytes>,
    /// Pre-computed `Content-Type` header value.
    pub content_type: HeaderValue,
    /// Pre-computed weak `ETag` derived from body hash (e.g.,
    /// `W/"a1b2c3..."`).
    pub etag: HeaderValue,
    /// Pre-computed `Content-Length` for the raw body (avoids chunked
    /// transfer encoding when serving identity responses).
    pub content_length: HeaderValue,
    /// Pre-computed `Content-Length` for the zstd body (`None` when no
    /// zstd variant exists).
    pub content_length_zstd: Option<HeaderValue>,
}

impl PageEntry {
    /// Create a `PageEntry`, inferring Content-Type from `filename` and
    /// computing a weak `ETag` from the body's SHA-256 hash (truncated
    /// to 16 bytes / 32 hex chars for brevity).
    ///
    /// Text content types (`.html`, `.css`, `.js`) are pre-compressed
    /// with zstd. Non-text types store `None` for the compressed
    /// variant.
    ///
    /// Bodies are converted to [`Bytes`] for zero-copy cloning on the
    /// serving path. Content-Length is pre-computed to avoid chunked
    /// transfer encoding overhead.
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

/// Manual `Debug` impl that elides large body byte buffers in favour of
/// a stable byte-count summary. Avoids dumping kilobytes of HTML/CSS
/// into trace output and keeps trace ids readable.
impl std::fmt::Debug for PageEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageEntry")
            .field("body", &format_args!("<{} bytes>", self.body.len()))
            .field(
                "body_zstd",
                &self
                    .body_zstd
                    .as_ref()
                    .map(|b| format!("<{} bytes>", b.len())),
            )
            .field("content_type", &self.content_type)
            .field("etag", &self.etag)
            .field("content_length", &self.content_length)
            .field("content_length_zstd", &self.content_length_zstd)
            .finish()
    }
}

/// Delta event broadcast to connected WebSocket clients when projection
/// pages are updated.
///
/// Per CHE-0049 R11/R13, the projection adapter publishes snapshot
/// deltas (not raw event envelopes) over the `/ws` upgrade route. The
/// `json` field carries the pre-serialised JSON payload so each
/// WebSocket session forwards it directly without re-serialising per
/// connection (O(1) instead of O(N) serialisations per broadcast).
///
/// Per CHE-0039 R2, every update carries an explicit
/// [`CorrelationContext`] so deltas can be correlated with the command
/// that triggered them. The context is *not* serialised into the
/// outgoing WS payload by default; it is held alongside for adapter-side
/// logging and trace correlation.
///
/// No `serde` decode bound (CHE-0014 R2; A3). `#[non_exhaustive]` so
/// additive field growth is non-breaking.
///
/// Migrated from the donor crate's `state::PageUpdateEvent` (m5 Phase 3c).
#[derive(Clone)]
#[non_exhaustive]
pub struct PageUpdate {
    /// Cache keys of pages that changed.
    ///
    /// Keys must match the snapshot map's keys. Examples:
    /// `"index.html"`, `"report.html"`, `"owners/team-a.html"`.
    pub pages: Arc<[Arc<str>]>,
    /// Repository / aggregate that triggered this update
    /// (empty for full sweep).
    pub repo: Arc<str>,
    /// ISO-8601 timestamp of the evidence that produced this update.
    pub timestamp: Arc<str>,
    /// Pre-serialised JSON payload for zero-cost forwarding to
    /// WebSocket clients. Built once at broadcast time, shared across
    /// all receivers.
    pub json: Arc<str>,
    /// Correlation context tying this delta back to the command /
    /// projection event that produced it (CHE-0039 R2).
    pub correlation: CorrelationContext,
}

impl PageUpdate {
    /// Create a new delta event, pre-serialising the JSON payload once.
    ///
    /// `correlation` is recorded on the event but is not included in
    /// the JSON payload — it is propagated separately for adapter-side
    /// trace correlation.
    #[must_use]
    pub fn new(
        pages: Vec<String>,
        repo: String,
        timestamp: String,
        correlation: CorrelationContext,
    ) -> Self {
        let json = serde_json::json!({
            "v": 1,
            "type": "update",
            "pages": pages,
            "repo": repo,
            "timestamp": timestamp,
        })
        .to_string();
        let pages: Arc<[Arc<str>]> = pages.into_iter().map(Arc::from).collect();
        Self {
            pages,
            repo: Arc::from(repo),
            timestamp: Arc::from(timestamp),
            json: Arc::from(json),
            correlation,
        }
    }
}

/// Manual `Debug` impl elides the pre-serialised JSON payload to a
/// byte-count summary. The `pages`, `repo`, `timestamp`, and
/// `correlation` fields format directly.
impl std::fmt::Debug for PageUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageUpdate")
            .field("pages", &self.pages)
            .field("repo", &self.repo)
            .field("timestamp", &self.timestamp)
            .field("json", &format_args!("<{} bytes>", self.json.len()))
            .field("correlation", &self.correlation)
            .finish()
    }
}

/// Typed adapter state threaded through [`super::build_projection_router`].
///
/// Generic in `P: ProjectionSource` per CHE-0005 R1 + CHE-0049 R12 — no
/// trait objects. Clone is cheap (`Arc` bump). The manual `Debug` impl
/// does NOT require `P: Debug` — it prints the strong-count of the
/// underlying `Arc` only, so `P` may be any type that satisfies
/// `ProjectionSource` (which does not include `Debug`).
pub struct ProjectionState<P> {
    source: Arc<P>,
}

impl<P> ProjectionState<P>
where
    P: ProjectionSource,
{
    /// Wrap an owned projection source for use by the web adapter.
    #[must_use]
    pub fn new(source: P) -> Self {
        Self {
            source: Arc::new(source),
        }
    }

    /// Wrap an already-shared projection source.
    #[must_use]
    pub fn from_arc(source: Arc<P>) -> Self {
        Self { source }
    }

    /// Borrow the underlying projection source.
    #[must_use]
    pub fn source(&self) -> &Arc<P> {
        &self.source
    }
}

impl<P> Clone for ProjectionState<P> {
    fn clone(&self) -> Self {
        Self {
            source: Arc::clone(&self.source),
        }
    }
}

/// Manual `Debug` impl independent of `P: Debug` (closes Phase-2 linus
/// Info-1). `ProjectionSource` does not require `Debug`, so deriving
/// `Debug` on `ProjectionState<P>` would constrain `P` unnecessarily.
impl<P> std::fmt::Debug for ProjectionState<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectionState")
            .field(
                "source",
                &format_args!("Arc<P>(strong={})", Arc::strong_count(&self.source)),
            )
            .finish()
    }
}

/// Compute a weak `ETag` from a SHA-256 hash of `body`, truncated to 16 bytes.
///
/// Format: `W/"<32 hex chars>"` — weak because the same `ETag` matches
/// regardless of content encoding (gzip, zstd, identity).
fn compute_etag(body: &[u8]) -> HeaderValue {
    use std::fmt::Write;
    let hash = Sha256::digest(body);
    let mut etag_str = String::with_capacity(36);
    etag_str.push_str("W/\"");
    for b in &hash[..16] {
        write!(etag_str, "{b:02x}").expect("hex write to String is infallible");
    }
    etag_str.push('"');
    HeaderValue::from_str(&etag_str).expect("ETag is valid ASCII")
}

/// Look up extension metadata via `match` (O(1) branch table).
///
/// Returns `(content_type, is_compressible)` for known extensions.
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
///
/// Fallback is `application/octet-stream`.
fn content_type_for_ext(ext: Option<&str>) -> HeaderValue {
    match lookup_ext(ext) {
        Some((ct, _)) => HeaderValue::from_static(ct),
        None => HeaderValue::from_static("application/octet-stream"),
    }
}

/// Whether the content type warrants pre-compression (text formats only).
fn is_compressible_ext(ext: Option<&str>) -> bool {
    lookup_ext(ext).is_some_and(|(_, compressible)| compressible)
}

/// Maximum input size accepted by [`compress_zstd`] (1 MiB).
///
/// Bounds the worst-case CPU cost of cache-population at zstd level 19.
/// Inputs above the limit return `None`; [`PageEntry::new`] logs a
/// `warn!` and falls back to identity serving.
const MAX_PRECOMPRESS_BYTES: usize = 1 << 20;

/// Compress `body` with zstd (level 19).
///
/// Returns `None` for inputs larger than [`MAX_PRECOMPRESS_BYTES`].
fn compress_zstd(body: &[u8]) -> Option<Vec<u8>> {
    if body.len() > MAX_PRECOMPRESS_BYTES {
        return None;
    }
    zstd::stream::encode_all(std::io::Cursor::new(body), 19).ok()
}

/// Pre-compute a `Content-Length` header value from a byte count.
fn content_length_header(len: usize) -> HeaderValue {
    HeaderValue::from_str(&len.to_string()).expect("numeric Content-Length is valid ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_entry_compresses_html() {
        let body = "<html><body>Hello, world!</body></html>";
        let entry = PageEntry::new("index.html", body.as_bytes().to_vec());
        assert!(entry.body_zstd.is_some(), "HTML should have zstd variant");
        assert!(!entry.body_zstd.as_ref().unwrap().is_empty());
    }

    #[test]
    fn page_entry_compresses_css() {
        let entry = PageEntry::new("style.css", b"body { color: red; }".to_vec());
        assert!(entry.body_zstd.is_some(), "CSS should have zstd variant");
    }

    #[test]
    fn page_entry_compresses_js() {
        let entry = PageEntry::new("ws.js", b"(function(){})();".to_vec());
        assert!(entry.body_zstd.is_some(), "JS should have zstd variant");
        assert_eq!(entry.content_type, "text/javascript; charset=utf-8");
    }

    #[test]
    fn page_entry_skips_compression_for_binary() {
        let entry = PageEntry::new("data.bin", vec![0u8, 1, 2, 3, 4]);
        assert!(entry.body_zstd.is_none(), "binary should not have zstd");
    }

    #[test]
    fn page_entry_zstd_round_trips() {
        let original = b"<html><body>Hello, world!</body></html>";
        let entry = PageEntry::new("index.html", original.to_vec());
        let compressed = entry.body_zstd.expect("zstd should be present");
        let decompressed = zstd::stream::decode_all(&compressed[..]).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn page_entry_has_content_length() {
        let body = "<html>hello</html>";
        let entry = PageEntry::new("index.html", body.as_bytes().to_vec());
        assert_eq!(entry.content_length, body.len().to_string());
        assert!(entry.content_length_zstd.is_some());
    }

    #[test]
    fn page_entry_binary_has_no_zstd_content_length() {
        let entry = PageEntry::new("data.bin", vec![0, 1, 2]);
        assert_eq!(entry.content_length, "3");
        assert!(entry.content_length_zstd.is_none());
    }

    #[test]
    fn mime_type_json() {
        let entry = PageEntry::new("data.json", b"{}".to_vec());
        assert_eq!(entry.content_type, "application/json; charset=utf-8");
    }

    #[test]
    fn mime_type_xml() {
        let entry = PageEntry::new("feed.xml", b"<xml/>".to_vec());
        assert_eq!(entry.content_type, "application/xml; charset=utf-8");
    }

    #[test]
    fn mime_type_svg() {
        let entry = PageEntry::new("logo.svg", b"<svg/>".to_vec());
        assert_eq!(entry.content_type, "image/svg+xml");
    }

    #[test]
    fn mime_type_png() {
        let entry = PageEntry::new("img.png", vec![0x89, 0x50]);
        assert_eq!(entry.content_type, "image/png");
    }

    #[test]
    fn mime_type_jpg() {
        let entry = PageEntry::new("photo.jpg", vec![0xFF, 0xD8]);
        assert_eq!(entry.content_type, "image/jpeg");
    }

    #[test]
    fn mime_type_jpeg() {
        let entry = PageEntry::new("photo.jpeg", vec![0xFF, 0xD8]);
        assert_eq!(entry.content_type, "image/jpeg");
    }

    #[test]
    fn mime_type_gif() {
        let entry = PageEntry::new("anim.gif", b"GIF89a".to_vec());
        assert_eq!(entry.content_type, "image/gif");
    }

    #[test]
    fn mime_type_webp() {
        let entry = PageEntry::new("photo.webp", vec![0; 4]);
        assert_eq!(entry.content_type, "image/webp");
    }

    #[test]
    fn mime_type_avif() {
        let entry = PageEntry::new("photo.avif", vec![0; 4]);
        assert_eq!(entry.content_type, "image/avif");
    }

    #[test]
    fn mime_type_ico() {
        let entry = PageEntry::new("favicon.ico", vec![0; 4]);
        assert_eq!(entry.content_type, "image/x-icon");
    }

    #[test]
    fn mime_type_woff() {
        let entry = PageEntry::new("font.woff", vec![0; 4]);
        assert_eq!(entry.content_type, "font/woff");
    }

    #[test]
    fn mime_type_woff2() {
        let entry = PageEntry::new("font.woff2", vec![0; 4]);
        assert_eq!(entry.content_type, "font/woff2");
    }

    #[test]
    fn mime_type_ttf() {
        let entry = PageEntry::new("font.ttf", vec![0; 4]);
        assert_eq!(entry.content_type, "font/ttf");
    }

    #[test]
    fn mime_type_otf() {
        let entry = PageEntry::new("font.otf", vec![0; 4]);
        assert_eq!(entry.content_type, "font/otf");
    }

    #[test]
    fn mime_type_wasm() {
        let entry = PageEntry::new("app.wasm", vec![0; 4]);
        assert_eq!(entry.content_type, "application/wasm");
    }

    #[test]
    fn mime_type_txt() {
        let entry = PageEntry::new("readme.txt", b"hello".to_vec());
        assert_eq!(entry.content_type, "text/plain; charset=utf-8");
    }

    #[test]
    fn mime_type_map() {
        let entry = PageEntry::new("style.css.map", b"{}".to_vec());
        assert_eq!(entry.content_type, "application/json; charset=utf-8");
    }

    #[test]
    fn mime_type_mjs() {
        let entry = PageEntry::new("module.mjs", b"export default 1;".to_vec());
        assert_eq!(entry.content_type, "text/javascript; charset=utf-8");
    }

    #[test]
    fn mime_type_extensionless_fallback() {
        let entry = PageEntry::new("LICENSE", b"MIT".to_vec());
        assert_eq!(entry.content_type, "application/octet-stream");
    }

    #[test]
    fn mime_type_double_extension_uses_last() {
        let entry = PageEntry::new("archive.tar.gz", vec![0; 4]);
        assert_eq!(entry.content_type, "application/octet-stream");
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
        assert!(is_compressible_ext(Some("SVG")));
        assert!(!is_compressible_ext(Some("PNG")));
    }

    #[test]
    fn compressible_text_formats() {
        assert!(is_compressible_ext(Some("svg")));
        assert!(is_compressible_ext(Some("json")));
        assert!(is_compressible_ext(Some("xml")));
        assert!(is_compressible_ext(Some("txt")));
        assert!(is_compressible_ext(Some("map")));
        assert!(is_compressible_ext(Some("mjs")));
    }

    #[test]
    fn not_compressible_binary_formats() {
        assert!(!is_compressible_ext(Some("png")));
        assert!(!is_compressible_ext(Some("jpg")));
        assert!(!is_compressible_ext(Some("woff2")));
        assert!(!is_compressible_ext(Some("wasm")));
    }

    #[test]
    fn page_entry_body_is_bytes() {
        let body = b"<html>test</html>";
        let entry = PageEntry::new("index.html", body.to_vec());
        let cloned = entry.body.clone();
        assert_eq!(cloned, body[..]);
    }

    #[test]
    fn dotfile_has_no_extension() {
        let entry = PageEntry::new(".gitignore", b"node_modules".to_vec());
        assert_eq!(entry.content_type, "application/octet-stream");
    }

    #[test]
    fn no_extension_returns_octet_stream() {
        let entry = PageEntry::new("Makefile", b"all:".to_vec());
        assert_eq!(entry.content_type, "application/octet-stream");
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
        assert!(compress_zstd(&oversize).is_none());
    }

    #[test]
    fn compress_zstd_accepts_at_limit() {
        let at_limit = vec![0u8; MAX_PRECOMPRESS_BYTES];
        assert!(compress_zstd(&at_limit).is_some());
    }

    #[test]
    fn page_entry_oversize_html_serves_identity_only() {
        let oversize = vec![b'x'; MAX_PRECOMPRESS_BYTES + 1];
        let entry = PageEntry::new("big.html", oversize);
        assert!(entry.body_zstd.is_none());
        assert!(entry.content_length_zstd.is_none());
    }

    #[test]
    fn content_length_header_format() {
        let hdr = content_length_header(42);
        assert_eq!(hdr.to_str().unwrap(), "42");
    }

    #[test]
    fn page_update_json_structure() {
        let update = PageUpdate::new(
            vec!["index.html".into(), "report.html".into()],
            "my-repo".into(),
            "2026-04-15T12:00:00Z".into(),
            CorrelationContext::none(),
        );
        let parsed: serde_json::Value = serde_json::from_str(&update.json).unwrap();
        assert_eq!(parsed["v"], 1);
        assert_eq!(parsed["type"], "update");
        assert_eq!(parsed["pages"][0], "index.html");
        assert_eq!(parsed["pages"][1], "report.html");
        assert_eq!(parsed["repo"], "my-repo");
        assert_eq!(parsed["timestamp"], "2026-04-15T12:00:00Z");
    }

    #[test]
    fn page_update_records_correlation() {
        let corr = uuid::Uuid::now_v7();
        let cause = uuid::Uuid::now_v7();
        let ctx = CorrelationContext::new(corr, cause);
        let update = PageUpdate::new(vec!["x.html".into()], "r".into(), "t".into(), ctx.clone());
        assert_eq!(update.correlation, ctx);
    }

    #[test]
    fn page_update_uncorrelated() {
        let update = PageUpdate::new(
            vec!["x.html".into()],
            "r".into(),
            "t".into(),
            CorrelationContext::none(),
        );
        assert!(update.correlation.correlation_id().is_none());
        assert!(update.correlation.causation_id().is_none());
    }

    /// A `ProjectionSource` impl that deliberately does NOT implement
    /// `Debug`. If `ProjectionState<P>: Debug` requires `P: Debug`,
    /// this test will not compile.
    struct NonDebugProjection;

    impl ProjectionSource for NonDebugProjection {
        fn snapshot(&self) -> Option<Arc<std::collections::HashMap<String, PageEntry>>> {
            None
        }
        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<PageUpdate> {
            let (tx, rx) = tokio::sync::broadcast::channel(1);
            drop(tx);
            rx
        }
        fn is_ready(&self) -> bool {
            false
        }
    }

    #[test]
    fn projection_state_debug_independent_of_p_debug() {
        let state = ProjectionState::new(NonDebugProjection);
        let formatted = format!("{state:?}");
        assert!(formatted.contains("ProjectionState"));
        assert!(formatted.contains("strong="));
    }

    #[test]
    fn projection_state_clone_is_cheap() {
        let state = ProjectionState::new(NonDebugProjection);
        let _cloned = state.clone();
        let formatted = format!("{state:?}");
        assert!(formatted.contains("strong=2"));
    }

    #[test]
    fn page_entry_debug_elides_body_bytes() {
        let entry = PageEntry::new("index.html", b"<html>hi</html>".to_vec());
        let formatted = format!("{entry:?}");
        assert!(formatted.contains("<15 bytes>"));
        assert!(!formatted.contains("<html>hi</html>"));
    }

    #[test]
    fn page_update_debug_elides_json_bytes() {
        let update = PageUpdate::new(
            vec!["x.html".into()],
            "r".into(),
            "t".into(),
            CorrelationContext::none(),
        );
        let formatted = format!("{update:?}");
        assert!(formatted.contains("json: <"));
        assert!(formatted.contains("bytes>"));
    }
}
