//! GitHub replenish policy for the generic `BudgetGate` seam.
//!
//! Supplies the two GitHub-shaped decisions the generic
//! [`cherry_pit_wq::BudgetGate`] deliberately does not own (COM-0012:R5,
//! CHE-0102:R5): how long to wait for the upstream quota window to roll,
//! and how large the next epoch's ceiling may be once it has.
//!
//! The wait is derived from GitHub's own `x-ratelimit-reset` timestamp,
//! already parsed by [`update_from_headers`](super::rate_limit::update_from_headers).
//! Per CHE-0102:R3 a stated future reset is never shortened; a missing,
//! already-elapsed, or implausibly distant timestamp is not authoritative
//! and falls back to [`config::API_BUDGET_WAIT_SECS`], which is bounded
//! and never shorter than GitHub's hourly window.
//!
//! The ceiling is re-derived from `x-ratelimit-limit` — the window's
//! entitlement — and NOT from the pre-reset `remaining`, which is stale
//! by construction at this point and is what wedged the epoch at a
//! ceiling of 1 (bd ghr-jiq9z).
//!
//! Having waited the window out, the policy records the roll with
//! [`RateLimitState::note_window_rolled`], which carries no quota
//! reading. It deliberately does NOT write a `remaining` count: no HTTP
//! response supplied one, and synthesising the entitlement would turn a
//! measured observer into an inferred one and let admission proceed on
//! fabricated quota (bd ghr-8i060). The last real reading stays intact
//! for telemetry; the next real response replaces it.

use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cherry_pit_wq::{ReplenishPolicy, Replenished};
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::config;
use crate::github::rate_limit::RateLimitState;

/// Calls held back from the window entitlement so the epoch ceiling
/// never drives `remaining` to exactly zero.
const CEILING_BUFFER: u64 = 100;

/// Ceiling used when the entitlement minus [`CEILING_BUFFER`] would be
/// zero. Keeps the gate on a gradient rather than at the degenerate
/// one-call corner FLO-0009:R1 rules out, while still honouring
/// [`cherry_pit_wq::BudgetGate::set_epoch_limit`]'s non-zero invariant.
const CEILING_FLOOR: NonZeroU64 = NonZeroU64::new(10).expect("ceiling floor is non-zero");

/// Seconds added to a reported reset before resuming, so a wait computed
/// from a whole-second timestamp can never land fractionally early.
const RESET_SLACK_SECS: u64 = 5;

/// Longest reset distance still treated as an authoritative reading.
/// GitHub's primary REST window is one hour; a timestamp further out
/// than a day is a malformed or clock-skewed reading, not a stated wait,
/// so honouring it literally would park the daemon indefinitely.
const MAX_PLAUSIBLE_RESET_SECS: u64 = 24 * 3600;

/// Waits for GitHub's reported rate-limit window to roll, then re-sizes
/// the budget epoch from the fresh window entitlement.
pub struct GithubReplenishPolicy {
    state: Arc<RateLimitState>,
}

impl GithubReplenishPolicy {
    /// Build a policy reading from the shared GitHub rate-limit observer.
    #[must_use]
    pub fn new(state: Arc<RateLimitState>) -> Self {
        Self { state }
    }
}

impl ReplenishPolicy for GithubReplenishPolicy {
    fn replenish<'a>(
        &'a self,
        cancel: &'a CancellationToken,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Replenished> + Send + 'a>> {
        Box::pin(async move {
            let reset = self.state.load_reset();
            let wait = wait_for_reset(reset, unix_now());
            info!(
                wait_secs = wait.as_secs(),
                reset = ?reset,
                authoritative = reset.is_some(),
                "budget epoch waiting for the GitHub rate-limit window to roll"
            );

            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = cancel.cancelled() => return Replenished::Cancelled,
            }

            let entitlement = window_entitlement(self.state.load_limit());
            let ceiling = ceiling_from_entitlement(entitlement);

            self.state.note_window_rolled();

            info!(
                ceiling = ceiling.get(),
                entitlement, "GitHub rate-limit window rolled; budget epoch ceiling re-sized"
            );
            Replenished::Ceiling(ceiling)
        })
    }
}

