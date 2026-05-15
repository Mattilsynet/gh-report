use std::error::Error;
use std::fmt;
use std::num::NonZeroU64;
use std::path::PathBuf;

use crate::aggregate::{Aggregate, HandleCommand};
use crate::aggregate_id::AggregateId;
use crate::event::EventEnvelope;

/// Stable retry guidance for framework errors.
/// (CHE-0021 R3: `ErrorCategory` exposed on all error types;
/// CHE-0046: retry/timeout/cancellation semantics.)
///
/// This category is intentionally coarse. Callers use it to choose a
/// first response strategy without matching every concrete error variant:
/// retry after reloading/backing off, or stop and surface the condition
/// for domain/operator action.
///
/// # Examples
///
/// ```
/// use cherry_pit_core::ErrorCategory;
///
/// let cat = ErrorCategory::Retryable;
/// assert!(cat.is_retryable());
/// assert!(!cat.is_terminal());
///
/// let cat = ErrorCategory::Terminal;
/// assert!(cat.is_terminal());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorCategory {
    /// Repeating the operation may succeed after backoff, reload, or
    /// infrastructure recovery.
    Retryable,

    /// Repeating the same operation against the same state is expected
    /// to fail until input, domain state, or stored data is repaired.
    Terminal,
}

impl ErrorCategory {
    /// Returns true for errors where retry is a valid first response.
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(self, Self::Retryable)
    }

    /// Returns true for errors requiring caller/operator action before retry.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Terminal)
    }
}

/// Errors that can occur during command dispatch.
/// (CHE-0021 R1: `#[non_exhaustive]`; CHE-0015 R2: lossless domain error;
/// CHE-0019 R2: `AggregateNotFound` lives here, not on `StoreError`.)
///
/// Generic over `E` — the domain-specific error type from
/// `HandleCommand<C>::Error`. This preserves full type information
/// through the gateway and bus, allowing callers to match on
/// domain errors without downcasting.
#[derive(Debug)]
#[non_exhaustive]
pub enum DispatchError<E: Error + Send + Sync> {
    /// The aggregate rejected the command (business invariant violation).
    Rejected(E),

    /// No events exist for this aggregate — it has never been created.
    AggregateNotFound { aggregate_id: AggregateId },

    /// Another command was persisted against this aggregate between our
    /// load and our persist. The caller may retry.
    ConcurrencyConflict {
        aggregate_id: AggregateId,
        /// Sequence the caller required for the next write. This comes
        /// from `EventEnvelope::sequence()` (or the equivalent counter)
        /// upstream, so it is always `NonZeroU64`.
        expected_sequence: NonZeroU64,
        /// Sequence actually observed in the store at the time of the
        /// write. May be `0` when the stream is empty (no events yet),
        /// hence raw `u64`.
        actual_sequence: u64,
    },

    /// Infrastructure failure (event store unavailable, serialization
    /// error, transport timeout, etc.).
    Infrastructure(Box<dyn Error + Send + Sync>),
}

impl<E: Error + Send + Sync> DispatchError<E> {
    /// Classify the dispatch failure as retryable or terminal.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        match self {
            Self::ConcurrencyConflict { .. } | Self::Infrastructure(_) => ErrorCategory::Retryable,
            Self::Rejected(_) | Self::AggregateNotFound { .. } => ErrorCategory::Terminal,
        }
    }
}

impl<E: Error + Send + Sync> fmt::Display for DispatchError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(e) => write!(f, "command rejected: {e}"),
            Self::AggregateNotFound { aggregate_id } => {
                write!(f, "aggregate not found: {aggregate_id}")
            }
            Self::ConcurrencyConflict {
                aggregate_id,
                expected_sequence,
                actual_sequence,
            } => write!(
                f,
                "concurrency conflict on {aggregate_id}: expected sequence {expected_sequence}, actual {actual_sequence}"
            ),
            Self::Infrastructure(e) => write!(f, "infrastructure error: {e}"),
        }
    }
}

impl<E: Error + Send + Sync + 'static> Error for DispatchError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Rejected(e) => Some(e),
            Self::Infrastructure(e) => Some(e.as_ref()),
            Self::AggregateNotFound { .. } | Self::ConcurrencyConflict { .. } => None,
        }
    }
}

