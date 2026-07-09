//! API call budget gate.
//!
//! Self-imposed limit on the number of API calls per epoch.
//! When the limit is reached, all workers block until a cooldown period
//! elapses, then the counter resets and work continues.
//!
//! The budget gate is orthogonal to upstream API rate limits — it is a
//! proactive measure to avoid consuming excessive API quota in a single
//! collection run.
//!
//! `BudgetGate` is designed for shared ownership via `Arc<BudgetGate>`.
//! All public methods take `&self`. The `pause_notify` channel uses
//! interior mutability (`std::sync::Mutex`) so it can be attached after
//! the gate is wrapped in `Arc`.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// A budget gate that limits the number of API calls per epoch.
///
/// CAS-based counter: never increments past the limit. When the limit is
/// reached, exactly one worker is elected (via the `resetting` flag) to
/// sleep for the wait duration and reset the counter; the remaining
/// workers register on `epoch_advanced` and are woken when the elected
/// worker completes the reset. No async mutex is held across the sleep.
#[non_exhaustive]
pub struct BudgetGate {
    /// Epoch-local call counter. Reset to 0 after each cooldown.
    calls: AtomicU64,
    /// Cumulative call counter. Never reset.
    total_calls: AtomicU64,
    /// Maximum calls per epoch. Mutable via [`Self::set_epoch_limit`]
    /// for live per-run resizing; a fixed value at construction behaves
    /// exactly as before.
    limit: AtomicU64,
    /// How long to sleep when the budget is exhausted.
    wait_duration: Duration,
    /// Election flag for the epoch-transition sleeper. CAS false→true
    /// elects the unique sleeper; the elected worker clears it back to
    /// false after resetting `calls` and before waking waiters.
    resetting: AtomicBool,
    /// Fires when an epoch transition completes (or aborts early). Woken
    /// waiters re-check `calls` and either CAS-increment or re-attempt
    /// the election.
    epoch_advanced: Notify,
    /// Optional notification channel for budget pause events.
    ///
    /// Uses `std::sync::Mutex` for interior mutability so the notify can
    /// be attached after the gate is wrapped in `Arc`. The mutex is held
    /// only long enough to clone the `Arc<Notify>` — never across await
    /// points.
    pause_notify: StdMutex<Option<Arc<Notify>>>,
}

impl std::fmt::Debug for BudgetGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BudgetGate")
            .field("calls", &self.calls.load(Ordering::Relaxed))
            .field("total_calls", &self.total_calls.load(Ordering::Relaxed))
            .field("limit", &self.limit.load(Ordering::Relaxed))
            .field("wait_duration", &self.wait_duration)
            .finish_non_exhaustive()
    }
}

impl BudgetGate {
    /// Create a new budget gate.
    ///
    /// # Panics
    ///
    /// Panics if `limit` is 0 (would cause infinite epoch transitions).
    #[must_use]
    pub fn new(limit: u64, wait_duration: Duration) -> Self {
        assert!(limit > 0, "budget limit must be > 0");
        Self {
            calls: AtomicU64::new(0),
            total_calls: AtomicU64::new(0),
            limit: AtomicU64::new(limit),
            wait_duration,
            resetting: AtomicBool::new(false),
            epoch_advanced: Notify::new(),
            pause_notify: StdMutex::new(None),
        }
    }

    /// Attach a `Notify` that fires when the budget is exhausted (before sleeping).
    #[must_use]
    pub fn with_pause_notify(self, notify: Arc<Notify>) -> Self {
        *self
            .pause_notify
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(notify);
        self
    }

    /// Set the pause notify after construction.
    ///
    /// Safe to call on a shared `Arc<BudgetGate>` — uses interior mutability.
    /// Replaces any previously attached `Notify`. Callers must ensure no
    /// partial publisher is still awaiting the old `Notify` before replacing it.
    pub fn set_pause_notify(&self, notify: Arc<Notify>) {
        *self
            .pause_notify
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(notify);
    }

