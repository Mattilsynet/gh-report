//! Link header pagination parsing.
//!
//! Parses the `Link` header from HTTP responses to extract the next page URL
//! for paginated API responses.

use http::HeaderMap;

/// The outcome of interpreting every `Link` field of one response.
///
/// `Uninterpretable` is distinct from [`NextLink::None`]: absence of a next
/// relation is a fact, whereas failure to interpret the header is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextLink {
    /// No `Link` field is present, or every field was interpreted and none
    /// carried a `next` relation.
    None,
    /// Exactly one distinct `next` destination was interpreted.
    One(String),
    /// The header could not be interpreted, or named more than one distinct
    /// `next` destination. Completeness cannot be concluded.
    Uninterpretable,
}

/// Interpret every `Link` field of a response into a [`NextLink`].
///
/// # Security
///
/// A returned URL comes verbatim from the server and is untrusted; the caller
/// must admit it against its own origin policy before following it.
#[must_use]
pub fn next_link(headers: &HeaderMap) -> NextLink {
    let mut targets: Vec<String> = Vec::new();
    for value in headers.get_all("link") {
        let Ok(text) = value.to_str() else {
            return NextLink::Uninterpretable;
        };
        match collect_next_targets(text, &mut targets) {
            Ok(()) => {}
            Err(Uninterpretable) => return NextLink::Uninterpretable,
        }
    }
    targets.dedup();
    match targets.len() {
        0 => NextLink::None,
        1 => NextLink::One(targets.remove(0)),
        _ => NextLink::Uninterpretable,
    }
}

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
    match next_link(headers) {
        NextLink::One(url) => Some(url),
        NextLink::None | NextLink::Uninterpretable => None,
    }
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
        let end = after_scheme.find('/').unwrap_or(after_scheme.len());
        Some(&url[..sep + 3 + end])
    }
    match (extract_origin(a), extract_origin(b)) {
        (Some(oa), Some(ob)) => oa.eq_ignore_ascii_case(ob),
        _ => false,
    }
}

struct Uninterpretable;

fn collect_next_targets(
    link_field: &str,
    targets: &mut Vec<String>,
) -> Result<(), Uninterpretable> {
    let mut remaining = link_field;
    loop {
        let head = remaining.trim_start_matches([' ', '\t', ',']);
        if head.is_empty() {
            return Ok(());
        }
        if !head.starts_with('<') {
            return Err(Uninterpretable);
        }
        let end = head.find('>').ok_or(Uninterpretable)?;
        let url = &head[1..end];

        let (params, rest) = scan_parameters(&head[end + 1..])?;
        if parameters_name_next(&params)? {
            targets.push(url.to_string());
        }

        remaining = rest;
    }
}

fn scan_parameters(input: &str) -> Result<(Vec<&str>, &str), Uninterpretable> {
    let bytes = input.as_bytes();
    let mut params = Vec::new();
    let mut idx = skip_whitespace(bytes, 0);
    while idx < bytes.len() {
        match bytes[idx] {
            b',' => return Ok((params, &input[idx..])),
            b';' => {
                let (param, next) = scan_one_parameter(input, idx + 1)?;
                params.push(param);
                idx = next;
            }
            _ => return Err(Uninterpretable),
        }
    }
    Ok((params, ""))
}

fn scan_one_parameter(input: &str, from: usize) -> Result<(&str, usize), Uninterpretable> {
    let bytes = input.as_bytes();
    let start = skip_whitespace(bytes, from);
    let mut idx = start;
    while idx < bytes.len() {
        match bytes[idx] {
            b';' | b',' => break,
            b'"' => idx = scan_quoted(bytes, idx)?,
            _ => idx += 1,
        }
    }
    Ok((input[start..idx].trim_end_matches([' ', '\t']), idx))
}

fn scan_quoted(bytes: &[u8], open: usize) -> Result<usize, Uninterpretable> {
    let mut idx = open + 1;
    while idx < bytes.len() {
        match bytes[idx] {
            b'\\' => idx += 2,
            b'"' => return Ok(idx + 1),
            _ => idx += 1,
        }
    }
    Err(Uninterpretable)
}

fn skip_whitespace(bytes: &[u8], from: usize) -> usize {
    let mut idx = from;
    while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
        idx += 1;
    }
    idx
}

fn parameters_name_next<'a>(params: &[&'a str]) -> Result<bool, Uninterpretable> {
    let mut relations: Option<Vec<&'a str>> = None;
    for param in params {
        let param: &'a str = param;
        let (name, value) = param.split_once('=').ok_or(Uninterpretable)?;
        let name = name.trim_end_matches([' ', '\t']);
        if !is_token(name) {
            return Err(Uninterpretable);
        }
        let value = parameter_value(value.trim_start_matches([' ', '\t']))?;
        if name.eq_ignore_ascii_case("rel") {
            if relations.is_some() {
                return Err(Uninterpretable);
            }
            relations = Some(relation_types(value)?);
        }
    }
    Ok(relations
        .is_some_and(|relations| relations.iter().any(|rel| rel.eq_ignore_ascii_case("next"))))
}