/// Errors from event store operations.
/// (CHE-0021 R1: `#[non_exhaustive]`; CHE-0027 R1: manual Display/Error,
/// no thiserror dependency.)
#[derive(Debug)]
#[non_exhaustive]
pub enum StoreError {
    /// Optimistic concurrency violation — another writer persisted
    /// events after our load.
    ConcurrencyConflict {
        aggregate_id: AggregateId,
        /// Sequence the caller required for the next write. Always
        /// `NonZeroU64` — propagated from `EventEnvelope::sequence()`.
        expected_sequence: NonZeroU64,
        /// Sequence actually observed in the store at the time of the
        /// write. May be `0` when the stream is empty, hence raw `u64`.
        actual_sequence: u64,
    },

    /// The store directory is locked by another process.
    ///
    /// Returned when a file-based store cannot acquire an exclusive
    /// advisory lock on its directory. This indicates another process
    /// is already using the same store directory, which violates the
    /// single-writer assumption (CHE-0006).
    StoreLocked {
        /// The path to the store directory that is locked.
        path: PathBuf,
    },

    /// Persisted data failed structural or semantic validation.
    ///
    /// This includes malformed bytes, invalid envelopes, aggregate ID
    /// mismatches, sequence gaps, duplicates, and out-of-order events.
    /// Retrying the same read is not expected to succeed until the store
    /// is repaired or restored from backup.
    CorruptData(Box<dyn Error + Send + Sync>),

    /// Infrastructure failure (disk I/O, network, serialization).
    Infrastructure(Box<dyn Error + Send + Sync>),
}

impl StoreError {
    /// Classify the store failure as retryable or terminal.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        match self {
            Self::ConcurrencyConflict { .. }
            | Self::StoreLocked { .. }
            | Self::Infrastructure(_) => ErrorCategory::Retryable,
            Self::CorruptData(_) => ErrorCategory::Terminal,
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConcurrencyConflict {
                aggregate_id,
                expected_sequence,
                actual_sequence,
            } => write!(
                f,
                "concurrency conflict on {aggregate_id}: expected sequence {expected_sequence}, actual {actual_sequence}"
            ),
            Self::StoreLocked { path } => write!(
                f,
                "store directory is locked by another process: {}",
                path.display()
            ),
            Self::CorruptData(e) => write!(f, "store corrupt data: {e}"),
            Self::Infrastructure(e) => write!(f, "store infrastructure error: {e}"),
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CorruptData(e) | Self::Infrastructure(e) => Some(e.as_ref()),
            Self::ConcurrencyConflict { .. } | Self::StoreLocked { .. } => None,
        }
    }
}

/// Error from event bus publication.
/// (CHE-0021 R1: `#[non_exhaustive]`; CHE-0027 R1: manual Display/Error.)
///
/// Intentionally simple — publication errors are infrastructure-level.
/// The `CommandBus` may log this error but does not propagate it as a
/// `DispatchError` — the command already succeeded (events are persisted).
#[derive(Debug)]
#[non_exhaustive]
pub struct BusError(Box<dyn Error + Send + Sync>);

impl BusError {
    /// Wrap an infrastructure error as a bus error.
    pub fn new(source: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self(source.into())
    }

    /// Consume the error and return the underlying cause.
    #[must_use]
    pub fn into_inner(self) -> Box<dyn Error + Send + Sync> {
        self.0
    }

    /// Classify event publication failure as retryable.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        ErrorCategory::Retryable
    }
}

impl fmt::Display for BusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "event bus error: {}", self.0)
    }
}

impl Error for BusError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

/// Errors from `EventEnvelope` construction or validation.
/// (CHE-0021 R1: `#[non_exhaustive]`; CHE-0042 R1–R4: validated
/// construction and stream integrity checks.)
///
/// Only one variant: `NilEventId`. Sequence validity is guaranteed
/// by `NonZeroU64` — the type system eliminates zero sequences at
/// compile time, and serde rejects zero on deserialization.
#[derive(Debug)]
#[non_exhaustive]
pub enum EnvelopeError {
    /// The `event_id` is nil (`Uuid::nil()`), which indicates a
    /// missing or corrupted event identifier.
    NilEventId,

