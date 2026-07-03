//! Typed error types for the gh-report application.

use thiserror::Error;

pub use cherry_pit_storage::PersistenceError;

/// Top-level error type for the gh-report application.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AppError {
    /// An error occurred during inventory collection or validation.
    #[error("inventory error: {0}")]
    Inventory(#[from] InventoryError),

    /// An error occurred communicating with the GitHub API.
    #[error("github api error: {0}")]
    GitHubApi(#[from] GitHubApiError),

    /// An error occurred during report rendering.
    #[error("report error: {0}")]
    Report(#[from] ReportError),

    /// An error occurred with persistence (checkpoints, evidence, publication).
    #[error("persistence error: {0}")]
    Persistence(#[from] PersistenceError),

    /// An error occurred with the web server.
    #[error("server error: {0}")]
    Server(#[from] ServerError),

    /// An error occurred with configuration.
    #[error("config error: {0}")]
    Config(#[from] ConfigError),
}

/// Errors related to the embedded web server.
///
/// gh-report does not match on individual server-error variants; the
/// upstream server error is collapsed to a single opaque
/// [`ServerError::Runtime`] value at the conversion boundary
/// ([`crate::app::daemon`]). This preserves the message for diagnostics
/// while keeping gh-report independent of the donor crate's error enum.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ServerError {
    /// Opaque runtime failure from the embedded HTTP server.
    ///
    /// Carries the upstream error's `Display` representation. Includes
    /// bind failures, address parse failures, and runtime errors —
    /// gh-report does not differentiate.
    #[error("{0}")]
    Runtime(String),
}

/// Errors related to repository inventory collection.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum InventoryError {
    #[error("unable to build inventory from GitHub API: {reason}")]
    ApiFetchFailed { reason: String },
}

/// Errors related to GitHub API communication.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GitHubApiError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("authentication failed: {reason}")]
    AuthenticationFailed { reason: String },

    #[error("authorization denied: {reason}")]
    AuthorizationDenied { reason: String },

    #[error("invalid response: {reason}")]
    InvalidResponse { reason: String },

    #[error("configuration error: {reason}")]
    ClientConfigError { reason: String },
}

/// Errors related to report generation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ReportError {
    #[error("template rendering failed: {reason}")]
    TemplateRenderFailed { reason: String },
}

/// Errors related to configuration.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
    #[error("missing required configuration: {field}")]
    MissingField { field: String },

    #[error("invalid configuration value: {field}: {reason}")]
    InvalidValue { field: String, reason: String },
}

#[must_use]
pub(crate) fn persist_error_variant(error: &PersistenceError) -> &'static str {
    match error {
        PersistenceError::LockFailed { .. } => "LockFailed",
        PersistenceError::AtomicWriteFailed { .. } => "AtomicWriteFailed",
        PersistenceError::LoadFailed { .. } => "LoadFailed",
        PersistenceError::TornWriteRecovery { .. } => "TornWriteRecovery",
        PersistenceError::FencedConflict { .. } => "FencedConflict",
        PersistenceError::Io(_) => "Io",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persist_error_variant_names_lock_failed() {
        let error = PersistenceError::LockFailed {
            reason: "x".to_string(),
        };
        assert_eq!(persist_error_variant(&error), "LockFailed");
    }

    #[test]
    fn persist_error_variant_names_atomic_write_failed() {
        let error = PersistenceError::AtomicWriteFailed {
            reason: "x".to_string(),
        };
        assert_eq!(persist_error_variant(&error), "AtomicWriteFailed");
    }

    #[test]
    fn persist_error_variant_names_load_failed() {
        let error = PersistenceError::LoadFailed {
            reason: "x".to_string(),
        };
        assert_eq!(persist_error_variant(&error), "LoadFailed");
    }

    #[test]
    fn persist_error_variant_names_torn_write_recovery() {
        let error = PersistenceError::TornWriteRecovery {
            source: Box::new(std::io::Error::other("x")),
        };
        assert_eq!(persist_error_variant(&error), "TornWriteRecovery");
    }

    #[test]
    fn persist_error_variant_names_fenced_conflict() {
        let error = PersistenceError::FencedConflict {
            source: Box::new(std::io::Error::other("x")),
        };
        assert_eq!(persist_error_variant(&error), "FencedConflict");
    }

    #[test]
    fn persist_error_variant_names_io() {
        let error = PersistenceError::Io(std::io::Error::other("x"));
        assert_eq!(persist_error_variant(&error), "Io");
    }
}
