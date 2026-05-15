//! Dead-letter sink trait + record type per CHE-0051:R7.
//!
//! Per CHE-0024:R5 + CHE-0040:R3 the record carries: `event_id`,
//! `correlation_id`, `causation_id`, `error_category`, `output_type`,
//! `policy_identity`. This sub-mission (S5) defines the trait + record
//! struct inline so the policy-output dispatcher can compile and route
//! `Terminal` errors. S6 will add the `TracingDeadLetterSink` default
//! impl, the golden-file schema test, and the tracing emission test.
//!
//! ## C1 boundary note
//!
//! Like the closure-handler shape on `InProcessEventBus` (linus R1
//! ruling at `event_bus.rs:22-37`), `DeadLetterSink` is a
//! **user-supplied infrastructure adapter trait**, not an
//! aggregate-bound infra port from `cherry-pit-core`. CHE-0005:R1
//! forbids `Box<dyn>` over `EventStore`, `EventBus`, `CommandBus`,
//! `CommandGateway`, `Policy`, `Projection`. `DeadLetterSink` lives
//! on the agent side of the boundary and exists precisely so consumers
//! can swap in durable backends without rebuilding `App`. Storing the
//! sink as a generic `D: DeadLetterSink` parameter on `App` keeps the
//! type discipline; the trait itself does not need to be object-safe
//! for the v0.1 wiring.

use std::error::Error;

use cherry_pit_core::ErrorCategory;

/// Diagnostic record handed to a [`DeadLetterSink`] when a policy
/// output's dispatch fails terminally.
///
/// Field set verbatim from CHE-0024:R5 + CHE-0040:R3 + CHE-0051:R7:
/// `event_id`, `correlation_id`, `causation_id`, `error_category`,
/// `output_type`, `policy_identity`. An additional `error_message`
/// field carries the stringified error for operator diagnosis — this
/// is documentation, not a stable schema commitment, and S6's
/// golden-file test will lock the schema.
///
/// All UUID fields are `Option<_>` to mirror `EventEnvelope`'s
/// nullable correlation/causation IDs (CHE-0039 — user-initiated
/// commands have no correlation context). `event_id` is always
/// present because every persisted event has one (CHE-0033:R1).
///
/// Marked `#[non_exhaustive]` per CHE-0021:R1: future fields (e.g.
/// retry attempt count, sink-supplied trace IDs) may be added
/// without a major-version bump. Construct via [`DeadLetterRecord::new`]
/// rather than struct literal so downstream consumers are insulated
/// from additive changes.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DeadLetterRecord {
    /// `event_id` of the envelope that triggered the failed dispatch.
    pub event_id: uuid::Uuid,

    /// Correlation ID inherited from the envelope, if any.
    pub correlation_id: Option<uuid::Uuid>,

    /// Causation ID inherited from the envelope, if any.
    pub causation_id: Option<uuid::Uuid>,

    /// Retry-guidance category at the moment of failure.
    pub error_category: ErrorCategory,

    /// Stable identifier for the policy output type (the enum name
    /// or other static string — supplied at registration).
    pub output_type: &'static str,

    /// Stable identifier for the policy that produced the output.
    pub policy_identity: &'static str,

    /// Stringified error for operator diagnosis. Schema-unstable
    /// until S6's golden-file test pins it.
    pub error_message: String,
}

impl DeadLetterRecord {
    /// Construct a [`DeadLetterRecord`] with all 7 fields in declaration
    /// order. Required entry point because the struct is
    /// `#[non_exhaustive]` (CHE-0021:R1) — downstream consumers cannot
    /// use struct-literal syntax across the crate boundary.
    ///
    /// Field order matches the declaration above and the tracing
    /// emission schema locked by `tests/dead_letter_tracing.rs`
    /// (CHE-0024:R5 + CHE-0040:R3 + CHE-0051:R7).
    #[must_use]
    pub fn new(
        event_id: uuid::Uuid,
        correlation_id: Option<uuid::Uuid>,
        causation_id: Option<uuid::Uuid>,
        error_category: ErrorCategory,
        output_type: &'static str,
        policy_identity: &'static str,
        error_message: String,
    ) -> Self {
        Self {
            event_id,
            correlation_id,
            causation_id,
            error_category,
            output_type,
            policy_identity,
            error_message,
        }
    }
}