    /// The envelope belongs to a different aggregate stream than the
    /// file or store partition being loaded.
    AggregateIdMismatch {
        /// Aggregate ID expected from the stream key.
        expected: AggregateId,
        /// Aggregate ID found in the envelope.
        actual: AggregateId,
    },

    /// The stream sequence is not exactly contiguous from 1..=N.
    SequenceGap {
        /// Sequence required at this stream position. This is a 1-based
        /// loop counter (`u64` to keep arithmetic simple at the call
        /// site); always ≥ 1 in practice.
        expected_sequence: u64,
        /// Sequence found in the envelope. Always `NonZeroU64` because
        /// it comes directly from `EventEnvelope::sequence()`.
        actual_sequence: NonZeroU64,
    },
}

impl EnvelopeError {
    /// Envelope validation failures indicate corrupt or malformed data.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        ErrorCategory::Terminal
    }
}

impl fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NilEventId => write!(f, "event_id must not be nil"),
            Self::AggregateIdMismatch { expected, actual } => write!(
                f,
                "event aggregate_id mismatch: expected {expected}, actual {actual}"
            ),
            Self::SequenceGap {
                expected_sequence,
                actual_sequence,
            } => write!(
                f,
                "event sequence gap: expected sequence {expected_sequence}, actual {actual_sequence}"
            ),
        }
    }
}

impl Error for EnvelopeError {}

/// Result type for command dispatch through the bus or gateway.
///
/// Returns the event envelopes produced and persisted on success.
/// Envelopes contain full metadata: `event_id`, `aggregate_id`,
/// `sequence`, and `timestamp` alongside the domain event payload.
pub type DispatchResult<A, C> = Result<
    Vec<EventEnvelope<<A as Aggregate>::Event>>,
    DispatchError<<A as HandleCommand<C>>::Error>,
>;

/// Result type for aggregate creation through the bus or gateway.
///
/// Returns the store-assigned [`AggregateId`] and the event envelopes
/// produced by the aggregate on success.
pub type CreateResult<A, C> = Result<
    (AggregateId, Vec<EventEnvelope<<A as Aggregate>::Event>>),
    DispatchError<<A as HandleCommand<C>>::Error>,
>;

