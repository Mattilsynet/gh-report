//! In-memory webhook delivery commands.

use cherry_pit_core::Command;

/// In-memory webhook lifecycle phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeliveryPhase {
    /// No delivery observed.
    #[default]
    Empty,
    /// Delivery observed.
    Received,
}

/// In-memory webhook delivery state retained for compatibility tests.
#[derive(Debug, Clone, Default)]
pub struct WebhookDelivery {
    /// Current lifecycle phase.
    pub phase: DeliveryPhase,
    /// Mapped action.
    pub action: Option<String>,
    /// Repository name, if any.
    pub repo: Option<String>,
}

/// Errors rejecting webhook delivery commands.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum WebhookError {
    /// Delivery has already been received by this in-memory instance.
    #[error("WebhookDelivery already received (terminal)")]
    AlreadyReceived,
    /// In-memory service storage compatibility error.
    #[error("storage error: {0}")]
    Storage(String),
}

impl From<cherry_pit_core::StoreError> for WebhookError {
    fn from(err: cherry_pit_core::StoreError) -> Self {
        Self::Storage(err.to_string())
    }
}

/// Record a single received GitHub webhook delivery.
#[derive(Debug, Clone)]
pub struct RecordDelivery {
    /// GitHub `X-GitHub-Delivery` header value.
    pub delivery_id: String,
    /// Mapped action.
    pub action: String,
    /// Repository name, if applicable.
    pub repo: Option<String>,
    /// ISO 8601 UTC timestamp.
    pub timestamp: String,
}
impl Command for RecordDelivery {}
