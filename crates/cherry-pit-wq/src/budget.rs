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

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tracing::{info, warn};

/// A budget gate that limits the number of API calls per epoch.
///
/// CAS-based counter: never increments past the limit. Workers that see
/// `current >= limit` block on the epoch mutex. Exactly one worker sleeps
/// for the wait duration and resets the counter; others re-check and
/// proceed.
#[non_exhaustive]
pub struct BudgetGate {
    /// Epoch-local call counter. Reset to 0 after each cooldown.
    calls: AtomicU64,
    /// Cumulative call counter. Never reset.
    total_calls: AtomicU64,
    /// Maximum calls per epoch.
    limit: u64,
    /// How long to sleep when the budget is exhausted.
    wait_duration: Duration,
    /// Mutex that serializes epoch transitions.
    gate: tokio::sync::Mutex<()>,
    /// Optional notification channel for budget pause events.
    ///
    /// Uses `std::sync::Mutex` for interior mutability so the notify can
    /// be attached after the gate is wrapped in `Arc`. The mutex is held
    /// only long enough to clone the `Arc<Notify>` — never across await
    /// points.
    pause_notify: StdMutex<Option<Arc<tokio::sync::Notify>>>,
}

impl std::fmt::Debug for BudgetGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BudgetGate")
            .field("calls", &self.calls.load(Ordering::Relaxed))
            .field("total_calls", &self.total_calls.load(Ordering::Relaxed))
            .field("limit", &self.limit)
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
            limit,
            wait_duration,
            gate: tokio::sync::Mutex::new(()),
            pause_notify: StdMutex::new(None),
        }
    }

    /// Attach a `Notify` that fires when the budget is exhausted (before sleeping).
    #[must_use]
    pub fn with_pause_notify(self, notify: Arc<tokio::sync::Notify>) -> Self {
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
    pub fn set_pause_notify(&self, notify: Arc<tokio::sync::Notify>) {
        *self
            .pause_notify
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(notify);
    }

    /// Acquire one API call permit.
    ///
    /// Returns immediately if budget is available. Blocks (sleeps) if the
    /// epoch limit is reached, then resets and returns.
    pub async fn acquire(&self) {
        loop {
            let current = self.calls.load(Ordering::Acquire);
            if current < self.limit {
                match self.calls.compare_exchange_weak(
                    current,
                    current + 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        self.total_calls.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    Err(_) => continue,
                }
            }
            // At limit — epoch transition.
            {
                let _g = self.gate.lock().await;
                // Double-check: another worker may have already reset.
                if self.calls.load(Ordering::Acquire) >= self.limit {
                    warn!(
                        calls = self.limit,
                        wait_secs = self.wait_duration.as_secs(),
                        "API budget exhausted, pausing collection"
                    );
                    // Clone the notify Arc inside the std::sync::Mutex, then
                    // drop the guard before calling notify_one(). The mutex
                    // is held for <1μs (pointer clone only).
                    let notify = self
                        .pause_notify
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    if let Some(n) = notify {
                        n.notify_one();
                    }
                    tokio::time::sleep(self.wait_duration).await;
                    self.calls.store(0, Ordering::Release);
                    info!("API budget replenished, resuming collection");
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn acquire_within_limit_succeeds() {
        let gate = BudgetGate::new(5, Duration::from_secs(1));
        for _ in 0..5 {
            gate.acquire().await;
        }
        assert_eq!(gate.calls_made(), 5);
        assert_eq!(gate.total_calls_made(), 5);
    }

    #[tokio::test]
    async fn sixth_call_blocks_then_resets() {
        tokio::time::pause();
        let gate = Arc::new(BudgetGate::new(5, Duration::from_mins(1)));

        // Use all 5 permits.
        for _ in 0..5 {
            gate.acquire().await;
        }
        assert_eq!(gate.calls_made(), 5);

        // 6th call should block.
        let gate2 = Arc::clone(&gate);
        let handle = tokio::spawn(async move {
            gate2.acquire().await;
        });

        // Advance time past the wait duration.
        tokio::time::advance(Duration::from_secs(61)).await;
        handle.await.unwrap();

        // Counter should have reset and incremented by 1.
        assert_eq!(gate.calls_made(), 1);
        assert_eq!(gate.total_calls_made(), 6);
    }

    #[tokio::test]
    async fn concurrent_acquire_never_exceeds_limit() {
        tokio::time::pause();
        let gate = Arc::new(BudgetGate::new(10, Duration::from_mins(1)));

        let mut handles = Vec::new();
        for _ in 0..16 {
            let g = Arc::clone(&gate);
            handles.push(tokio::spawn(async move {
                g.acquire().await;
            }));
        }

        // Let the 10 acquire, then advance time for the remaining 6.
        tokio::time::advance(Duration::from_secs(61)).await;

        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(gate.total_calls_made(), 16);
        // After reset, remaining 6 were served in a new epoch.
        assert!(gate.calls_made() <= 10);
    }

    #[tokio::test]
    async fn total_calls_cumulative_across_resets() {
        tokio::time::pause();
        let gate = Arc::new(BudgetGate::new(2, Duration::from_secs(10)));

        // Epoch 1: 2 calls.
        gate.acquire().await;
        gate.acquire().await;
        assert_eq!(gate.total_calls_made(), 2);

        // Epoch transition via 3rd call.
        let g = Arc::clone(&gate);
        let h = tokio::spawn(async move { g.acquire().await });
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
        let gate = Arc::new(
            BudgetGate::new(2, Duration::from_secs(10)).with_pause_notify(Arc::clone(&notify)),
        );

        gate.acquire().await;
        gate.acquire().await;

        let notify2 = Arc::clone(&notify);
        let notified = tokio::spawn(async move {
            notify2.notified().await;
            true
        });

        let g = Arc::clone(&gate);
        tokio::spawn(async move { g.acquire().await });

        // The notification should fire before the sleep completes.
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;

        // Advance past sleep so the acquire completes.
        tokio::time::advance(Duration::from_secs(11)).await;

        assert!(notified.await.unwrap());
    }

    #[tokio::test]
    async fn set_pause_notify_fires_on_epoch_pause() {
        tokio::time::pause();
        let notify = Arc::new(tokio::sync::Notify::new());
        let gate = BudgetGate::new(2, Duration::from_secs(10));
        gate.set_pause_notify(Arc::clone(&notify));
        let gate = Arc::new(gate);

        gate.acquire().await;
        gate.acquire().await;

        let notify2 = Arc::clone(&notify);
        let notified = tokio::spawn(async move {
            notify2.notified().await;
            true
        });

        let g = Arc::clone(&gate);
        tokio::spawn(async move { g.acquire().await });

        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(11)).await;

        assert!(notified.await.unwrap());
    }

    #[tokio::test]
    async fn set_pause_notify_through_arc() {
        // Verify set_pause_notify works on an already-Arc'd BudgetGate
        // (interior mutability via std::sync::Mutex).
        tokio::time::pause();
        let gate = Arc::new(BudgetGate::new(2, Duration::from_secs(10)));

        // Attach notify AFTER wrapping in Arc — this is the webhook use case.
        let notify = Arc::new(tokio::sync::Notify::new());
        gate.set_pause_notify(Arc::clone(&notify));

        gate.acquire().await;
        gate.acquire().await;

        let notify2 = Arc::clone(&notify);
        let notified = tokio::spawn(async move {
            notify2.notified().await;
            true
        });

        let g = Arc::clone(&gate);
        tokio::spawn(async move { g.acquire().await });

        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(11)).await;

        assert!(notified.await.unwrap());
    }
}