/// Seconds since the Unix epoch, saturating to 0 on a pre-epoch clock.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// How long to wait before treating the quota window as rolled.
///
/// Only a future, plausibly-distant reported reset is authoritative; it
/// is honoured in full plus [`RESET_SLACK_SECS`] and never shortened
/// (CHE-0102:R3). Every other reading — absent, already elapsed, or
/// beyond [`MAX_PLAUSIBLE_RESET_SECS`] — is not a stated wait, and falls
/// back to the bounded [`config::API_BUDGET_WAIT_SECS`] window.
fn wait_for_reset(reset: Option<u64>, now: u64) -> Duration {
    match reset {
        Some(reset) if reset > now && reset.saturating_sub(now) <= MAX_PLAUSIBLE_RESET_SECS => {
            Duration::from_secs(reset - now + RESET_SLACK_SECS)
        }
        _ => Duration::from_secs(config::API_BUDGET_WAIT_SECS),
    }
}

/// The rolled window's call entitlement.
///
/// Once the reported reset has elapsed, GitHub's window restarts at
/// `x-ratelimit-limit`; the pre-reset `remaining` describes the window
/// that just ended and must not be reused. Falls back to the
/// conservative configured budget when no limit has been observed.
fn window_entitlement(limit: Option<u32>) -> u64 {
    limit.map_or(config::API_BUDGET_LIMIT, u64::from)
}

/// Epoch ceiling for a window of `entitlement` calls.
pub(crate) fn ceiling_from_entitlement(entitlement: u64) -> NonZeroU64 {
    NonZeroU64::new(entitlement.saturating_sub(CEILING_BUFFER)).unwrap_or(CEILING_FLOOR)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn future_reset_is_honoured_in_full_plus_slack() {
        assert_eq!(
            wait_for_reset(Some(1_000 + 3600), 1_000),
            Duration::from_secs(3600 + RESET_SLACK_SECS),
            "a stated future reset must never be shortened (CHE-0102:R3)"
        );
    }

    #[test]
    fn missing_reset_falls_back_to_the_bounded_window() {
        assert_eq!(
            wait_for_reset(None, 1_000),
            Duration::from_secs(config::API_BUDGET_WAIT_SECS)
        );
    }

    #[test]
    fn past_reset_falls_back_to_the_bounded_window() {
        assert_eq!(
            wait_for_reset(Some(999), 1_000),
            Duration::from_secs(config::API_BUDGET_WAIT_SECS),
            "an elapsed timestamp states no wait; it must not authorise an instant resize"
        );
        assert_eq!(
            wait_for_reset(Some(1_000), 1_000),
            Duration::from_secs(config::API_BUDGET_WAIT_SECS)
        );
    }

    #[test]
    fn implausibly_distant_reset_is_rejected_not_slept_through() {
        assert_eq!(
            wait_for_reset(Some(1_000 + MAX_PLAUSIBLE_RESET_SECS + 1), 1_000),
            Duration::from_secs(config::API_BUDGET_WAIT_SECS),
            "a reset beyond a day is malformed, and must never park the daemon unbounded"
        );
        assert_eq!(
            wait_for_reset(Some(1_000 + MAX_PLAUSIBLE_RESET_SECS), 1_000),
            Duration::from_secs(MAX_PLAUSIBLE_RESET_SECS + RESET_SLACK_SECS),
            "the plausibility boundary itself is still authoritative"
        );
    }

    #[test]
    fn ceiling_is_entitlement_minus_buffer() {
        assert_eq!(ceiling_from_entitlement(5000).get(), 4900);
    }

    #[test]
    fn ceiling_never_collapses_to_the_one_call_corner() {
        assert_eq!(ceiling_from_entitlement(100).get(), CEILING_FLOOR.get());
        assert_eq!(ceiling_from_entitlement(0).get(), CEILING_FLOOR.get());
        assert_eq!(ceiling_from_entitlement(40).get(), CEILING_FLOOR.get());
    }

    #[test]
    fn entitlement_uses_window_limit_never_stale_remaining() {
        assert_eq!(window_entitlement(Some(5000)), 5000);
        assert_eq!(window_entitlement(None), config::API_BUDGET_LIMIT);
    }
}
