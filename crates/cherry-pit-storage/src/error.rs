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

    /// Returned when a torn `.pgno` append could not be recovered to
    /// the last durable manifest checkpoint.
    #[error("torn-write recovery failed: {source}")]
    TornWriteRecovery {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    #[error("single-writer fence conflict: {source}")]
    FencedConflict {
        /// Sequence the caller expected; threaded from the
        /// `pardosa-fiber-store` concurrency-conflict variant. `None` when
        /// the lower ring did not populate it.
        expected_seq: Option<u64>,
        /// Broker-observed current sequence; threaded from the
        /// `pardosa-fiber-store` concurrency-conflict variant. `None` when
        /// the lower ring could not extract it.
        actual_seq: Option<u64>,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    /// Returned when the underlying store backend (e.g. NATS) is
    /// unreachable or otherwise infrastructurally unavailable. Transient:
    /// a retry may succeed once the backend recovers.
    #[error("backend unavailable: {reason}")]
    BackendUnavailable { reason: String },

    /// Returned when a store-level invariant (one-fiber-per-key) is
    /// violated. Structural: not retryable, indicates a bug or corrupted
    /// state upstream of this conversion.
    #[error("store invariant violated: {reason}")]
    InvariantViolation { reason: String },

    /// Returned when the underlying store's in-process mutex was
    /// poisoned by a panicking holder. Unrecoverable: the process must
    /// not continue operating on this store instance.
    #[error("store state poisoned")]
    PoisonedState,

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
