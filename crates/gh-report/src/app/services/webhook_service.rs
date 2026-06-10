//! In-memory `WebhookService` for delivery observability.

use cherry_pit_core::CorrelationContext;

use crate::domain::aggregates::webhook::{RecordDelivery, WebhookError};

/// In-memory webhook service.
#[derive(Debug, Default)]
pub struct WebhookService;

impl WebhookService {
    /// Construct an in-memory `WebhookService`.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Record a received webhook delivery in tracing only.
    ///
    /// # Errors
    ///
    /// This in-memory implementation is infallible.
    pub async fn ingest(
        &self,
        cmd: RecordDelivery,
        _ctx: &CorrelationContext,
    ) -> Result<(), WebhookError> {
        std::future::ready(()).await;
        tracing::info!(
            delivery_id = %cmd.delivery_id,
            action = %cmd.action,
            repo = ?cmd.repo,
            timestamp = %cmd.timestamp,
            "webhook received"
        );
        Ok(())
    }
}
