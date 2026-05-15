//! Path validation and normalisation utilities (CHE-0049 R8 transport
//! helpers).
//!
//! Pure validation/normalisation functions with no I/O or domain-specific
//! error types. Used to sanitise path segments from untrusted sources
//! before URL interpolation or cache-key lookup, and to normalise inbound
//! request URI paths into safe cache lookup keys.
//!
//! Ported byte-for-byte from the donor crate per CHE-0049 R14; donor copies
//! remain in that crate until the gh-report migration completes.

use std::borrow::Cow;

use percent_encoding::percent_decode_str;
use thiserror::Error;

/// Error returned when a path segment fails validation.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{field}: {reason}")]
#[non_exhaustive]
pub struct PathSegmentError {
    /// The field name that failed validation (e.g., `"repo_name"`).
    pub field: String,
    /// Human-readable reason for rejection.
    pub reason: String,
}

impl PathSegmentError {
    /// Create a new `PathSegmentError`.
    #[must_use]
    pub fn new(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            reason: reason.into(),
        }
    }
}

/// Validate and sanitize a path segment from untrusted data.
///
/// Rejects values containing path traversal sequences (`..`), slashes,
/// backslashes, control characters, or empty strings that could cause
/// requests to hit unintended endpoints.
///
/// Returns `Cow::Borrowed` when no transformation is needed, avoiding
/// allocation on the happy path.
///
/// # Errors
///
/// Returns [`PathSegmentError`] describing the validation failure.
///
/// # Examples
///
/// ```
/// # use cherry_pit_web::sanitize_path_segment;
/// assert_eq!(sanitize_path_segment("main", "branch").unwrap(), "main");
/// assert!(sanitize_path_segment("../etc/passwd", "branch").is_err());
/// assert!(sanitize_path_segment("", "branch").is_err());
/// ```
pub fn sanitize_path_segment<'a>(
    value: &'a str,
    field_name: &str,
) -> Result<Cow<'a, str>, PathSegmentError> {
    if value.is_empty() {
        return Err(PathSegmentError {
            field: field_name.to_string(),
            reason: format!("{field_name} is empty"),
        });
    }
    if value.contains("..") || value.contains('/') || value.contains('\\') {
        // Truncate untrusted input to prevent log/error amplification.
        let truncated: String = value.chars().take(100).collect();
        return Err(PathSegmentError {
            field: field_name.to_string(),
            reason: format!("{field_name} contains invalid path characters: {truncated}"),
        });
    }
    if value.chars().any(char::is_control) {
        return Err(PathSegmentError {
            field: field_name.to_string(),
            reason: format!("{field_name} contains control characters"),
        });
    }
    Ok(Cow::Borrowed(value))
}

// ===========================================================================
// Request path normalisation (security-critical)
// ===========================================================================

/// Result of normalising a raw URI path.
///
/// Carries the clean cache lookup key and whether the original path had
/// a trailing slash — needed to choose the correct fallback strategy.
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedPath {
    /// Clean cache key (e.g., `"about"`, `"index.html"`, `"blog/post"`).
    pub key: String,
    /// Whether the original request path ended with `/`.
    pub has_trailing_slash: bool,
}

