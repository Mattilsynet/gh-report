use crate::event::{DomainId, Index};
use crate::fiber_state::{FiberAction, FiberState};

/// All errors produced by pardosa operations.
#[derive(Debug, thiserror::Error)]
pub enum PardosaError {
    // State machine
    #[error("invalid transition: state {state:?} + action {action:?}")]
    InvalidTransition {
        state: FiberState,
        action: FiberAction,
    },

    // Fiber integrity
    #[error("fiber invariant violation: {0}")]
    FiberInvariantViolation(String),

    // Identity
    #[error("domain ID {0:?} is not in Purged state — cannot reuse")]
    IdNotPurged(DomainId),

    #[error("domain ID {0:?} already exists")]
    IdAlreadyExists(DomainId),

    #[error("fiber not found for domain ID {0:?}")]
    FiberNotFound(DomainId),

    #[error("index overflow")]
    IndexOverflow,

    #[error("domain ID counter overflow")]
    DomainIdOverflow,

    #[error("event ID counter overflow")]
    EventIdOverflow,

    // Server state
    #[error("migration in progress — application operations rejected")]
    MigrationInProgress,

    // Persistence
    #[error("NATS connection unavailable")]
    NatsUnavailable,

    #[error("serialization failed: {0}")]
    SerializationFailed(String),

    #[error("deserialization failed: {0}")]
    DeserializationFailed(String),

    // Migration
    #[error("migration failed: {0}")]
    MigrationFailed(String),

    #[error("generation {requested} is not greater than current {current}")]
    InvalidGeneration { current: u64, requested: u64 },

    #[error("detached fiber {0:?} has no migration policy")]
    MissingMigrationPolicy(DomainId),

    // Registry
    #[error("stream not found in registry: {0}")]
    StreamNotFound(String),

    #[error("registry unavailable")]
    RegistryUnavailable,

    #[error(
        "registry CAS conflict on key {key}: expected revision {expected_revision}, actual {actual_revision}"
    )]
    RegistryConflict {
        key: String,
        expected_revision: u64,
        actual_revision: u64,
    },

    #[error("schema mismatch: expected {expected}, got {actual}")]
    SchemaMismatch { expected: String, actual: String },

    // Precursor integrity
    #[error(
        "precursor chain broken at event_id {event_id}: precursor index {precursor:?} not found"
    )]
    BrokenPrecursorChain { event_id: u64, precursor: Index },
}
