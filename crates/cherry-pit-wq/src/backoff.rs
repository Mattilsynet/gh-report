//! Server-authoritative pause-until-instant [`Regulator`] — CHE
//! secondary-limit-backoff ADR (adr-fmt-egsrk).
//!
//! [`BackoffRegulator`] parks worker admission until a caller-supplied
//! `resume_at` [`Instant`] then admits, mirroring the `halted_until`
//! atomic-timestamp / `fetch_max` / fast-pre-check pattern in
//! `gh_report::github::client::GitHubClient` (the primary rate-limit
//! halt). It is domain-agnostic (CHE-0084:R1/R7/R9): the input is an
//! opaque wall-future `Instant`, carrying no HTTP/GitHub/Retry-After
//! vocabulary — the upstream signal-to-instant mapping lives in the
//! consumer crate.
//!
//! `fetch_max` on [`BackoffRegulator::set_backoff`] means a call with an
//! earlier `resume_at` than the currently-armed value is a no-op: the
//! armed instant only ever moves forward, matching the "never shorten a
//! server-authoritative wait" safety rule for real callers composing
//! multiple observations concurrently.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::regulator::{Admission, Regulator, SettleOutcome};

/// [`Regulator`] that pauses admission until a caller-supplied resume-at
/// [`Instant`], then admits. `settle` is a no-op — a backoff pause carries
/// no charge concept, matching [`RateLimitRegulator`](crate::RateLimitRegulator).
pub struct BackoffRegulator {
    origin: Instant,
    resume_at_nanos: AtomicU64,
}

impl Default for BackoffRegulator {
    fn default() -> Self {
        Self::new()
    }
}

impl BackoffRegulator {
    /// Construct an unarmed regulator (admits immediately until
    /// [`Self::set_backoff`] is called).
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            resume_at_nanos: AtomicU64::new(0),
        }
    }

    /// Arm (or extend) the pause: admission blocks until `resume_at`.
    ///
    /// Uses `fetch_max` on the stored offset, so a `resume_at` earlier
    /// than the already-armed instant is a no-op — the armed instant
    /// never regresses.
    pub fn set_backoff(&self, resume_at: Instant) {
        let offset_nanos = resume_at.saturating_duration_since(self.origin).as_nanos();
        let offset_nanos = u64::try_from(offset_nanos).unwrap_or(u64::MAX);
        self.resume_at_nanos
            .fetch_max(offset_nanos, Ordering::Release);
    }

    /// The currently-armed resume-at instant, or `None` if unarmed.
    ///
    /// A caller that needs to fold a server-authoritative wait into its own
    /// retry-backoff computation (rather than parking on [`Regulator::admit`])
    /// can read this directly — e.g. overriding a computed jittered-exponential
    /// wait with the exact remaining duration to `resume_at` (CHE-0046
    /// inheritance: a `Retry-After` override narrows the wait only).
    #[must_use]
    pub fn resume_at(&self) -> Option<Instant> {
        let offset_nanos = self.resume_at_nanos.load(Ordering::Acquire);
        if offset_nanos == 0 {
            None
        } else {
            Some(self.origin + Duration::from_nanos(offset_nanos))
        }
    }
}

impl Regulator for BackoffRegulator {
    fn admit<'a>(
        &'a self,
        cancel: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Admission> + Send + 'a>> {
        Box::pin(async move {
            loop {
                let Some(until) = self.resume_at() else {
                    return Admission::Admitted;
                };
                let now = Instant::now();
                if now >= until {
                    return Admission::Admitted;
                }
                tokio::select! {
                    () = tokio::time::sleep(until - now) => {}
                    () = cancel.cancelled() => return Admission::Cancelled,
                }
            }
        })
    }

    fn settle(&self, _outcome: SettleOutcome) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn admit_grants_immediately_when_unarmed() {
        let regulator = BackoffRegulator::new();
        let cancel = CancellationToken::new();
        assert_eq!(regulator.admit(&cancel).await, Admission::Admitted);
    }

    #[tokio::test]
    async fn admit_grants_immediately_when_resume_at_already_passed() {
        let regulator = BackoffRegulator::new();
        regulator.set_backoff(Instant::now().checked_sub(Duration::from_secs(1)).unwrap());
        let cancel = CancellationToken::new();
        assert_eq!(regulator.admit(&cancel).await, Admission::Admitted);
    }

    #[tokio::test(start_paused = true)]
    async fn admit_parks_until_resume_at_then_admits() {
        let regulator = std::sync::Arc::new(BackoffRegulator::new());
        let until = Instant::now() + Duration::from_secs(5);
        regulator.set_backoff(until);

        let regulator_clone = std::sync::Arc::clone(&regulator);
        let handle = tokio::spawn(async move {
            let cancel = CancellationToken::new();
            regulator_clone.admit(&cancel).await
        });

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !handle.is_finished(),
            "admit must not resolve before resume_at"
        );

        tokio::time::advance(Duration::from_secs(5)).await;
        assert_eq!(handle.await.unwrap(), Admission::Admitted);
    }

    #[tokio::test]
    async fn admit_returns_cancelled_when_cancel_fires_while_waiting() {
        let regulator = BackoffRegulator::new();
        regulator.set_backoff(Instant::now() + Duration::from_hours(1));
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(regulator.admit(&cancel).await, Admission::Cancelled);
    }

    #[test]
    fn set_backoff_never_shortens_already_armed_resume_at() {
        let regulator = BackoffRegulator::new();
        let far = Instant::now() + Duration::from_secs(100);
        let near = Instant::now() + Duration::from_secs(1);

        regulator.set_backoff(far);
        regulator.set_backoff(near);

        assert_eq!(
            regulator.resume_at(),
            Some(far),
            "a later set_backoff call with an earlier resume_at must not shorten the armed wait"
        );
    }

    #[test]
    fn set_backoff_extends_when_new_resume_at_is_later() {
        let regulator = BackoffRegulator::new();
        let near = Instant::now() + Duration::from_secs(1);
        let far = Instant::now() + Duration::from_secs(100);

        regulator.set_backoff(near);
        regulator.set_backoff(far);

        assert_eq!(regulator.resume_at(), Some(far));
    }

    #[test]
    fn settle_is_a_documented_no_op() {
        let regulator = BackoffRegulator::new();
        regulator.settle(SettleOutcome::Charged);
        regulator.settle(SettleOutcome::Free);
    }
}
