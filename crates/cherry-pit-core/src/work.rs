//! Shared work-execution types for the cherry-pit family.
//!
//! These types describe the *shape* of asynchronous unit-of-work
//! dispatch (key, source, outcome) without committing to any runtime,
//! queue topology, or transport. They live in `cherry-pit-core` so
//! upstream domain crates and downstream wq/agent/projection crates
//! can name them without depending on `cherry-pit-wq` or each other.
//!
//! Per CHE-0018:R3, no async runtime types appear here — only `std`
//! primitives and [`crate::CorrelationContext`] (already in core).

use std::time::Duration;

use crate::CorrelationContext;

/// Domain-specific identifier for a unit of work.
///
/// Opaque to generic infrastructure (queue, worker pool); domain code
/// chooses the semantics (e.g. a numeric repo ID rendered as text).
pub type DomainKey = String;

/// Origin of a job — observability only; never affects ordering.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum JobSource {
    /// Part of a scheduled batch.
    ScheduledBatch,
    /// Triggered by an external event (e.g. webhook, notification).
    External { id: String, kind: String },
    /// Initial load at startup.
    InitialLoad,
}

/// Result of processing a single unit of work.
///
/// The `correlation` field propagates the producer's chain end-to-end
/// per CHE-0055 G5 so dead-letter / outcome consumers observe the same
/// causal chain as the originating job.
#[derive(Debug)]
#[non_exhaustive]
pub enum JobOutcome<R> {
    /// Job completed successfully.
    Success {
        domain_key: DomainKey,
        result: R,
        source: JobSource,
        duration: Duration,
        correlation: CorrelationContext,
    },
    /// Job failed.
    Failure {
        domain_key: DomainKey,
        error: String,
        source: JobSource,
        duration: Duration,
        correlation: CorrelationContext,
    },
}
