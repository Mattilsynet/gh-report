//! In-memory sweep lifecycle commands.

use cherry_pit_core::Command;

/// In-memory run lifecycle phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunPhase {
    /// No lifecycle call observed.
    #[default]
    Empty,
    /// Sweep has started.
    Started,
    /// Sweep completed successfully.
    Completed,
    /// Sweep failed.
    Failed,
    /// Evidence has been published.
    Published,
}

/// In-memory run state retained for compatibility with tests and status code.
#[derive(Debug, Clone, Default)]
pub struct Run {
    /// Current lifecycle phase.
    pub phase: RunPhase,
    /// Current batch id.
    pub batch_id: Option<String>,
    /// Total repository count.
    pub repo_count: u64,
    /// Completed repository count.
    pub completed: u64,
}

/// Errors rejecting in-memory lifecycle commands.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum RunError {
    /// Command requires an empty run.
    #[error("Run already started: phase={0:?}")]
    AlreadyStarted(RunPhase),
    /// Command requires a started run.
    #[error("Run not in Started phase: phase={0:?}")]
    NotStarted(RunPhase),
    /// Publish requires a completed run.
    #[error("EvidencePublished may only follow SweepCompleted: phase={0:?}")]
    NotCompleted(RunPhase),
    /// Historical routing miss retained for callers/tests.
    #[error("routing index has no AggregateId for batch_id={0:?}")]
    RoutingMiss(String),
    /// In-memory service storage compatibility error.
    #[error("storage error: {0}")]
    Storage(String),
}

impl From<cherry_pit_core::StoreError> for RunError {
    fn from(err: cherry_pit_core::StoreError) -> Self {
        Self::Storage(err.to_string())
    }
}

/// Begin a new sweep run.
#[derive(Debug, Clone)]
pub struct StartSweep {
    pub org: String,
    pub repo_count: u64,
    pub batch_id: String,
    pub timestamp: String,
    pub snapshot_signature: String,
}
impl Command for StartSweep {}

/// Record a progress checkpoint mid-sweep.
#[derive(Debug, Clone)]
pub struct RecordProgress {
    pub batch_id: String,
    pub completed: u64,
    pub total: u64,
    pub timestamp: String,
}
impl Command for RecordProgress {}

/// Mark sweep complete.
#[derive(Debug, Clone)]
pub struct CompleteSweep {
    pub batch_id: String,
    pub duration_ms: u64,
    pub repo_count: u64,
    pub timestamp: String,
}
impl Command for CompleteSweep {}

/// Mark sweep failed.
#[derive(Debug, Clone)]
pub struct FailSweep {
    pub batch_id: String,
    pub error: String,
    pub duration_ms: u64,
    pub timestamp: String,
}
impl Command for FailSweep {}

/// Publish evidence after a successful sweep.
#[derive(Debug, Clone)]
pub struct PublishEvidence {
    pub page_count: u64,
    pub warm_start: bool,
    pub timestamp: String,
}
impl Command for PublishEvidence {}

/// Record a mid-sweep partial render.
#[derive(Debug, Clone)]
pub struct RenderPartial {
    pub batch_id: String,
    pub page_count: u64,
    pub pending_repos: u64,
    pub timestamp: String,
}
impl Command for RenderPartial {}
