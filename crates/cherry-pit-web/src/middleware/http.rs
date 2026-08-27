//! Shared HTTP conditional-request helpers (CHE-0086:R8).
//!
//! Single home for the `ETag` comparison used by both the static-serve
//! runtime and the projection adapter. Pure functions over header
//! values — no I/O, no shared state.

use axum::http::HeaderValue;

/// Weak `ETag` comparison per RFC 7232 §2.3.2.
///
/// Handles the `*` wildcard: `If-None-Match: *` matches any `ETag`.
/// Otherwise strips a leading `W/` from both values (if present) before
/// comparing the opaque-tag portion, so a strong client tag matches the
/// weak server tag carrying the same opaque value.
pub(crate) fn etag_weak_match(client_val: &HeaderValue, server_val: &HeaderValue) -> bool {
    fn strip_weak(v: &[u8]) -> &[u8] {
        v.strip_prefix(b"W/").unwrap_or(v)
    }

    if client_val.as_bytes() == b"*" {
        return true;
    }

    strip_weak(client_val.as_bytes()) == strip_weak(server_val.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etag_weak_match_identical() {
        let v = HeaderValue::from_static("W/\"abc123\"");
        assert!(etag_weak_match(&v, &v));
    }

    #[test]
    fn etag_weak_match_strips_w_prefix() {
        let client = HeaderValue::from_static("\"abc123\"");
        let server = HeaderValue::from_static("W/\"abc123\"");
        assert!(etag_weak_match(&client, &server));
    }

    #[test]
    fn etag_weak_match_strong_client_weak_server() {
        let client = HeaderValue::from_static("\"abc123\"");
        let server = HeaderValue::from_static("W/\"abc123\"");
        assert!(etag_weak_match(&client, &server));
    }

    #[test]
    fn etag_weak_match_different_values() {
        let client = HeaderValue::from_static("W/\"abc123\"");
        let server = HeaderValue::from_static("W/\"def456\"");
        assert!(!etag_weak_match(&client, &server));
    }

    #[test]
    fn etag_weak_match_empty_values() {
        let client = HeaderValue::from_static("");
        let server = HeaderValue::from_static("");
        assert!(etag_weak_match(&client, &server));
    }

    #[test]
    fn etag_weak_match_w_prefix_only() {
        let client = HeaderValue::from_static("W/");
        let server = HeaderValue::from_static("");
        assert!(etag_weak_match(&client, &server));
    }

    #[test]
    fn etag_weak_match_malformed_no_closing_quote() {
        let client = HeaderValue::from_static("W/\"abc123");
        let server = HeaderValue::from_static("W/\"abc123");
        assert!(etag_weak_match(&client, &server));
    }

    #[test]
    fn etag_weak_match_malformed_vs_well_formed() {
        let client = HeaderValue::from_static("W/\"abc123");
        let server = HeaderValue::from_static("W/\"abc123\"");
        assert!(!etag_weak_match(&client, &server));
    }

    #[test]
    fn etag_weak_match_wildcard() {
        let client = HeaderValue::from_static("*");
        let server = HeaderValue::from_static("W/\"anything\"");
        assert!(etag_weak_match(&client, &server));
    }
}
