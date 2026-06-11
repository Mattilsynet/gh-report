use std::error::Error;
use std::fmt;
/// Typed errors surfaced by
/// [`crate::JetStreamConfigBuilder::build`].
///
/// `#[non_exhaustive]` per ADR-0007 §error-taxonomy so future
/// validation clauses (TTL forbidden, compression rejected, …)
/// can land without a breaking change.
#[derive(Debug)]
#[non_exhaustive]
pub enum JetStreamConfigError {
    /// `stream_name` was missing or empty.
    EmptyStreamName,
    /// `subject` was missing or empty.
    EmptySubject,
    /// `subject` contained a NATS wildcard token; the
    /// authoritative stream binds to a *single* subject per
    /// Phase 1.5 §7.
    SubjectContainsWildcard {
        /// The offending wildcard character (`*` or `>`).
        wildcard: char,
    },
    /// `durable_consumer` was missing or empty.
    EmptyDurableConsumer,
    /// Caller selected [`crate::Discard::Old`], which Phase 1.5
    /// §7.3 forbids in v0 (would lose authoritative-log events).
    DiscardOldForbidden,
    /// Caller set replicas to `0`; Phase 1.5 §7.3 requires
    /// `R ≥ 1`.
    ReplicasMustBePositive,
    /// No runtime handle was supplied; ADR-0022 §D7 forbids a
    /// global / implicit default.
    MissingRuntimeHandle,
    /// Operation timeout must be greater than zero seconds.
    OperationTimeoutMustBePositive,
    /// Operation timeout environment override could not be parsed as
    /// a positive integer second count.
    InvalidOperationTimeout {
        /// Raw value supplied by the environment.
        value: String,
    },
}
impl fmt::Display for JetStreamConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyStreamName => f.write_str("stream_name must be non-empty"),
            Self::EmptySubject => f.write_str("subject must be non-empty"),
            Self::SubjectContainsWildcard { wildcard } => {
                write!(
                    f,
                    "subject must not contain NATS wildcard '{wildcard}' (Phase 1.5 §7 single-subject layout)"
                )
            }
            Self::EmptyDurableConsumer => f.write_str("durable_consumer must be non-empty"),
            Self::DiscardOldForbidden => {
                f.write_str("Discard::Old is forbidden in v0 (Phase 1.5 §7.3); use Discard::New")
            }
            Self::ReplicasMustBePositive => {
                f.write_str("replicas must be >= 1 (Phase 1.5 §7.3 R >= 1)")
            }
            Self::MissingRuntimeHandle => {
                f.write_str("runtime_handle must be supplied (ADR-0022 §D7 — no implicit default)")
            }
            Self::OperationTimeoutMustBePositive => {
                f.write_str("operation_timeout must be greater than zero")
            }
            Self::InvalidOperationTimeout { value } => {
                write!(f, "operation timeout override must be a positive integer second count: {value:?}")
            }
        }
    }
}
impl Error for JetStreamConfigError {}
/// Typed errors surfaced by [`crate::JetStreamHandle::append`] and
/// [`crate::JetStreamHandle::sync`].
///
/// `#[non_exhaustive]` per ADR-0007 §error-taxonomy so additional
/// failure modes (connection-lost, message-too-large, …) can land
/// without a breaking change. The in-crate adapter shim inside
/// `pardosa` maps each variant onto the runtime's
/// `pardosa::store::BackendError` taxonomy at the sealed-trait
/// boundary (ADR-0022 §D7 / §D11).
#[derive(Debug)]
#[non_exhaustive]
pub enum JetStreamRuntimeError {
    /// The handle was constructed with
    /// [`crate::RuntimeHandle::detached_for_tests`] and is not
    /// backed by a tokio runtime. ADR-0022 §D7 forbids silent
    /// no-ops here; the in-crate adapter maps this to
    /// `BackendError::RuntimeFailure { kind: RuntimeShutdown }`.
    Detached,
    /// Connecting to or provisioning the `JetStream` stream failed.
    Connect {
        /// Underlying client error.
        source: Box<dyn Error + Send + Sync + 'static>,
    },
    /// The server rejected the publish, or the publish-ack stream
    /// surfaced an error (per ADR-0022 §D2 — `append` blocks until
    /// `PubAck` because the ack is the durability signal).
    Publish {
        /// Underlying client error.
        source: Box<dyn Error + Send + Sync + 'static>,
    },
    /// The publish-ack did not arrive within the v0 per-operation
    /// timeout (ADR-0022 §D7 binds a typed timeout surface;
    /// the v0 substrate uses a hard-coded default until the
    /// per-operation knob lands).
    Timeout {
        /// Wall-clock elapsed before the timeout fired.
        elapsed: std::time::Duration,
        /// Configured per-operation timeout that fired.
        configured: std::time::Duration,
    },
    /// Reading the stream contents back through
    /// [`crate::JetStreamHandle::replay_all`] failed — the
    /// server rejected the read, the per-message fetch surfaced
    /// an error, or the message stream terminated abnormally.
    Replay {
        /// Underlying client error.
        source: Box<dyn Error + Send + Sync + 'static>,
    },
}
impl fmt::Display for JetStreamRuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Detached => {
                f.write_str(
                    "runtime handle is detached_for_tests; cannot drive a live JetStream client (ADR-0022 §D7)",
                )
            }
            Self::Connect { source } => write!(f, "connect / provision failed: {source}"),
            Self::Publish { source } => write!(f, "publish failed: {source}"),
            Self::Timeout {
                elapsed,
                configured,
            } => {
                write!(f, "operation timed out after {elapsed:?} (configured {configured:?})")
            }
            Self::Replay { source } => write!(f, "replay failed: {source}"),
        }
    }
}
impl Error for JetStreamRuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Detached | Self::Timeout { .. } => None,
            Self::Connect { source } | Self::Publish { source } | Self::Replay { source } => {
                Some(source.as_ref())
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn boxed_io(msg: &str) -> Box<dyn Error + Send + Sync + 'static> {
        Box::new(std::io::Error::other(msg))
    }
    #[test]
    fn replay_display_names_replay_failure_and_carries_inner_message() {
        let err = JetStreamRuntimeError::Replay {
            source: boxed_io("inner-replay-cause"),
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains("replay failed"),
            "Display must name the read-path operation (got: {rendered:?}) so \
             recovery logs distinguish read failures from write failures at the \
             substrate boundary (mission rescue-pardosa-k67j)"
        );
        assert!(
            rendered.contains("inner-replay-cause"),
            "Display must include the inner source message verbatim (got: \
             {rendered:?}) — single-line operator-readable rendering of the \
             substrate-side read failure (mission rescue-pardosa-k67j)"
        );
    }
    #[test]
    fn replay_source_chain_exposes_inner_error() {
        let err = JetStreamRuntimeError::Replay {
            source: boxed_io("inner-replay-cause"),
        };
        let src = err
            .source()
            .expect("Replay variant must expose its inner source via Error::source()");
        assert!(
            src.to_string().contains("inner-replay-cause"),
            "Error::source() must reach the underlying transport error directly \
             so the in-crate adapter shim (pardosa::backend::jetstream::\
             map_runtime_error) can hand the same inner Box to BackendError::\
             Publish.source without an extra wrapper layer — read-path / \
             write-path source-chain parity (mission rescue-pardosa-k67j)"
        );
    }
    #[test]
    fn detached_and_timeout_have_no_inner_source() {
        let detached = JetStreamRuntimeError::Detached;
        assert!(
            detached.source().is_none(),
            "Detached carries no transport error — it is a runtime-shape pre-condition"
        );
        let timeout = JetStreamRuntimeError::Timeout {
            elapsed: std::time::Duration::from_secs(1),
            configured: std::time::Duration::from_secs(30),
        };
        assert!(
            timeout.source().is_none(),
            "Timeout is a per-operation deadline marker — the source chain has \
             no underlying error to expose"
        );
    }
}
