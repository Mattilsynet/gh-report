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
use crate::github::replenish::GithubReplenishPolicy;

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
    pub(crate) budget_gate: Arc<BudgetGate>,

    /// Shared rate limit state tracking GitHub's `X-RateLimit-*` headers.
    /// Constructed once at daemon startup. Updated from every API response.
    pub(crate) rate_limit_state: Arc<RateLimitState>,

    /// Long-lived GitHub API client. Lazily constructed on the first
    /// collection run via `OnceCell::get_or_try_init()`. `None` before
    /// the first successful credential resolution.
    ///
    /// The client's HTTP connection pool, credential refresh mechanism,
    /// and per-run `scc::HashMap` cache persist across runs. Between runs,
    /// `clear_run_cache()` resets the `scc::HashMap` without dropping the client.
    pub(crate) client: tokio::sync::OnceCell<Arc<GitHubClient>>,

    /// Cross-run repository detail cache (TTL + capacity bounded via moka).
    pub(crate) repo_detail_cache: moka::future::Cache<String, CachedRepoDetail>,
}

impl GithubState {
    /// Create a production `GithubState` with default capacity.
    pub(crate) fn new() -> Self {
        Self::with_cache_capacity(DEFAULT_CACHE_CAPACITY)
    }

    /// Create a `GithubState` with a custom cache capacity.
    pub(crate) fn with_cache_capacity(capacity: u64) -> Self {
        let clamped = capacity.max(1);
        let rate_limit_state = Arc::new(crate::github::rate_limit::new_default());
        Self {
            budget_gate: build_budget_gate(&rate_limit_state),
            rate_limit_state,
            client: tokio::sync::OnceCell::new(),
            repo_detail_cache: build_cache(clamped),
        }
    }
}

/// Build the production budget gate, wired to the GitHub replenish
/// policy that re-sizes the epoch ceiling from a fresh rate-limit
/// window instead of resuming on the pre-pause ceiling (bd ghr-jiq9z).
pub(crate) fn build_budget_gate(rate_limit_state: &Arc<RateLimitState>) -> Arc<BudgetGate> {
    Arc::new(
        BudgetGate::new(
            crate::config::API_BUDGET_LIMIT,
            Duration::from_secs(crate::config::API_BUDGET_WAIT_SECS),
        )
        .with_replenish_policy(Arc::new(GithubReplenishPolicy::new(Arc::clone(
            rate_limit_state,
        )))),
    )
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

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use cherry_pit_wq::{Admission, EpochUsage, RateLimitObservation, Regulator};
    use tokio_util::sync::CancellationToken;

    use super::{Arc, Duration, build_budget_gate};
    use crate::app::worker_pool::RateLimitRegulator;

    /// Incident regression (bd ghr-jiq9z): a quota trough sizes the epoch
    /// ceiling at its floor, and before this fix the gate resumed on that
    /// same ceiling forever — replenish reset only the call counter, so
    /// the run could never finish, never restart, and never re-read
    /// GitHub's long-since-reset quota.
    #[tokio::test(start_paused = true)]
    async fn replenish_resizes_the_epoch_ceiling_from_the_rolled_window() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let rate_limit = Arc::new(crate::github::rate_limit::new_default());
        rate_limit.observe(RateLimitObservation {
            limit: Some(5000),
            remaining: Some(40),
            reset: Some(now + 3600),
        });

        let gate = build_budget_gate(&rate_limit);
        gate.set_epoch_limit(1);
        let cancel = CancellationToken::new();

        assert!(gate.acquire(&cancel).await);

        let waiter_gate = Arc::clone(&gate);
        let waiter_cancel = cancel.clone();
        let waiter = tokio::spawn(async move { waiter_gate.acquire(&waiter_cancel).await });

        tokio::time::advance(Duration::from_secs(3599)).await;
        tokio::task::yield_now().await;
        assert!(
            !waiter.is_finished(),
            "CHE-0102:R3 — GitHub's stated reset must never be undercut"
        );
        assert_eq!(
            gate.epoch_usage(),
            EpochUsage {
                calls_made: 1,
                epoch_limit: 1
            }
        );

        tokio::time::advance(Duration::from_secs(60)).await;
        assert!(waiter.await.unwrap(), "the epoch must reopen after reset");

        assert_eq!(
            gate.epoch_usage(),
            EpochUsage {
                calls_made: 1,
                epoch_limit: 4900
            },
            "the ceiling must be re-sized from the rolled window, not retained"
        );

        let next = tokio::time::timeout(Duration::from_millis(100), gate.acquire(&cancel))
            .await
            .expect("a replenished epoch must admit the next permit immediately");
        assert!(
            next,
            "the next permit must be granted, not refused by a still-closed epoch"
        );
        assert_eq!(gate.calls_made(), 2);

        let regulator = RateLimitRegulator::new(Arc::clone(&rate_limit));
        let admission = tokio::time::timeout(Duration::from_millis(100), regulator.admit(&cancel))
            .await
            .expect("a rolled window must clear the stale halt, not park the worker");
        assert_eq!(
            admission,
            Admission::Admitted,
            "worker admission must clear too — a resized ceiling alone still starves the pool"
        );
    }
}