/// Normalise a raw URI path into a safe cache lookup key.
///
/// Returns `None` (→ 400) if the path contains traversal sequences, null
/// bytes, backslashes, or any component that resolves to `..` after
/// percent-decoding.
///
/// # Algorithm
///
/// 1. Percent-decode the raw path (handles `%2e%2e`, `%2f`, etc.).
/// 2. Reject null bytes (`\0`), backslashes (`\`).
/// 3. Detect trailing slash before filtering (empty final segment).
/// 4. Split on `/`, skip empty components (collapses `//`), reject `..`.
/// 5. Re-join with `/` — the result is a clean relative path suitable
///    for `HashMap` lookup.
/// 6. Map empty result (root `/`) to `index.html`.
///
/// **Double-encoding resilience:** we decode only once. A doubly-encoded
/// `%252e%252e` decodes to the literal `%2e%2e` which contains no `/` or
/// `.` path separators, so it becomes a harmless (non-existent) cache key.
#[must_use]
pub fn normalize_request_path(raw: &str) -> Option<NormalizedPath> {
    // Step 1: percent-decode.
    let decoded = percent_decode_str(raw).decode_utf8().ok()?;

    // Step 2: reject dangerous bytes.
    if decoded.contains('\0') || decoded.contains('\\') {
        return None;
    }

    // Step 3: detect trailing slash before filtering.
    let has_trailing_slash = decoded.ends_with('/') && decoded.len() > 1;

    // Step 4: split, filter empties, reject traversal.
    let mut segments: Vec<&str> = Vec::new();
    for seg in decoded.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        // Defense-in-depth: reject any segment containing ".." — not just
        // exact-match ".." — to prevent bypass via prefixed/suffixed variants
        // like "\x08.." that survive the exact check.
        if seg.contains("..") {
            return None; // traversal attempt
        }
        segments.push(seg);
    }

    // Step 5: re-join.
    let joined = segments.join("/");

    // Step 6: root → index.html.
    if joined.is_empty() {
        Some(NormalizedPath {
            key: "index.html".to_string(),
            has_trailing_slash: true,
        })
    } else {
        Some(NormalizedPath {
            key: joined,
            has_trailing_slash,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── sanitize_path_segment ───────────────────────────────────

    #[test]
    fn accepts_normal_values() {
        assert_eq!(
            sanitize_path_segment("main", "default_branch").unwrap(),
            "main"
        );
        assert_eq!(
            sanitize_path_segment("release-v2.1", "default_branch").unwrap(),
            "release-v2.1"
        );
    }

    #[test]
    fn rejects_traversal() {
        assert!(sanitize_path_segment("../etc/passwd", "default_branch").is_err());
        assert!(sanitize_path_segment("foo/../bar", "default_branch").is_err());
    }

    #[test]
    fn rejects_slashes() {
        assert!(sanitize_path_segment("feature/branch", "default_branch").is_err());
        assert!(sanitize_path_segment("a\\b", "default_branch").is_err());
    }

    #[test]
    fn rejects_empty() {
        assert!(sanitize_path_segment("", "default_branch").is_err());
    }

    #[test]
    fn rejects_control_chars() {
        assert!(sanitize_path_segment("foo\x00bar", "repo_name").is_err());
        assert!(sanitize_path_segment("foo\nbar", "repo_name").is_err());
        assert!(sanitize_path_segment("foo\rbar", "repo_name").is_err());
    }

    #[test]
    fn error_display_includes_field_and_reason() {
        let err = sanitize_path_segment("", "repo_name").unwrap_err();
        let display = err.to_string();
        assert!(display.contains("repo_name"));
    }

    // ── normalize_request_path ──────────────────────────────────

    #[test]
    fn normalize_root_to_index() {
        let result = normalize_request_path("/").unwrap();
        assert_eq!(result.key, "index.html");
        assert!(result.has_trailing_slash);
    }

    #[test]
    fn normalize_simple_path() {
        let result = normalize_request_path("/report.html").unwrap();
        assert_eq!(result.key, "report.html");
        assert!(!result.has_trailing_slash);
    }

    #[test]
    fn normalize_nested_path() {
        let result = normalize_request_path("/owners/acme.html").unwrap();
        assert_eq!(result.key, "owners/acme.html");
        assert!(!result.has_trailing_slash);
    }

    #[test]
    fn normalize_collapses_double_slashes() {
        assert_eq!(
            normalize_request_path("//report.html").unwrap().key,
            "report.html"
        );
        assert_eq!(
            normalize_request_path("/owners///acme.html").unwrap().key,
            "owners/acme.html"
        );
    }

    #[test]
    fn normalize_strips_dot_segments() {
        assert_eq!(
            normalize_request_path("/./report.html").unwrap().key,
            "report.html"
        );
    }

    #[test]
    fn normalize_rejects_dotdot() {
        assert_eq!(normalize_request_path("/../secret.txt"), None);
        assert_eq!(normalize_request_path("/foo/../bar"), None);
        assert_eq!(normalize_request_path("/.."), None);
    }

    #[test]
    fn normalize_rejects_encoded_dotdot() {
        // %2e = '.', %2f = '/'
        assert_eq!(normalize_request_path("/%2e%2e/secret.txt"), None);
        assert_eq!(normalize_request_path("/%2e%2e%2fsecret.txt"), None);
    }

    #[test]
    fn normalize_rejects_null_byte() {
        assert_eq!(normalize_request_path("/foo%00bar"), None);
    }

    #[test]
    fn normalize_rejects_backslash() {
        assert_eq!(normalize_request_path("/foo%5Cbar"), None);
        assert_eq!(normalize_request_path("/foo\\bar"), None);
    }

    #[test]
    fn normalize_double_encoded_is_harmless() {
        // %252e%252e → decodes to literal "%2e%2e" (no dots), safe cache key.
        let result = normalize_request_path("/%252e%252e/secret.txt").unwrap();
        assert!(!result.key.contains(".."));
    }

    #[test]
    fn normalize_empty_path() {
        let result = normalize_request_path("").unwrap();
        assert_eq!(result.key, "index.html");
    }

    #[test]
    fn normalize_rejects_invalid_utf8() {
        // %FF is not valid UTF-8 start byte in isolation.
        assert_eq!(normalize_request_path("/%FF"), None);
    }

    // ── NormalizedPath trailing-slash discrimination (Phase 4b' port) ──

    #[test]
    fn normalized_path_trailing_slash_about() {
        let result = normalize_request_path("/about/").unwrap();
        assert_eq!(result.key, "about");
        assert!(result.has_trailing_slash);
    }

    #[test]
    fn normalized_path_no_trailing_slash_about() {
        let result = normalize_request_path("/about").unwrap();
        assert_eq!(result.key, "about");
        assert!(!result.has_trailing_slash);
    }
}