    /// Change the per-epoch call limit that `acquire` gates against.
    ///
    /// Safe to call on a shared `Arc<BudgetGate>` — uses an atomic
    /// store, no lock. Takes effect for any `acquire` loop iteration
    /// that has not yet snapshotted the previous limit; does not reset
    /// `calls`, so a lower limit takes effect once `calls` reaches it
    /// via new `acquire` calls or the next epoch reset.
    ///
    /// # Panics
    ///
    /// Panics if `limit` is 0 (would cause infinite epoch transitions),
    /// matching [`Self::new`]'s invariant.
    pub fn set_epoch_limit(&self, limit: u64) {
        assert!(limit > 0, "budget limit must be > 0");
        self.limit.store(limit, Ordering::Release);
    }

    /// Acquire one API call permit.
    ///
    /// Returns immediately if budget is available. If the epoch limit is
    /// reached, exactly one caller is elected to sleep `wait_duration`
    /// and reset the counter; the rest wait on `epoch_advanced` without
    /// holding any async mutex across the sleep.
    ///
    /// Returns `false` when `cancel` is cancelled while this caller is
    /// parked in the elected cooldown sleep; in that case the epoch is not
    /// reset and callers should exit rather than resume work.
    pub async fn acquire(&self, cancel: &CancellationToken) -> bool {
        loop {
            let limit = self.limit.load(Ordering::Acquire);
            let current = self.calls.load(Ordering::Acquire);
            if current < limit {
                match self.calls.compare_exchange_weak(
                    current,
                    current + 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        self.total_calls.fetch_add(1, Ordering::Relaxed);
                        return true;
                    }
                    Err(_) => continue,
                }
            }
            if self
                .resetting
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let guard = ResetGuard { gate: self };
                if self.calls.load(Ordering::Acquire) < limit {
                    drop(guard);
                    continue;
                }
                warn!(
                    calls = limit,
                    wait_secs = self.wait_duration.as_secs(),
                    "API budget exhausted, pausing collection"
                );
                let notify = self
                    .pause_notify
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                if let Some(n) = notify {
                    n.notify_one();
                }
                tokio::select! {
                    () = tokio::time::sleep(self.wait_duration) => {}
                    () = cancel.cancelled() => return false,
                }
                self.calls.store(0, Ordering::Release);
                drop(guard);
                info!("API budget replenished, resuming collection");
                continue;
            }
            let notified = self.epoch_advanced.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.calls.load(Ordering::Acquire) < limit {
                continue;
            }
            notified.await;
        }
    }

    /// Number of calls made in the current epoch.
    #[must_use]
    pub fn calls_made(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }

    /// Cumulative number of calls made across all epochs.
    #[must_use]
    pub fn total_calls_made(&self) -> u64 {
        self.total_calls.load(Ordering::Relaxed)
    }
}

struct ResetGuard<'a> {
    gate: &'a BudgetGate,
}