/// Result type for [`EventStore::create`](crate::EventStore::create).
///
/// Returns the store-assigned [`AggregateId`] and the persisted event
/// envelopes on success.
pub type StoreCreateResult<E> = Result<(AggregateId, Vec<EventEnvelope<E>>), StoreError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU64;

    // A minimal domain error for testing DispatchError<E>.
    #[derive(Debug)]
    struct TestDomainError(&'static str);
    impl fmt::Display for TestDomainError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.0)
        }
    }
    impl Error for TestDomainError {}

    // ── CHE-0021 R1: non_exhaustive on all public error types ──

    #[test]
    fn dispatch_error_is_non_exhaustive() {
        // This test verifies the enum is #[non_exhaustive] by confirming
        // we can construct known variants. The non_exhaustive attribute
        // prevents external crates from exhaustive matching — verified
        // at the language level (attribute presence checked by m22 in
        // adt_obligations).
        let _err: DispatchError<TestDomainError> = DispatchError::AggregateNotFound {
            aggregate_id: AggregateId::new(NonZeroU64::new(1).unwrap()),
        };
    }

    // ── CHE-0021 R3: ErrorCategory on all error types ──

    #[test]
    fn dispatch_error_category_retryable() {
        let id = AggregateId::new(NonZeroU64::new(1).unwrap());
        let err: DispatchError<TestDomainError> = DispatchError::ConcurrencyConflict {
            aggregate_id: id,
            expected_sequence: NonZeroU64::new(1).unwrap(),
            actual_sequence: 2,
        };
        assert_eq!(err.category(), ErrorCategory::Retryable);

        let err2: DispatchError<TestDomainError> = DispatchError::Infrastructure("io".into());
        assert_eq!(err2.category(), ErrorCategory::Retryable);
    }

    #[test]
    fn dispatch_error_category_terminal() {
        let err: DispatchError<TestDomainError> = DispatchError::Rejected(TestDomainError("nope"));
        assert_eq!(err.category(), ErrorCategory::Terminal);

        let err2: DispatchError<TestDomainError> = DispatchError::AggregateNotFound {
            aggregate_id: AggregateId::new(NonZeroU64::new(1).unwrap()),
        };
        assert_eq!(err2.category(), ErrorCategory::Terminal);
    }

    #[test]
    fn store_error_category_retryable() {
        let id = AggregateId::new(NonZeroU64::new(1).unwrap());
        assert_eq!(
            StoreError::ConcurrencyConflict {
                aggregate_id: id,
                expected_sequence: NonZeroU64::new(1).unwrap(),
                actual_sequence: 2,
            }
            .category(),
            ErrorCategory::Retryable
        );
        assert_eq!(
            StoreError::StoreLocked {
                path: "/tmp".into()
            }
            .category(),
            ErrorCategory::Retryable
        );
        assert_eq!(
            StoreError::Infrastructure("io".into()).category(),
            ErrorCategory::Retryable
        );
    }

    #[test]
    fn store_error_category_terminal() {
        assert_eq!(
            StoreError::CorruptData("bad".into()).category(),
            ErrorCategory::Terminal
        );
    }

    #[test]
    fn bus_error_category_always_retryable() {
        let err = BusError::new("network");
        assert_eq!(err.category(), ErrorCategory::Retryable);
    }

    #[test]
    fn envelope_error_category_always_terminal() {
        assert_eq!(
            EnvelopeError::NilEventId.category(),
            ErrorCategory::Terminal
        );
        assert_eq!(
            EnvelopeError::AggregateIdMismatch {
                expected: AggregateId::new(NonZeroU64::new(1).unwrap()),
                actual: AggregateId::new(NonZeroU64::new(2).unwrap()),
            }
            .category(),
            ErrorCategory::Terminal
        );
        assert_eq!(
            EnvelopeError::SequenceGap {
                expected_sequence: 1,
                actual_sequence: NonZeroU64::new(3).unwrap(),
            }
            .category(),
            ErrorCategory::Terminal
        );
    }

    // ── CHE-0027 R1: manual Display/Error (no thiserror) ──

    #[test]
    fn dispatch_error_display() {
        let err: DispatchError<TestDomainError> =
            DispatchError::Rejected(TestDomainError("invariant violated"));
        assert_eq!(err.to_string(), "command rejected: invariant violated");

        let err2: DispatchError<TestDomainError> = DispatchError::AggregateNotFound {
            aggregate_id: AggregateId::new(NonZeroU64::new(42).unwrap()),
        };
        assert_eq!(err2.to_string(), "aggregate not found: 42");
    }

    #[test]
    fn store_error_display() {
        let err = StoreError::StoreLocked {
            path: "/data/store".into(),
        };
        assert!(err.to_string().contains("/data/store"));

        let err2 = StoreError::CorruptData("bad checksum".into());
        assert!(err2.to_string().contains("bad checksum"));
    }

    #[test]
    fn bus_error_display_and_source() {
        let err = BusError::new("connection reset");
        assert!(err.to_string().contains("connection reset"));
        assert!(err.source().is_some());
    }

    #[test]
    fn envelope_error_display() {
        assert_eq!(
            EnvelopeError::NilEventId.to_string(),
            "event_id must not be nil"
        );
    }

    #[test]
    fn dispatch_error_source() {
        let err: DispatchError<TestDomainError> = DispatchError::Rejected(TestDomainError("x"));
        assert!(err.source().is_some());

        let err2: DispatchError<TestDomainError> = DispatchError::AggregateNotFound {
            aggregate_id: AggregateId::new(NonZeroU64::new(1).unwrap()),
        };
        assert!(err2.source().is_none());
    }

    // ── CHE-0015 R2: domain error preserved losslessly ──

    #[test]
    fn dispatch_error_preserves_domain_error() {
        let domain_err = TestDomainError("business rule X");
        let err: DispatchError<TestDomainError> = DispatchError::Rejected(domain_err);
        // Access the inner error through pattern matching — type preserved.
        if let DispatchError::Rejected(inner) = &err {
            assert_eq!(inner.0, "business rule X");
        } else {
            panic!("expected Rejected variant");
        }
    }

    // ── BusError into_inner ──

    #[test]
    fn bus_error_into_inner() {
        let err = BusError::new("timeout");
        let inner = err.into_inner();
        assert_eq!(inner.to_string(), "timeout");
    }
}