fn relation_types(value: &str) -> Result<Vec<&str>, Uninterpretable> {
    let types: Vec<&str> = value.split_whitespace().collect();
    if types.is_empty() || !types.iter().all(|rel| is_token(rel)) {
        return Err(Uninterpretable);
    }
    Ok(types)
}

fn parameter_value(value: &str) -> Result<&str, Uninterpretable> {
    let bytes = value.as_bytes();
    match bytes.first() {
        Some(b'"') => {
            let end = scan_quoted(bytes, 0)?;
            if end == bytes.len() {
                Ok(&value[1..end - 1])
            } else {
                Err(Uninterpretable)
            }
        }
        _ if is_token(value) => Ok(value),
        _ => Err(Uninterpretable),
    }
}

fn is_token(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link_headers(values: &[&str]) -> HeaderMap {
        use http::header::HeaderValue;
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(
                "link",
                HeaderValue::from_str(value).expect("test header is valid"),
            );
        }
        headers
    }

    fn interpret(value: &str) -> NextLink {
        next_link(&link_headers(&[value]))
    }

    #[test]
    fn parse_next_url_from_link_header() {
        let header = r#"<https://api.github.com/orgs/test-org/repos?page=2>; rel="next", <https://api.github.com/orgs/test-org/repos?page=5>; rel="last""#;
        assert_eq!(
            interpret(header),
            NextLink::One("https://api.github.com/orgs/test-org/repos?page=2".to_string())
        );
    }

    #[test]
    fn parse_no_next_url() {
        let header = r#"<https://api.github.com/orgs/test-org/repos?page=1>; rel="prev""#;
        assert_eq!(interpret(header), NextLink::None);
    }

    #[test]
    fn parse_empty_link_header() {
        assert_eq!(interpret(""), NextLink::None);
    }

    #[test]
    fn absent_link_header_is_genuine_completion() {
        assert_eq!(next_link(&HeaderMap::new()), NextLink::None);
    }

    #[test]
    fn parse_comma_in_url() {
        let header = r#"<https://api.github.com/repos?q=a,b>; rel="next""#;
        assert_eq!(
            interpret(header),
            NextLink::One("https://api.github.com/repos?q=a,b".to_string())
        );
    }

    #[test]
    fn missing_target_delimiter_is_uninterpretable() {
        let header = r#"<https://api.github.com/page2; rel="next""#;
        assert_eq!(interpret(header), NextLink::Uninterpretable);
    }

    #[test]
    fn unbracketed_link_value_is_uninterpretable() {
        assert_eq!(
            interpret(r#"https://api.github.com/page2; rel="next""#),
            NextLink::Uninterpretable
        );
    }

    #[test]
    fn unterminated_relation_quote_is_uninterpretable() {
        let header = r#"<https://api.github.com/page2>; rel="next"#;
        assert_eq!(interpret(header), NextLink::Uninterpretable);
    }

    #[test]
    fn unquoted_relation_is_supported() {
        assert_eq!(
            interpret("<https://api.github.com/p2>; rel=next"),
            NextLink::One("https://api.github.com/p2".to_string())
        );
    }

    #[test]
    fn multiple_relations_in_one_value_are_supported() {
        assert_eq!(
            interpret(r#"<https://api.github.com/p2>; rel="prev next""#),
            NextLink::One("https://api.github.com/p2".to_string())
        );
    }

    #[test]
    fn next_in_a_later_link_field_is_interpreted() {
        let headers = link_headers(&[
            r#"<https://api.github.com/p0>; rel="prev""#,
            r#"<https://api.github.com/p2>; rel="next""#,
        ]);
        assert_eq!(
            next_link(&headers),
            NextLink::One("https://api.github.com/p2".to_string())
        );
    }

    #[test]
    fn one_next_destination_repeated_is_not_ambiguous() {
        let headers = link_headers(&[
            r#"<https://api.github.com/p2>; rel="next""#,
            r#"<https://api.github.com/p2>; rel="next""#,
        ]);
        assert_eq!(
            next_link(&headers),
            NextLink::One("https://api.github.com/p2".to_string())
        );
    }

    #[test]
    fn distinct_next_destinations_are_uninterpretable() {
        let headers = link_headers(&[
            r#"<https://api.github.com/p2>; rel="next""#,
            r#"<https://api.github.com/p3>; rel="next""#,
        ]);
        assert_eq!(next_link(&headers), NextLink::Uninterpretable);
    }

    #[test]
    fn non_text_link_header_is_uninterpretable() {
        use http::header::HeaderValue;
        let mut headers = HeaderMap::new();
        headers.insert(
            "link",
            HeaderValue::from_bytes(b"<https://api.github.com/p2\xff>; rel=\"next\"")
                .expect("bytes are a valid header value"),
        );
        assert_eq!(next_link(&headers), NextLink::Uninterpretable);
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
        assert_eq!(
            interpret(r#"<https://api.github.com/repos?page=2>; REL="next""#),
            NextLink::One("https://api.github.com/repos?page=2".to_string())
        );
        assert_eq!(
            interpret(r#"<https://api.github.com/repos?page=2>; Rel="Next""#),
            NextLink::One("https://api.github.com/repos?page=2".to_string())
        );
    }

    #[test]
    fn parameter_without_value_is_uninterpretable() {
        assert_eq!(
            interpret(r#"<https://api.github.com/p2>; rel "next""#),
            NextLink::Uninterpretable
        );
    }

    #[test]
    fn empty_relation_value_is_uninterpretable() {
        assert_eq!(
            interpret("<https://api.github.com/p2>; rel="),
            NextLink::Uninterpretable
        );
    }

    #[test]
    fn unterminated_extension_quote_is_uninterpretable() {
        assert_eq!(
            interpret(r#"<https://api.github.com/p2>; title="unterminated; rel=next"#),
            NextLink::Uninterpretable
        );
    }

    #[test]
    fn trailing_garbage_after_link_value_is_uninterpretable() {
        assert_eq!(
            interpret("<https://api.github.com/p2>; rel=next, garbage"),
            NextLink::Uninterpretable
        );
    }

    #[test]
    fn missing_link_value_separator_is_uninterpretable() {
        assert_eq!(
            interpret(
                "<https://api.github.com/p1>; rel=prev <https://api.github.com/p2>; rel=next"
            ),
            NextLink::Uninterpretable
        );
    }

    #[test]
    fn quoted_title_is_not_parsed_as_parameters() {
        assert_eq!(
            interpret(r#"<https://api.github.com/p0>; title="a; rel=next; b"; rel=prev"#),
            NextLink::None
        );
    }

    #[test]
    fn quoted_title_containing_brackets_is_not_a_link_target() {
        assert_eq!(
            interpret(r#"<https://api.github.com/p0>; title="<note>"; rel=next"#),
            NextLink::One("https://api.github.com/p0".to_string())
        );
    }

    #[test]
    fn repeated_relation_parameter_is_uninterpretable() {
        assert_eq!(
            interpret("<https://api.github.com/p2>; rel=prev; rel=next"),
            NextLink::Uninterpretable
        );
    }

    #[test]
    fn valid_extension_parameters_are_preserved() {
        assert_eq!(
            interpret(
                r#"<https://api.github.com/p2>; title="page two"; type="text/html"; rel=next"#
            ),
            NextLink::One("https://api.github.com/p2".to_string())
        );
    }

    #[test]
    fn empty_quoted_relation_is_uninterpretable() {
        assert_eq!(
            interpret(r#"<https://api.github.com/p2>; rel="""#),
            NextLink::Uninterpretable
        );
    }

    #[test]
    fn whitespace_only_quoted_relation_is_uninterpretable() {
        assert_eq!(
            interpret(r#"<https://api.github.com/p2>; rel="   ""#),
            NextLink::Uninterpretable
        );
    }

    #[test]
    fn leftover_garbage_after_quoted_relation_is_uninterpretable() {
        assert_eq!(
            interpret(r#"<https://api.github.com/p2>; rel="prev" garbage "next""#),
            NextLink::Uninterpretable
        );
    }

    #[test]
    fn leftover_garbage_after_quoted_extension_is_uninterpretable() {
        assert_eq!(
            interpret(r#"<https://api.github.com/p0>; title="a" garbage "b"; rel=prev"#),
            NextLink::Uninterpretable
        );
    }

    #[test]
    fn empty_quoted_extension_value_is_valid() {
        assert_eq!(
            interpret(r#"<https://api.github.com/p2>; title=""; rel=next"#),
            NextLink::One("https://api.github.com/p2".to_string())
        );
    }

    #[test]
    fn escaped_quote_inside_title_stays_literal() {
        assert_eq!(
            interpret(r#"<https://api.github.com/p2>; title="a\"; rel=prev; b"; rel=next"#),
            NextLink::One("https://api.github.com/p2".to_string())
        );
    }

    #[test]
    fn empty_parameter_segment_is_uninterpretable() {
        assert_eq!(
            interpret("<https://api.github.com/p2>; ; rel=next"),
            NextLink::Uninterpretable
        );
        assert_eq!(
            interpret("<https://api.github.com/p2>; rel=next;"),
            NextLink::Uninterpretable
        );
    }

    #[test]
    fn non_token_relation_inside_quotes_is_uninterpretable() {
        assert_eq!(
            interpret(r#"<https://api.github.com/p2>; rel="ne\"xt""#),
            NextLink::Uninterpretable
        );
    }

    #[test]
    fn ordinary_terminal_controls_are_still_interpreted() {
        assert_eq!(
            interpret(r#"<https://api.github.com/p1>; rel="prev""#),
            NextLink::None
        );
        assert_eq!(
            interpret(r#"<https://api.github.com/p9>; rel="last""#),
            NextLink::None
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
