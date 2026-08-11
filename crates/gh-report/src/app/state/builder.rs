//! `AppStateBuilder`: test-only fluent constructor for `AppState`.
//!
//! Consolidates the previous `new_with_cache_capacity`,
//! `new_with_webhook_secret`, and `new_test` constructors into a
//! single fluent API. Extracted from `state.rs` (K9, adr-fmt-b98n1) as a
//! pure structural move — no behavioural change.

use std::sync::Arc;
use std::sync::Mutex;

use arc_swap::ArcSwap;
use jiff::Timestamp;

use super::{
    AppState, GithubState, WebhookState, WorkQueue, WorkerShutdownToken, noop_event_store,
    noop_org_event_store, noop_scheduler_event_store, noop_sweep_timeout_event_store,
    noop_team_event_store,
};
use crate::app::evidence_service::EvidenceState;

/// Builder for constructing `AppState` with explicit control
/// over cache capacity and webhook secret.
///
/// # Example
///
/// ```ignore
/// let state = AppStateBuilder::new()
///     .cache_capacity(10)
///     .webhook_secret("test-secret")
///     .build();
/// ```
#[cfg(test)]
pub struct AppStateBuilder {
    cache_capacity: Option<u64>,
    webhook_secret: Option<secrecy::SecretString>,
}

#[cfg(test)]
impl Default for AppStateBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl AppStateBuilder {
    /// Create a builder with default values.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache_capacity: None,
            webhook_secret: None,
        }
    }

    /// Set the cross-run repo detail cache capacity.
    #[must_use]
    pub fn cache_capacity(mut self, capacity: u64) -> Self {
        self.cache_capacity = Some(capacity);
        self
    }

    /// Set the webhook HMAC secret.
    #[must_use]
    pub fn webhook_secret(mut self, secret: &str) -> Self {
        self.webhook_secret = Some(secrecy::SecretString::from(secret.to_string()));
        self
    }

    /// Build the `Arc<AppState>`.
    ///
    /// # Panics
    ///
    /// Panics if the unique tempdir-based noop event-store directory
    /// cannot acquire the CHE-0043:R1 advisory flock at `open` time.
    /// This is an infrastructure-level failure (disk full, permissions,
    /// no `/tmp`) at builder construction in a test path; halting is
    /// appropriate.
    pub async fn build(self) -> Arc<AppState> {
        let github = match self.cache_capacity {
            Some(cap) => GithubState::with_cache_capacity(cap),
            None => GithubState::new(),
        };
        let webhook = WebhookState::with_secret(self.webhook_secret);
        let event_store = noop_event_store().await;
        let org_event_store = noop_org_event_store().await;
        let team_event_store = noop_team_event_store().await;
        let scheduler_event_store = noop_scheduler_event_store();
        let sweep_timeout_event_store = noop_sweep_timeout_event_store();
        let projection_state =
            Arc::new(Mutex::new(crate::projection::EvidenceProjection::default()));

        Arc::new(AppState {
            started_at: Timestamp::now(),
            owner_id: uuid::Uuid::now_v7(),
            current_run: ArcSwap::from_pointee(None),
            last_completed_run: ArcSwap::from_pointee(None),
            last_recovery: ArcSwap::from_pointee(None),
            work_queue: Arc::new(WorkQueue::new(crate::config::WORK_QUEUE_CAPACITY)),
            worker_pool_started: tokio::sync::OnceCell::new(),
            worker_pool_cancel: WorkerShutdownToken::new(),
            event_store,
            org_event_store,
            team_event_store,
            scheduler_event_store,
            sweep_timeout_event_store,
            projection_state,
            webhook,
            github,
            evidence: EvidenceState::new(),
            sweep_lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }
}

/// Legacy convenience constructors (delegate to builder).
#[cfg(test)]
impl AppState {
    /// Create an `AppState` with a custom cache capacity (for testing).
    pub async fn new_with_cache_capacity(capacity: u64) -> Arc<Self> {
        AppStateBuilder::new()
            .cache_capacity(capacity)
            .build()
            .await
    }

    /// Create an `AppState` with a known webhook secret (for testing).
    pub async fn new_with_webhook_secret(secret: &str) -> Arc<Self> {
        AppStateBuilder::new().webhook_secret(secret).build().await
    }
}
