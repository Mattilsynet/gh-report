//! Generic result type for a single HTTP JSON API call.
//!
//! [`ApiOutcome`] carries no GitHub-specific vocabulary in its fields or
//! signatures; it is reusable for any HTTP API that returns JSON payloads
//! and communicates retryability via status code. See ADR COM-0014 (stable
//! interface for a slow-changing shared type) and CHE-0084:R1 (extraction
//! eligibility test) for the placement rationale.

use std::collections::HashMap;

/// Result of a single HTTP API request.
///
/// Uses an enum to make illegal states unrepresentable: a `Success` can never
/// carry an error message, and a `Failure` can never carry response data.
#[derive(Debug)]
#[non_exhaustive]
pub enum ApiOutcome {
    /// Successful API response (2xx status code).
    Success {
        status_code: u16,
        data: Option<serde_json::Value>,
        headers: Option<HashMap<String, String>>,
        truncated: bool,
    },
    /// Failed API response (non-2xx, timeout, network error, etc.).
    Failure {
        status_code: Option<u16>,
        error: String,
        retryable: bool,
    },
}

impl ApiOutcome {
    /// Whether this outcome represents a successful API call.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        matches!(self, ApiOutcome::Success { .. })
    }

    /// Whether this outcome represents a failed API call.
    #[must_use]
    pub fn is_err(&self) -> bool {
        !self.is_ok()
    }

    /// Return a reference to the response data, if this is a success with data.
    #[must_use]
    pub fn data(&self) -> Option<&serde_json::Value> {
        match self {
            ApiOutcome::Success { data, .. } => data.as_ref(),
            ApiOutcome::Failure { .. } => None,
        }
    }

    /// Return the HTTP status code, if available.
    #[must_use]
    pub fn status_code(&self) -> Option<u16> {
        match self {
            ApiOutcome::Success { status_code, .. } => Some(*status_code),
            ApiOutcome::Failure { status_code, .. } => *status_code,
        }
    }

    /// Return a reference to the captured response headers, if any.
    #[must_use]
    pub fn headers(&self) -> Option<&HashMap<String, String>> {
        match self {
            ApiOutcome::Success { headers, .. } => headers.as_ref(),
            ApiOutcome::Failure { .. } => None,
        }
    }

    /// Whether this failure is retryable (e.g., 429, 5xx, timeout).
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ApiOutcome::Failure {
                retryable: true,
                ..
            }
        )
    }

    /// Return the error message, if this is a failure.
    #[must_use]
    pub fn error_message(&self) -> Option<&str> {
        match self {
            ApiOutcome::Failure { error, .. } => Some(error.as_str()),
            ApiOutcome::Success { .. } => None,
        }
    }

    /// Create a successful result with JSON data.
    ///
    /// Sets `status_code` to `200` by convention — this constructor
    /// is not derived from an HTTP response. Use the enum variant directly
    /// when the actual HTTP status matters.
    #[must_use]
    pub fn success(data: serde_json::Value) -> Self {
        ApiOutcome::Success {
            status_code: 200,
            data: Some(data),
            headers: None,
            truncated: false,
        }
    }

    /// Whether a successful paginated response stopped before exhaustion.
    #[must_use]
    pub fn is_truncated(&self) -> bool {
        matches!(
            self,
            ApiOutcome::Success {
                truncated: true,
                ..
            }
        )
    }

    /// Create a failed result.
    ///
    /// When `retryable` is `true`, the caller's retry loop will attempt
    /// the request again (e.g., 429 rate limit, 5xx server error, timeout).
    #[must_use]
    pub fn failure(status_code: Option<u16>, error: String, retryable: bool) -> Self {
        ApiOutcome::Failure {
            status_code,
            error,
            retryable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ApiOutcome;

    #[test]
    fn api_outcome_is_err_for_failure() {
        let outcome = ApiOutcome::Failure {
            status_code: Some(500),
            error: "internal server error".to_string(),
            retryable: true,
        };
        assert!(outcome.is_err());
        assert!(!outcome.is_ok());
    }

    #[test]
    fn api_outcome_is_err_false_for_success() {
        let outcome = ApiOutcome::success(serde_json::json!({"ok": true}));
        assert!(!outcome.is_err());
        assert!(outcome.is_ok());
    }
}
