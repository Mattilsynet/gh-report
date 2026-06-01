//! GitHub API infrastructure: budget gate, rate limit state, client, and cache.
//!
//! Extracted from [`AppState`] as part of the Phase 2 decomposition.
//! Groups the four fields related to GitHub API client lifecycle.
//!
//! [`AppState`]: super::state::AppState

use std::sync::Arc;
use std::time::Duration;

use crate::domain::cache::CachedRepoDetail;
use crate::github::budget::BudgetGate;
use crate::github::client::GitHubClient;
use crate::github::rate_limit::RateLimitState;

/// Default cross-run cache capacity.
const DEFAULT_CACHE_CAPACITY: u64 = 50_000;

/// GitHub API infrastructure sub-aggregate.
///
/// Holds the budget gate, rate limit tracking, the lazily-initialized
/// API client, and the cross-run repository detail cache. All four
/// fields persist across collection runs.
pub struct GithubState {
    /// Shared API budget gate. Constructed once at daemon startup.
    /// Cumulative call counter persists across runs.
    pub budget_gate: Arc<BudgetGate>,

    /// Shared rate limit state tracking GitHub's `X-RateLimit-*` headers.
    /// Constructed once at daemon startup. Updated from every API response.
    pub rate_limit_state: Arc<RateLimitState>,

    /// Long-lived GitHub API client. Lazily constructed on the first
    /// collection run via `OnceCell::get_or_try_init()`. `None` before
    /// the first successful credential resolution.
    ///
    /// The client's HTTP connection pool, credential refresh mechanism,
    /// and per-run `scc::HashMap` cache persist across runs. Between runs,
    /// `clear_run_cache()` resets the `scc::HashMap` without dropping the client.
    pub client: tokio::sync::OnceCell<Arc<GitHubClient>>,

    /// Cross-run repository detail cache (TTL + capacity bounded via moka).
    pub repo_detail_cache: moka::future::Cache<String, CachedRepoDetail>,
}

impl GithubState {
    /// Create a production `GithubState` with default capacity.
    pub(crate) fn new() -> Self {
        Self::with_cache_capacity(DEFAULT_CACHE_CAPACITY)
    }

    /// Create a `GithubState` with a custom cache capacity.
    pub(crate) fn with_cache_capacity(capacity: u64) -> Self {
        let clamped = capacity.max(1);
        Self {
            budget_gate: Arc::new(BudgetGate::new(
                crate::config::API_BUDGET_LIMIT,
                Duration::from_secs(crate::config::API_BUDGET_WAIT_SECS),
            )),
            rate_limit_state: Arc::new(crate::github::rate_limit::new_default()),
            client: tokio::sync::OnceCell::new(),
            repo_detail_cache: build_cache(clamped),
        }
    }
}

/// Build a cross-run repo detail cache with TTL and the given capacity.
pub(crate) fn build_cache(capacity: u64) -> moka::future::Cache<String, CachedRepoDetail> {
    moka::future::Cache::builder()
        .max_capacity(capacity)
        .time_to_live(Duration::from_secs(
            crate::config::REPO_CACHE_TTL_HOURS * 3600,
        ))
        .build()
}