impl Drop for ResetGuard<'_> {
    fn drop(&mut self) {
        self.gate.resetting.store(false, Ordering::Release);
        self.gate.epoch_advanced.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn acquire_within_limit_succeeds() {
        let gate = BudgetGate::new(5, Duration::from_secs(1));
        let cancel = CancellationToken::new();
        for _ in 0..5 {
            assert!(gate.acquire(&cancel).await);
        }
        assert_eq!(gate.calls_made(), 5);
        assert_eq!(gate.total_calls_made(), 5);
    }

    #[tokio::test]
    async fn sixth_call_blocks_then_resets() {
        tokio::time::pause();
        let gate = Arc::new(BudgetGate::new(5, Duration::from_mins(1)));
        let cancel = CancellationToken::new();

        for _ in 0..5 {
            assert!(gate.acquire(&cancel).await);
        }
        assert_eq!(gate.calls_made(), 5);

        let gate2 = Arc::clone(&gate);
        let waiter_cancel = cancel.clone();
        let handle = tokio::spawn(async move {
            gate2.acquire(&waiter_cancel).await;
        });

        tokio::time::advance(Duration::from_secs(61)).await;
        handle.await.unwrap();

        assert_eq!(gate.calls_made(), 1);
        assert_eq!(gate.total_calls_made(), 6);
    }

    #[tokio::test]
    async fn concurrent_acquire_never_exceeds_limit() {
        tokio::time::pause();
        let gate = Arc::new(BudgetGate::new(10, Duration::from_mins(1)));
        let cancel = CancellationToken::new();

        let mut handles = Vec::new();
        for _ in 0..16 {
            let g = Arc::clone(&gate);
            let worker_cancel = cancel.clone();
            handles.push(tokio::spawn(async move {
                g.acquire(&worker_cancel).await;
            }));
        }

        tokio::time::advance(Duration::from_secs(61)).await;

        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(gate.total_calls_made(), 16);
        assert!(gate.calls_made() <= 10);
    }

    #[tokio::test]
    async fn total_calls_cumulative_across_resets() {
        tokio::time::pause();
        let gate = Arc::new(BudgetGate::new(2, Duration::from_secs(10)));
        let cancel = CancellationToken::new();

        assert!(gate.acquire(&cancel).await);
        assert!(gate.acquire(&cancel).await);
        assert_eq!(gate.total_calls_made(), 2);

        let g = Arc::clone(&gate);
        let waiter_cancel = cancel.clone();
        let h = tokio::spawn(async move { g.acquire(&waiter_cancel).await });
        tokio::time::advance(Duration::from_secs(11)).await;
        h.await.unwrap();

        assert_eq!(gate.total_calls_made(), 3);
        assert_eq!(gate.calls_made(), 1);
    }

    #[test]
    #[should_panic(expected = "budget limit must be > 0")]
    fn zero_limit_panics() {
        let _ = BudgetGate::new(0, Duration::from_secs(1));
    }

    /// Static assertions that `BudgetGate` is `Send + Sync`.
    #[test]
    fn budget_gate_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<BudgetGate>();
    }

    #[tokio::test]
    async fn pause_notify_fires_on_epoch_pause() {
        tokio::time::pause();
        let notify = Arc::new(tokio::sync::Notify::new());
        let cancel = CancellationToken::new();
        let gate = Arc::new(
            BudgetGate::new(2, Duration::from_secs(10)).with_pause_notify(Arc::clone(&notify)),
        );

        assert!(gate.acquire(&cancel).await);
        assert!(gate.acquire(&cancel).await);

        let notify2 = Arc::clone(&notify);
        let notified = tokio::spawn(async move {
            notify2.notified().await;
            true
        });

        let g = Arc::clone(&gate);
        let waiter_cancel = cancel.clone();
        tokio::spawn(async move { g.acquire(&waiter_cancel).await });

        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;

        tokio::time::advance(Duration::from_secs(11)).await;

        assert!(notified.await.unwrap());
    }

    #[tokio::test]
    async fn set_pause_notify_fires_on_epoch_pause() {
        tokio::time::pause();
        let notify = Arc::new(tokio::sync::Notify::new());
        let gate = BudgetGate::new(2, Duration::from_secs(10));
        let cancel = CancellationToken::new();
        gate.set_pause_notify(Arc::clone(&notify));
        let gate = Arc::new(gate);

        assert!(gate.acquire(&cancel).await);
        assert!(gate.acquire(&cancel).await);

        let notify2 = Arc::clone(&notify);
        let notified = tokio::spawn(async move {
            notify2.notified().await;
            true
        });

        let g = Arc::clone(&gate);
        let waiter_cancel = cancel.clone();
        tokio::spawn(async move { g.acquire(&waiter_cancel).await });

        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(11)).await;

        assert!(notified.await.unwrap());
    }

    #[tokio::test]
    async fn set_pause_notify_through_arc() {
        tokio::time::pause();
        let gate = Arc::new(BudgetGate::new(2, Duration::from_secs(10)));
        let cancel = CancellationToken::new();

        let notify = Arc::new(tokio::sync::Notify::new());
        gate.set_pause_notify(Arc::clone(&notify));

        assert!(gate.acquire(&cancel).await);
        assert!(gate.acquire(&cancel).await);

        let notify2 = Arc::clone(&notify);
        let notified = tokio::spawn(async move {
            notify2.notified().await;
            true
        });

        let g = Arc::clone(&gate);
        let waiter_cancel = cancel.clone();
        tokio::spawn(async move { g.acquire(&waiter_cancel).await });

        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(11)).await;

        assert!(notified.await.unwrap());
    }

    #[tokio::test]
    async fn late_waiters_join_in_flight_epoch_transition() {
        tokio::time::pause();
        let pause = Arc::new(tokio::sync::Notify::new());
        let gate = Arc::new(
            BudgetGate::new(2, Duration::from_secs(10)).with_pause_notify(Arc::clone(&pause)),
        );
        let cancel = CancellationToken::new();

        assert!(gate.acquire(&cancel).await);
        assert!(gate.acquire(&cancel).await);
        assert_eq!(gate.calls_made(), 2);

        let g_first = Arc::clone(&gate);
        let first_cancel = cancel.clone();
        let first = tokio::spawn(async move { g_first.acquire(&first_cancel).await });

        pause.notified().await;
        assert_eq!(gate.calls_made(), 2);

        let mut late = Vec::new();
        for _ in 0..4 {
            let g = Arc::clone(&gate);
            let waiter_cancel = cancel.clone();
            late.push(tokio::spawn(async move { g.acquire(&waiter_cancel).await }));
        }

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(11)).await;

        first.await.unwrap();
        for h in late {
            h.await.unwrap();
        }

        assert_eq!(gate.total_calls_made(), 7);
        assert!(gate.calls_made() <= 2);
    }

    #[tokio::test]
    async fn cancelled_resetter_releases_election_and_wakes_waiters() {
        tokio::time::pause();
        let pause = Arc::new(tokio::sync::Notify::new());
        let gate = Arc::new(
            BudgetGate::new(2, Duration::from_secs(10)).with_pause_notify(Arc::clone(&pause)),
        );
        let cancel = CancellationToken::new();

        assert!(gate.acquire(&cancel).await);
        assert!(gate.acquire(&cancel).await);
        assert_eq!(gate.calls_made(), 2);

        let g_doomed = Arc::clone(&gate);
        let doomed_cancel = cancel.clone();
        let doomed = tokio::spawn(async move { g_doomed.acquire(&doomed_cancel).await });

        pause.notified().await;
        doomed.abort();
        let _ = doomed.await;

        let g_next = Arc::clone(&gate);
        let next_cancel = cancel.clone();
        let next = tokio::spawn(async move { g_next.acquire(&next_cancel).await });

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(11)).await;

        next.await.unwrap();
        assert_eq!(gate.total_calls_made(), 3);
        assert_eq!(gate.calls_made(), 1);
    }

    #[tokio::test]
    async fn cancellation_token_preempts_budget_backoff_sleep() {
        let pause = Arc::new(tokio::sync::Notify::new());
        let gate = Arc::new(
            BudgetGate::new(1, Duration::from_mins(1)).with_pause_notify(Arc::clone(&pause)),
        );
        let cancel = tokio_util::sync::CancellationToken::new();

        assert!(gate.acquire(&cancel).await);
        assert_eq!(gate.calls_made(), 1);

        let waiter_gate = Arc::clone(&gate);
        let waiter_cancel = cancel.clone();
        let waiter = tokio::spawn(async move { waiter_gate.acquire(&waiter_cancel).await });

        pause.notified().await;
        cancel.cancel();

        let acquired = tokio::time::timeout(Duration::from_millis(100), waiter)
            .await
            .expect("cancelled budget acquire should return promptly")
            .expect("waiter task should not panic");
        assert!(!acquired);
        assert_eq!(gate.total_calls_made(), 1);
        assert_eq!(gate.calls_made(), 1);
    }

    #[tokio::test]
    async fn set_epoch_limit_raises_ceiling_live() {
        let gate = BudgetGate::new(2, Duration::from_mins(1));
        let cancel = CancellationToken::new();

        assert!(gate.acquire(&cancel).await);
        assert!(gate.acquire(&cancel).await);
        assert_eq!(gate.calls_made(), 2);

        gate.set_epoch_limit(10);

        let acquired = tokio::time::timeout(Duration::from_millis(100), gate.acquire(&cancel))
            .await
            .expect("acquire after raising the epoch limit should not block");
        assert!(acquired);
        assert_eq!(gate.calls_made(), 3);
        assert_eq!(gate.total_calls_made(), 3);
    }

    #[test]
    #[should_panic(expected = "budget limit must be > 0")]
    fn set_epoch_limit_zero_panics() {
        let gate = BudgetGate::new(1, Duration::from_secs(1));
        gate.set_epoch_limit(0);
    }
}
