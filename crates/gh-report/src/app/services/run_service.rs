//! In-memory `RunService` for sweep lifecycle observability.

use cherry_pit_core::CorrelationContext;

use crate::domain::aggregates::run::{
    CompleteSweep, FailSweep, PublishEvidence, RecordProgress, RenderPartial, RunError, StartSweep,
};

/// In-memory sweep lifecycle service.
#[derive(Debug, Default)]
pub struct RunService;

impl RunService {
    /// Construct an in-memory `RunService`.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Begin a new sweep run.
    ///
    /// # Errors
    ///
    /// This in-memory implementation is infallible.
    pub async fn start_sweep(
        &self,
        cmd: StartSweep,
        _ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        std::future::ready(()).await;
        tracing::info!(
            org = %cmd.org,
            repo_count = cmd.repo_count,
            batch_id = %cmd.batch_id,
            timestamp = %cmd.timestamp,
            snapshot_signature = %cmd.snapshot_signature,
            "sweep started"
        );
        Ok(())
    }

    /// Record a progress checkpoint.
    ///
    /// # Errors
    ///
    /// This in-memory implementation is infallible.
    pub async fn record_progress(
        &self,
        _batch_id: &str,
        cmd: RecordProgress,
        _ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        std::future::ready(()).await;
        tracing::info!(
            batch_id = %cmd.batch_id,
            completed = cmd.completed,
            total = cmd.total,
            timestamp = %cmd.timestamp,
            "sweep progress"
        );
        Ok(())
    }

    /// Mark the sweep complete.
    ///
    /// # Errors
    ///
    /// This in-memory implementation is infallible.
    pub async fn complete(
        &self,
        _batch_id: &str,
        cmd: CompleteSweep,
        _ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        std::future::ready(()).await;
        tracing::info!(
            batch_id = %cmd.batch_id,
            duration_ms = cmd.duration_ms,
            repo_count = cmd.repo_count,
            timestamp = %cmd.timestamp,
            "sweep completed"
        );
        Ok(())
    }

    /// Mark the sweep failed.
    ///
    /// # Errors
    ///
    /// This in-memory implementation is infallible.
    pub async fn fail(
        &self,
        _batch_id: &str,
        cmd: FailSweep,
        _ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        std::future::ready(()).await;
        tracing::warn!(
            batch_id = %cmd.batch_id,
            error = %cmd.error,
            duration_ms = cmd.duration_ms,
            timestamp = %cmd.timestamp,
            "sweep failed"
        );
        Ok(())
    }

    /// Publish evidence after a successful sweep.
    ///
    /// # Errors
    ///
    /// This in-memory implementation is infallible.
    pub async fn publish_evidence(
        &self,
        _batch_id: &str,
        cmd: PublishEvidence,
        _ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        std::future::ready(()).await;
        tracing::info!(
            page_count = cmd.page_count,
            warm_start = cmd.warm_start,
            timestamp = %cmd.timestamp,
            "evidence published"
        );
        Ok(())
    }

    /// Record a mid-sweep partial render.
    ///
    /// # Errors
    ///
    /// This in-memory implementation is infallible.
    pub async fn render_partial(
        &self,
        _batch_id: &str,
        cmd: RenderPartial,
        _ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        std::future::ready(()).await;
        tracing::info!(
            batch_id = %cmd.batch_id,
            page_count = cmd.page_count,
            pending_repos = cmd.pending_repos,
            timestamp = %cmd.timestamp,
            "partial evidence rendered"
        );
        Ok(())
    }
}