/// Sink for terminal policy-output dispatch failures per CHE-0051:R7.
///
/// `record` is async-fallible to accommodate durable sinks that may
/// hit infrastructure failures (S6 ships a `TracingDeadLetterSink`
/// default that is infallible; later durable sinks may surface real
/// errors).
///
/// ## v0.1 default impl
///
/// S6 supplies `TracingDeadLetterSink` emitting `tracing::error!`.
/// Consumers needing durable persistence implement this trait against
/// their preferred store. Per CHE-0051 Consequences, the durable
/// default is deferred.
pub trait DeadLetterSink: Send + Sync + 'static {
    /// Record a dead-letter event. Returns `Err` only on infrastructure
    /// failure of the sink itself (e.g. disk full for a file-backed
    /// impl); the agent treats `Err` as terminal and surfaces it as
    /// [`AgentError::DeadLetter`](crate::AgentError::DeadLetter).
    fn record(
        &self,
        record: DeadLetterRecord,
    ) -> impl std::future::Future<Output = Result<(), Box<dyn Error + Send + Sync>>> + Send;
}

/// Default [`DeadLetterSink`] impl emitting one structured
/// `tracing::error!` event per record per CHE-0051:R7 + CHE-0024:R5 +
/// CHE-0040:R3.
///
/// Field shape (locked by the golden-file schema test):
/// `event_id`, `correlation_id`, `causation_id`, `error_category`,
/// `output_type`, `policy_identity`, `error_message`. `Option` UUID
/// fields are emitted via `tracing`'s `?` debug-format sigil so
/// `None` renders as `None` rather than being dropped.
///
/// Infallible: in-memory `tracing` emission has no I/O. Consumers
/// needing durable persistence implement [`DeadLetterSink`] against
/// their preferred backend; this default exists so the agent ships
/// with a working sink out of the box.
#[derive(Debug, Default, Clone)]
pub struct TracingDeadLetterSink;

impl TracingDeadLetterSink {
    /// Construct a new [`TracingDeadLetterSink`].
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl DeadLetterSink for TracingDeadLetterSink {
    fn record(
        &self,
        record: DeadLetterRecord,
    ) -> impl std::future::Future<Output = Result<(), Box<dyn Error + Send + Sync>>> + Send {
        // Schema is golden-file pinned by `tests/dead_letter_tracing.rs` —
        // do not reorder or rename fields without updating that test.
        tracing::error!(
            event_id = %record.event_id,
            correlation_id = ?record.correlation_id,
            causation_id = ?record.causation_id,
            error_category = ?record.error_category,
            output_type = record.output_type,
            policy_identity = record.policy_identity,
            error_message = %record.error_message,
            "policy output dispatch failed terminally; routed to dead-letter"
        );
        async move { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Test sink capturing records into a `Vec` for assertions.
    pub(crate) struct VecSink {
        pub records: Mutex<Vec<DeadLetterRecord>>,
    }

    impl VecSink {
        pub(crate) fn new() -> Self {
            Self {
                records: Mutex::new(Vec::new()),
            }
        }
    }

    impl DeadLetterSink for VecSink {
        fn record(
            &self,
            record: DeadLetterRecord,
        ) -> impl std::future::Future<Output = Result<(), Box<dyn Error + Send + Sync>>> + Send
        {
            self.records.lock().unwrap().push(record);
            async move { Ok(()) }
        }
    }

    #[tokio::test]
    async fn vec_sink_records_payload() {
        let sink = VecSink::new();
        let event_id = uuid::Uuid::now_v7();
        let record = DeadLetterRecord {
            event_id,
            correlation_id: None,
            causation_id: None,
            error_category: ErrorCategory::Terminal,
            output_type: "Notify",
            policy_identity: "OrderNotifier",
            error_message: "smtp 421".into(),
        };
        sink.record(record).await.unwrap();

        let captured = sink.records.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].event_id, event_id);
        assert_eq!(captured[0].output_type, "Notify");
        assert_eq!(captured[0].error_category, ErrorCategory::Terminal);
    }
}
