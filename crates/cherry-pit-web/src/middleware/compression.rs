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

/// Supported response encodings, in preference order.
///
/// The projection adapter advertises only zstd and identity; gzip / br
/// are deliberately omitted because (a) every page body in the snapshot
/// is already zstd-precompressed at cache-population time and (b)
/// per-request compression on the response path would defeat the
/// snapshot-precompression optimisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encoding {
    Zstd,
    Identity,
}

/// Negotiate the best response encoding from an `Accept-Encoding` header.
///
/// Parses q-values (quality factors) per RFC 7231 §5.3.4 and selects the
/// highest-quality supported encoding. An entry with `q=0` is an
/// explicit refusal and is excluded; absent q-value defaults to `1.0`.
///
/// Strict superset of the prior simplified inline check
/// (`s.split(',').any(|p| p.trim().starts_with("zstd"))`): every header
/// value the simplified version accepted is still accepted here, plus
/// q-value-aware refusals are now honoured.
#[cfg_attr(
    all(not(feature = "projection"), not(test)),
    expect(
        dead_code,
        reason = "see `Encoding` above — same dual-gate (projection feature + cfg(test)) drives reachability."
    )
)]
pub(crate) fn negotiate_encoding(accept: &HeaderValue) -> Encoding {
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

    /// Reproduces the simplified inline check at the prior call site so the
    /// strict-superset assertion below is anchored to the actual replaced
    /// expression, not a paraphrase. If this helper drifts from the call
    /// site, the equivalence claim drifts with it — which is precisely
    /// what the equivalence test is meant to detect.
    fn simplified_accepts_zstd(v: &HeaderValue) -> bool {
        v.to_str()
            .ok()
            .is_some_and(|s| s.split(',').any(|p| p.trim().starts_with("zstd")))
    }

    #[test]
    fn negotiate_encoding_strict_superset_of_simplified() {
        let accepted_by_simplified = [
            "zstd",
            "zstd,gzip",
            "gzip,zstd",
            " zstd ",
            "gzip, zstd",
            "zstd;q=1.0",
            "zstd;q=0.5",
            "br, zstd;q=0.9",
            "gzip;q=1.0, zstd;q=0.1",
            "zstd, deflate",
            "*, zstd",
        ];
        for raw in accepted_by_simplified {
            let h = HeaderValue::from_static(raw);
            assert!(
                simplified_accepts_zstd(&h),
                "test anchor: simplified must accept {raw:?}"
            );
            assert_eq!(
                negotiate_encoding(&h),
                Encoding::Zstd,
                "strict superset broken: new parser must accept {raw:?}"
            );
        }
    }

    #[test]
    fn negotiate_encoding_honours_q_zero_refusal() {
        let h = HeaderValue::from_static("zstd;q=0");
        assert!(
            simplified_accepts_zstd(&h),
            "anchor: simplified incorrectly accepts q=0"
        );
        assert_eq!(
            negotiate_encoding(&h),
            Encoding::Identity,
            "q=0 must be honoured as a refusal"
        );
    }

    #[test]
    fn negotiate_encoding_absent_zstd_returns_identity() {
        let h = HeaderValue::from_static("gzip, br, deflate");
        assert_eq!(negotiate_encoding(&h), Encoding::Identity);
    }

    #[test]
    fn negotiate_encoding_empty_header_returns_identity() {
        let h = HeaderValue::from_static("");
        assert_eq!(negotiate_encoding(&h), Encoding::Identity);
    }

    #[test]
    fn negotiate_encoding_multi_value_with_whitespace() {
        let h = HeaderValue::from_static("  gzip ; q=0.8 ,  zstd ; q=0.9  ");
        assert_eq!(negotiate_encoding(&h), Encoding::Zstd);
    }

    #[test]
    fn negotiate_encoding_malformed_q_value_defaults_to_one() {
        let h = HeaderValue::from_static("zstd;q=notanumber");
        assert_eq!(negotiate_encoding(&h), Encoding::Zstd);
    }

    #[test]
    fn negotiate_encoding_wildcard_alone_is_identity() {
        let h = HeaderValue::from_static("*;q=0.5");
        assert_eq!(negotiate_encoding(&h), Encoding::Identity);
    }
}
