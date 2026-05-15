//! HTTP response body utilities: `ETag` computation and zstd compression
//! (CHE-0049 R8 transport helpers).
//!
//! Pure functions over byte slices — no I/O, no shared state. Used by
//! HTTP handlers to attach weak `ETag`s and pre-compress responses.
//!
//! Ported byte-for-byte from the donor crate per CHE-0049 R14; donor copies
//! remain in that crate until the gh-report migration completes.

use axum::http::HeaderValue;
use sha2::{Digest, Sha256};

/// Compute a weak `ETag` from a SHA-256 hash of `body`, truncated to 16 bytes.
///
/// Format: `W/"<32 hex chars>"` — weak because the same `ETag` matches
/// regardless of content encoding (gzip, zstd, identity).
///
/// Uses a single `String::with_capacity(36)` allocation: 3 bytes for `W/"`,
/// 32 hex chars, and 1 closing `"`. No intermediate allocations.
///
/// # Panics
///
/// Panics if the generated `ETag` string is not valid ASCII (should never
/// happen since the output is hex-encoded).
#[must_use]
pub fn compute_etag(body: &[u8]) -> HeaderValue {
    use std::fmt::Write;
    let hash = Sha256::digest(body);
    // W/" (3) + 32 hex chars + " (1) = 36 bytes
    let mut etag_str = String::with_capacity(36);
    etag_str.push_str("W/\"");
    for b in &hash[..16] {
        write!(etag_str, "{b:02x}").expect("hex write to String is infallible");
    }
    etag_str.push('"');
    HeaderValue::from_str(&etag_str).expect("ETag is valid ASCII")
}

/// Maximum input size accepted by [`compress_zstd`] (1 MiB).
///
/// Pages larger than this are served identity-only. Bounds the worst-case
/// CPU cost of cache-population at zstd level 19, mitigating R1 (unbounded
/// compression CPU). Inputs above the limit return `None` from
/// `compress_zstd`; callers fall back to identity serving on `None`.
///
/// 1 MiB is generous for the intended workload (HTML/CSS/JS dashboards,
/// typically tens of kilobytes per page). Adjust if a legitimate use case
/// needs larger pre-compressed bodies.
pub const MAX_PRECOMPRESS_BYTES: usize = 1 << 20;

/// Compress `body` with zstd (level 19).
///
/// Uses `zstd::stream::encode_all` which handles buffer management
/// internally (zstd typically achieves 2-4× compression on HTML/CSS/JS).
///
/// Returns `None` for inputs larger than 1 MiB to bound worst-case CPU
/// at cache-population time. Callers fall back to identity serving on
/// `None`.
#[must_use]
pub fn compress_zstd(body: &[u8]) -> Option<Vec<u8>> {
    if body.len() > MAX_PRECOMPRESS_BYTES {
        return None;
    }
    zstd::stream::encode_all(std::io::Cursor::new(body), 19).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // R1 mitigation: bodies larger than MAX_PRECOMPRESS_BYTES return None
        // so callers fall back to identity serving rather than blocking on a
        // worst-case level-19 compression at the cache-population stage.
        let oversize = vec![0u8; MAX_PRECOMPRESS_BYTES + 1];
        assert!(
            compress_zstd(&oversize).is_none(),
            "inputs above MAX_PRECOMPRESS_BYTES must return None"
        );
    }

    #[test]
    fn compress_zstd_accepts_at_limit() {
        // Boundary case: exactly MAX_PRECOMPRESS_BYTES is still compressible.
        let at_limit = vec![0u8; MAX_PRECOMPRESS_BYTES];
        assert!(
            compress_zstd(&at_limit).is_some(),
            "inputs at exactly MAX_PRECOMPRESS_BYTES must compress"
        );
    }
}
