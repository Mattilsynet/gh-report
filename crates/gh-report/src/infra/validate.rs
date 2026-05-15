//! Input validation utilities for untrusted data.
//!
//! Pure validation functions with no I/O or domain-specific error types.
//! Used to sanitize path segments from untrusted sources before URL
//! interpolation or filesystem operations.
//!
//! Absorbed under mission `absorb-helpers-1778694900` (P1-A.5.1).
//! Byte-for-byte port from prior upstream helpers.

use std::borrow::Cow;
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
/// # use gh_report::infra::validate::sanitize_path_segment;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
