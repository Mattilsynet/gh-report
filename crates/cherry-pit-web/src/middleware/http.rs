//! Shared HTTP conditional-request helpers (CHE-0086:R8).
//!
//! Single home for the `If-None-Match` evaluation used by both the
//! static-serve runtime and the projection adapter. Pure functions over
//! header values — no I/O, no shared state.

use axum::http::{HeaderMap, HeaderValue, header};

/// Evaluate `If-None-Match` against the server's `ETag` per RFC 7232
/// §3.2.
///
/// The field is a **list**, and a client may spread that list across
/// several header instances as well as commas within one. Both forms
/// are walked here; a match on any entry means "not modified".
///
/// Previously only `HeaderMap::get` was consulted and the whole value
/// was compared as one opaque string, so any client sending more than
/// one validator — which RFC 7232 explicitly permits, and caches do —
/// silently received `200` with a full body instead of `304`.
pub(crate) fn if_none_match_matches(headers: &HeaderMap, server_etag: &HeaderValue) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|candidate| etag_weak_match(candidate.as_bytes(), server_etag.as_bytes()))
}

/// Weak `ETag` comparison per RFC 7232 §2.3.2.
///
/// Handles the `*` wildcard: `If-None-Match: *` matches any `ETag`.
/// Otherwise strips a leading `W/` from both values (if present) before
/// comparing the opaque-tag portion, so a strong client tag matches the
/// weak server tag carrying the same opaque value.
fn etag_weak_match(client_val: &[u8], server_val: &[u8]) -> bool {
    fn strip_weak(v: &[u8]) -> &[u8] {
        v.strip_prefix(b"W/").unwrap_or(v)
    }

    if client_val == b"*" {
        return true;
    }

    strip_weak(client_val) == strip_weak(server_val)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(value: &'static str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::IF_NONE_MATCH, HeaderValue::from_static(value));
        h
    }

    fn etag(value: &'static str) -> HeaderValue {
        HeaderValue::from_static(value)
    }

    #[test]
    fn etag_weak_match_identical() {
        assert!(if_none_match_matches(
            &client("W/\"abc123\""),
            &etag("W/\"abc123\"")
        ));
    }

    #[test]
    fn etag_weak_match_strips_w_prefix() {
        assert!(if_none_match_matches(
            &client("\"abc123\""),
            &etag("W/\"abc123\"")
        ));
    }

    #[test]
    fn etag_weak_match_different_values() {
        assert!(!if_none_match_matches(
            &client("W/\"abc123\""),
            &etag("W/\"def456\"")
        ));
    }

    #[test]
    fn etag_weak_match_empty_values() {
        assert!(if_none_match_matches(&client(""), &etag("")));
    }

    #[test]
    fn etag_weak_match_w_prefix_only() {
        assert!(if_none_match_matches(&client("W/"), &etag("")));
    }

    #[test]
    fn etag_weak_match_malformed_no_closing_quote() {
        assert!(if_none_match_matches(
            &client("W/\"abc123"),
            &etag("W/\"abc123")
        ));
    }

    #[test]
    fn etag_weak_match_malformed_vs_well_formed() {
        assert!(!if_none_match_matches(
            &client("W/\"abc123"),
            &etag("W/\"abc123\"")
        ));
    }

    #[test]
    fn etag_weak_match_wildcard() {
        assert!(if_none_match_matches(&client("*"), &etag("W/\"anything\"")));
    }

    /// RFC 7232 §3.2: `If-None-Match` is a list. A cache holding
    /// several validators sends them comma-separated, and a match on
    /// any one is a match.
    #[test]
    fn comma_separated_list_matches_on_any_entry() {
        let headers = client("W/\"stale1\", W/\"abc123\", W/\"stale2\"");
        assert!(if_none_match_matches(&headers, &etag("W/\"abc123\"")));
    }

    #[test]
    fn comma_separated_list_without_the_server_tag_does_not_match() {
        let headers = client("W/\"stale1\", W/\"stale2\"");
        assert!(!if_none_match_matches(&headers, &etag("W/\"abc123\"")));
    }

    /// The same list may arrive as repeated header instances.
    #[test]
    fn repeated_header_instances_match_on_any_entry() {
        let mut headers = HeaderMap::new();
        headers.append(header::IF_NONE_MATCH, HeaderValue::from_static("W/\"x\""));
        headers.append(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("W/\"abc123\""),
        );
        assert!(if_none_match_matches(&headers, &etag("W/\"abc123\"")));
    }

    #[test]
    fn wildcard_anywhere_in_the_list_matches() {
        assert!(if_none_match_matches(
            &client("W/\"stale\", *"),
            &etag("W/\"abc123\"")
        ));
    }

    #[test]
    fn absent_header_does_not_match() {
        assert!(!if_none_match_matches(
            &HeaderMap::new(),
            &etag("W/\"abc123\"")
        ));
    }
}
