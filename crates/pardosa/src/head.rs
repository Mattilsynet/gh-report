//! Cross-process `writer_head_seq` source for a separate serving
//! process (PGN-0027 R1, closing the GAP-1 deferral in PGN-0022 R3 /
//! PGN-0023 R8).
//!
//! [`WriterHeadReader`] is a typed read-side port (CHE-0075, PGN-0023
//! R7): it wraps a [`pardosa_nats::JetStreamHandle`] but exposes only
//! [`WriterHeadReader::writer_head_seq`] — no append surface exists on
//! this type, so constructing one confers no write capability
//! (R16: illegal states unrepresentable). The read is scoped strictly
//! to the read tier (R3/R4): it never touches, gates, or resyncs the
//! append path, and reading stream head dispatches no command and
//! authors no truth.

use pardosa_nats::JetStreamHandle;

use crate::backend::jetstream::map_runtime_error;
use crate::error::{BackendError, BackendOp};

/// Read-only observer of a `JetStream` stream's server-reported head
/// sequence, for a serving process that performs no local appends
/// (PGN-0027 R1). Never wraps append/sync capability.
pub struct WriterHeadReader {
    handle: JetStreamHandle,
}

impl WriterHeadReader {
    /// Wrap a [`pardosa_nats::JetStreamHandle`] as a head-only reader.
    ///
    /// Mirrors [`crate::store::JetStreamBackend::open`]'s opaque-wrapper
    /// shape but narrows the surface further: this type has no
    /// `append`/`sync` method at all, so the sync-facade invariant
    /// (PGN-0010 R5) and the non-writer constraint (PGN-0024 R4) hold
    /// by construction, not by convention.
    #[must_use]
    pub fn open(handle: JetStreamHandle) -> Self {
        Self { handle }
    }

    /// Read the `JetStream` stream head as the serving process's
    /// `writer_head_seq` (PGN-0027 R1; PGN-0023 R1's
    /// `writer_head_seq - projection_applied_seq` lag primitive).
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] when the underlying substrate read
    /// fails (connect, timeout, or replay-tier fetch failure); see
    /// [`pardosa_nats::JetStreamHandle::stream_head_seq`].
    pub fn writer_head_seq(&self) -> Result<u64, BackendError> {
        self.handle
            .stream_head_seq()
            .map_err(|source| map_runtime_error(source, BackendOp::Open))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detached_handle() -> JetStreamHandle {
        let cfg = pardosa_nats::JetStreamConfig::builder()
            .stream_name("gh-report-test")
            .subject("gh-report.test.events")
            .durable_consumer("gh-report-test")
            .nats_url("nats://127.0.0.1:4222")
            .runtime_handle(pardosa_nats::RuntimeHandle::detached_for_tests())
            .build()
            .expect("minimal config builds");
        pardosa_nats::JetStreamBackend::open(cfg)
    }

    #[test]
    fn detached_reader_writer_head_seq_returns_runtime_failure() {
        let reader = WriterHeadReader::open(detached_handle());

        let err = reader
            .writer_head_seq()
            .expect_err("detached handle cannot read a live stream head");

        assert!(matches!(err, BackendError::RuntimeFailure { .. }));
    }
}
