//! Persistence error types for crash-safe file operations.

use thiserror::Error;

/// Errors related to persistence (checkpoints, evidence, locks, publishing).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PersistenceError {
    #[error("collection lock failed: {reason}")]
    LockFailed { reason: String },

    #[error("atomic write failed: {reason}")]
    AtomicWriteFailed { reason: String },

    /// Returned when a persistence file exists but cannot be parsed,
    /// has an unsupported schema version, or fails structural validation.
    /// Distinct from `Io` (which covers filesystem-level failures) and
    /// `AtomicWriteFailed` (which covers write-path errors).
    #[error("load failed: {reason}")]
    LoadFailed { reason: String },

    #[error("single-writer fence conflict: {source}")]
    FencedConflict {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
