//! Webhook ingestion concerns: secret, replay protection, and debounce.
//!
//! Extracted from [`AppState`] as part of the Phase 2 decomposition.
//! Groups the three fields that are exclusively used by the webhook
//! handler (`POST /webhook`).
//!
//! [`AppState`]: super::state::AppState

use std::time::Duration;

use crate::config;

/// Webhook ingestion sub-aggregate.
///
/// Holds the HMAC secret, replay protection cache, and push debounce
/// cache. All three fields are used exclusively by the webhook handler
/// and its event-mapping logic.
pub struct WebhookState {
    /// Webhook secret for HMAC-SHA256 signature validation.
    /// `None` when webhooks are disabled (env var `WEBHOOK_SECRET` unset).
    /// Changes require a daemon restart.
    pub(crate) secret: Option<secrecy::SecretString>,

    /// Replay protection cache for webhook delivery IDs.
    /// `moka::future::Cache` with 100k capacity, 1h TTL.
    pub(crate) replay_cache: moka::future::Cache<String, ()>,

    /// Debounce cache: per-repo last-enqueue time for push event debounce.
    pub(crate) debounce_cache: moka::future::Cache<String, tokio::time::Instant>,
}

impl WebhookState {
    /// Create a `WebhookState` with an explicit secret and production-grade
    /// cache configuration.
    ///
    /// This is the primary constructor. Uses [`config::REPLAY_CACHE_CAPACITY`]
    /// and [`config::REPLAY_CACHE_TTL_SECS`] for cache sizing.
    #[must_use]
    pub fn with_secret(secret: Option<secrecy::SecretString>) -> Self {
        Self::with_config(
            secret,
            config::REPLAY_CACHE_CAPACITY,
            Duration::from_secs(config::REPLAY_CACHE_TTL_SECS),
        )
    }

    /// Create a `WebhookState` with explicit secret and cache parameters.
    ///
    /// Prefer [`with_secret`](Self::with_secret) for production use.
    /// This constructor exists for tests that need fine-grained control
    /// over cache capacity and TTL.
    #[must_use]
    pub fn with_config(
        secret: Option<secrecy::SecretString>,
        capacity: u64,
        ttl: Duration,
    ) -> Self {
        Self {
            secret,
            replay_cache: moka::future::Cache::builder()
                .max_capacity(capacity)
                .time_to_live(ttl)
                .build(),
            debounce_cache: moka::future::Cache::builder()
                .max_capacity(capacity)
                .time_to_live(Duration::from_secs(
                    config::DEFAULT_WEBHOOK_DEBOUNCE_SECS.saturating_mul(2),
                ))
                .build(),
        }
    }

    /// Create a `WebhookState` by reading the secret from the
    /// `WEBHOOK_SECRET` environment variable.
    ///
    /// Convenience wrapper: resolves the env var and delegates to
    /// [`with_secret`](Self::with_secret).
    #[must_use]
    pub fn from_env() -> Self {
        Self::with_secret(resolve_webhook_secret())
    }

    /// Create a production `WebhookState`, resolving the secret from
    /// the `WEBHOOK_SECRET` environment variable.
    ///
    /// Delegates to [`from_env`](Self::from_env).
    pub(crate) fn from_environment() -> Self {
        Self::from_env()
    }
}

/// Resolve webhook secret from environment. Returns `None` if unset.
fn resolve_webhook_secret() -> Option<secrecy::SecretString> {
    std::env::var("WEBHOOK_SECRET")
        .ok()
        .filter(|s| !s.is_empty())
        .map(secrecy::SecretString::from)
}
