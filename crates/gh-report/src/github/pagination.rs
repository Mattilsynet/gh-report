//! Link header pagination parsing.
//!
//! Parses the `Link` header from HTTP responses to extract the next page URL
//! for paginated API responses.

use http::HeaderMap;

/// Extract the `next` URL from the `Link` header, if present.
///
/// # Security
///
/// This function returns the URL verbatim from the `Link` header. A malicious
/// or compromised server can return an arbitrary URL, potentially causing
/// server-side request forgery (SSRF) if the caller follows it blindly.
/// Use [`next_url_same_origin`] to validate that the returned URL shares the
/// same scheme and host as the original request.
#[must_use]
pub fn next_url(headers: &HeaderMap) -> Option<String> {
    let link_header = headers.get("link")?.to_str().ok()?;
    parse_next_from_link(link_header)
}

/// Extract the `next` URL from the `Link` header, validating that it shares
/// the same scheme and host as `original_url`.
///
/// Returns `None` if there is no next link or if the origin does not match.
#[must_use]
pub fn next_url_same_origin(headers: &HeaderMap, original_url: &str) -> Option<String> {
    let candidate = next_url(headers)?;
    if same_origin(&candidate, original_url) {
        Some(candidate)
    } else {
        None
    }
}

/// Check whether two URLs share the same scheme and host (origin).
///
/// Uses simple string parsing to avoid pulling in a URL crate.
/// Expects absolute URLs of the form `scheme://host[:port]/...`.
fn same_origin(a: &str, b: &str) -> bool {
    fn extract_origin(url: &str) -> Option<&str> {
        let sep = url.find("://")?;
        let after_scheme = &url[sep + 3..];
        // Origin ends at first `/` after scheme, or end of string.
        let end = after_scheme.find('/').unwrap_or(after_scheme.len());
        Some(&url[..sep + 3 + end])
    }
    match (extract_origin(a), extract_origin(b)) {
        (Some(oa), Some(ob)) => oa.eq_ignore_ascii_case(ob),
        _ => false,
    }
}

/// Parse the `next` rel URL from a Link header value.
///
/// Segments are delimited by `>` boundaries rather than commas, because
/// URLs may themselves contain commas (e.g., query parameters like `?q=a,b`).
///
/// The `rel` parameter is matched case-insensitively per RFC 8288.
fn parse_next_from_link(link_header: &str) -> Option<String> {
    // Each link-value is: `<URL>; param1; param2`, separated by commas.
    // But commas can appear inside `<URL>`. We split on `>, ` or `>,`
    // after the closing `>` — i.e., we find each `<...>; ...` segment
    // by scanning for `<` and `>` delimiters.
    let mut remaining = link_header;
    while !remaining.is_empty() {
        let start = remaining.find('<')?;
        let end = remaining[start..].find('>')? + start;
        let url = &remaining[start + 1..end];

        // The params follow after `>` until the next `<` or end-of-string.
        let params_start = end + 1;
        let params_end = remaining[params_start..]
            .find('<')
            .map_or(remaining.len(), |pos| params_start + pos);
        let params = &remaining[params_start..params_end];

        if params.to_ascii_lowercase().contains("rel=\"next\"") {
            return Some(url.to_string());
        }

        remaining = &remaining[params_end..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_next_url_from_link_header() {
        let header = r#"<https://api.github.com/orgs/test-org/repos?page=2>; rel="next", <https://api.github.com/orgs/test-org/repos?page=5>; rel="last""#;
        let next = parse_next_from_link(header);
        assert_eq!(
            next,
            Some("https://api.github.com/orgs/test-org/repos?page=2".to_string())
        );
    }

    #[test]
    fn parse_no_next_url() {
        let header = r#"<https://api.github.com/orgs/test-org/repos?page=1>; rel="prev""#;
        let next = parse_next_from_link(header);
        assert_eq!(next, None);
    }

    #[test]
    fn parse_empty_link_header() {
        let next = parse_next_from_link("");
        assert_eq!(next, None);
    }

    #[test]
    fn parse_comma_in_url() {
        let header = r#"<https://api.github.com/repos?q=a,b>; rel="next""#;
        let next = parse_next_from_link(header);
        assert_eq!(next, Some("https://api.github.com/repos?q=a,b".to_string()));
    }

    #[test]
    fn next_url_from_header_map() {
        use http::header::{HeaderMap, HeaderValue};
        let mut headers = HeaderMap::new();
        headers.insert(
            "link",
            HeaderValue::from_static(
                r#"<https://api.github.com/orgs/test/repos?page=3>; rel="next""#,
            ),
        );
        assert_eq!(
            next_url(&headers),
            Some("https://api.github.com/orgs/test/repos?page=3".to_string())
        );
    }

    #[test]
    fn case_insensitive_rel_matching() {
        let header = r#"<https://api.github.com/repos?page=2>; REL="next""#;
        assert_eq!(
            parse_next_from_link(header),
            Some("https://api.github.com/repos?page=2".to_string())
        );

        let header2 = r#"<https://api.github.com/repos?page=2>; Rel="Next""#;
        assert_eq!(
            parse_next_from_link(header2),
            Some("https://api.github.com/repos?page=2".to_string())
        );
    }

    #[test]
    fn same_origin_accepts_matching_host() {
        assert!(same_origin(
            "https://api.github.com/repos?page=2",
            "https://api.github.com/orgs/test"
        ));
    }

    #[test]
    fn same_origin_rejects_different_host() {
        assert!(!same_origin(
            "https://evil.com/repos?page=2",
            "https://api.github.com/orgs/test"
        ));
    }

    #[test]
    fn same_origin_rejects_different_scheme() {
        assert!(!same_origin(
            "http://api.github.com/repos?page=2",
            "https://api.github.com/orgs/test"
        ));
    }

    #[test]
    fn next_url_same_origin_filters_cross_origin() {
        use http::header::{HeaderMap, HeaderValue};
        let mut headers = HeaderMap::new();
        headers.insert(
            "link",
            HeaderValue::from_static(r#"<https://evil.com/repos?page=2>; rel="next""#),
        );
        assert_eq!(
            next_url_same_origin(&headers, "https://api.github.com/repos"),
            None
        );
    }
}
