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

use std::future::Future;
use std::num::NonZeroU64;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
#[cfg(test)]
use std::sync::Weak;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Outcome of consulting a [`ReplenishPolicy`] at an epoch transition.
///
/// [`Self::Ceiling`] carries a [`NonZeroU64`] so a zero ceiling — which
/// [`BudgetGate::set_epoch_limit`] rejects by panic — has no constructor
/// path through this seam.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replenished {
    /// The guarded resource has replenished; resume the next epoch with
    /// this ceiling.
    Ceiling(NonZeroU64),
    /// Cancellation fired while the policy was waiting.
    Cancelled,
    /// The policy could not establish a replenished ceiling (no
    /// authoritative signal, or the signal was rejected as implausible).
    /// The gate fails closed rather than resuming on a stale ceiling.
    Unavailable,
}

/// Source-agnostic replenish contract for [`BudgetGate`].
///
/// When a policy is attached, it REPLACES the gate's fixed cooldown
/// sleep at an epoch transition: the elected worker calls
/// [`Self::replenish`], which waits until the guarded resource has
/// actually replenished and reports the ceiling for the next epoch. The
/// gate then applies that ceiling and resets the call counter as one
/// transition, so an epoch can never resume on a ceiling sized from a
/// pre-replenish reading.
///
/// Vocabulary is deliberately source-neutral (CHE-0084, CHE-0055:R9,
/// COM-0012:R5): the wait duration and ceiling policy belong to the
/// adapter that owns the upstream resource's semantics, never to this
/// crate.
///
/// Implementations must not construct a runtime (CHE-0055:R5) — they run
/// on the caller's ambient runtime. The returned future is boxed at this
/// boundary to keep the trait dyn-compatible without `#[async_trait]`,
/// which is forbidden fleet-wide.
///
/// # Panics
///
/// A panic inside [`Self::replenish`] is a fail-closed path, not a
/// resume path. The panic unwinds out of [`BudgetGate::acquire`] to the
/// elected caller, and while unwinding the gate's epoch-transition guard
/// runs: it clears the election flag and wakes the parked waiters, so a
/// successor can be elected rather than the epoch deadlocking on a
/// permanently-held election. Neither the call counter nor the epoch
/// ceiling is touched on this path, so no epoch can reopen on a ceiling
/// the policy never established — the successor sees exactly the
/// pre-panic counter and ceiling.
pub trait ReplenishPolicy: Send + Sync + 'static {
    /// Wait for the guarded resource to replenish, then report the
    /// ceiling for the next epoch.
    ///
    /// Must return [`Replenished::Cancelled`] promptly when `cancel`
    /// fires, and [`Replenished::Unavailable`] rather than guessing when
    /// no authoritative replenish signal can be established.
    fn replenish<'a>(
        &'a self,
        cancel: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Replenished> + Send + 'a>>;
}

/// A paired reading of one [`BudgetGate`] epoch's call count and ceiling.
///
/// Best-effort diagnostic snapshot, not a linearizable pair — see
/// [`BudgetGate::epoch_usage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpochUsage {
    /// Calls made in the current epoch at the moment of the read.
    pub calls_made: u64,
    /// The per-epoch ceiling in force at the moment of the read.
    pub epoch_limit: u64,
}

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
    /// Optional replenish policy consulted at each epoch transition.
    ///
    /// When present it replaces the fixed [`Self::wait_duration`] sleep
    /// and supplies the next epoch's ceiling. Uses `std::sync::Mutex`
    /// for the same reason as [`Self::pause_notify`]: attachment after
    /// the gate is wrapped in `Arc`. The lock is held only long enough
    /// to clone the `Arc` — never across an await point.
    replenish_policy: StdMutex<Option<Arc<dyn ReplenishPolicy>>>,
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
            replenish_policy: StdMutex::new(None),
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

    /// Attach a [`ReplenishPolicy`] consulted at every epoch transition.
    ///
    /// Replaces the fixed cooldown sleep: see [`ReplenishPolicy`] for the
    /// contract. Distinct from [`Self::with_pause_notify`], which is an
    /// observability hook delivered with `notify_one` to a single
    /// competing listener and must not be repurposed for control flow.
    #[must_use]
    pub fn with_replenish_policy(self, policy: Arc<dyn ReplenishPolicy>) -> Self {
        *self
            .replenish_policy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(policy);
        self
    }

    /// Set the replenish policy after construction.
    ///
    /// Safe to call on a shared `Arc<BudgetGate>` — uses interior
    /// mutability. Replaces any previously attached policy; a policy
    /// already awaiting inside an in-flight epoch transition keeps running
    /// to completion.
    pub fn set_replenish_policy(&self, policy: Arc<dyn ReplenishPolicy>) {
        *self
            .replenish_policy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(policy);
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
    /// reached, exactly one caller is elected to perform the epoch
    /// transition; the rest wait on `epoch_advanced` without holding any
    /// async mutex across the wait.
    ///
    /// The elected caller either consults an attached [`ReplenishPolicy`]
    /// — which waits for the guarded resource to replenish and supplies
    /// the next epoch's ceiling — or, with no policy attached, sleeps the
    /// fixed `wait_duration` and retains the current ceiling.
    ///
    /// Returns `false` when the epoch did NOT reopen: `cancel` fired
    /// while this caller was parked in the transition, or an attached
    /// policy reported [`Replenished::Unavailable`]. In both cases the
    /// counter is not reset and callers must exit rather than resume work
    /// on a stale ceiling — the seam fails closed.
    #[must_use = "false means the epoch did not reopen; caller must exit, not resume work"]
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
                let policy = self
                    .replenish_policy
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                let reopened = match policy {
                    Some(policy) => match policy.replenish(cancel).await {
                        Replenished::Ceiling(ceiling) => {
                            self.limit.store(ceiling.get(), Ordering::Release);
                            self.calls.store(0, Ordering::Release);
                            true
                        }
                        Replenished::Cancelled => false,
                        Replenished::Unavailable => {
                            warn!(
                                "replenish policy reported no authoritative ceiling; \
                                 holding the budget epoch closed rather than resuming stale"
                            );
                            false
                        }
                    },
                    None => {
                        tokio::select! {
                            () = tokio::time::sleep(self.wait_duration) => {
                                self.calls.store(0, Ordering::Release);
                                true
                            }
                            () = cancel.cancelled() => false,
                        }
                    }
                };
                drop(guard);
                if !reopened {
                    return false;
                }
                info!(
                    epoch_limit = self.limit.load(Ordering::Acquire),
                    "API budget replenished, resuming collection"
                );
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

    /// A paired reading of the current epoch's call count and ceiling.
    ///
    /// Best-effort telemetry: the two counters are read with separate
    /// relaxed loads, so a concurrent [`Self::set_epoch_limit`] or epoch
    /// reset can land between them. Intended for diagnostic log fields,
    /// never for a gating decision — [`Self::acquire`] is the only
    /// authority on whether a call may proceed.
    #[must_use]
    pub fn epoch_usage(&self) -> EpochUsage {
        EpochUsage {
            epoch_limit: self.limit.load(Ordering::Relaxed),
            calls_made: self.calls.load(Ordering::Relaxed),
        }
    }

    /// Cumulative number of calls made across all epochs.
    #[must_use]
    pub fn total_calls_made(&self) -> u64 {
        self.total_calls.load(Ordering::Relaxed)
    }

    /// Refund one previously-acquired permit back to the current epoch.
    ///
    /// For a call that turned out not to count against the resource this
    /// gate protects (e.g. a GitHub 304 Not Modified conditional
    /// revalidation, which does not consume the real upstream rate
    /// limit) — call [`Self::acquire`] up front as usual, then call this
    /// once the outcome is known to be free.
    ///
    /// CAS-based, matching [`Self::acquire`]'s concurrency idiom.
    /// Saturates at 0: never underflows past a concurrent epoch reset
    /// that already zeroed `calls`.
    pub fn refund(&self) {
        loop {
            let current = self.calls.load(Ordering::Acquire);
            if current == 0 {
                return;
            }
            if self
                .calls
                .compare_exchange_weak(current, current - 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return;
            }
        }
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

    fn nz(v: u64) -> NonZeroU64 {
        NonZeroU64::new(v).expect("test ceiling is non-zero")
    }

    /// Records the gate's observable state at the moment the policy is
    /// consulted, then reports a fixed outcome.
    struct RecordingPolicy {
        outcome: Replenished,
        wait: Duration,
        seen: StdMutex<Vec<EpochUsage>>,
        gate: StdMutex<Option<std::sync::Weak<BudgetGate>>>,
    }

    impl RecordingPolicy {
        fn new(outcome: Replenished, wait: Duration) -> Arc<Self> {
            Arc::new(Self {
                outcome,
                wait,
                seen: StdMutex::new(Vec::new()),
                gate: StdMutex::new(None),
            })
        }

        fn watch(&self, gate: &Arc<BudgetGate>) {
            *self.gate.lock().unwrap() = Some(Arc::downgrade(gate));
        }

        fn observations(&self) -> Vec<EpochUsage> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl ReplenishPolicy for RecordingPolicy {
        fn replenish<'a>(
            &'a self,
            cancel: &'a CancellationToken,
        ) -> Pin<Box<dyn Future<Output = Replenished> + Send + 'a>> {
            Box::pin(async move {
                if let Some(gate) = self.gate.lock().unwrap().as_ref().and_then(Weak::upgrade) {
                    self.seen.lock().unwrap().push(gate.epoch_usage());
                }
                tokio::select! {
                    () = tokio::time::sleep(self.wait) => {}
                    () = cancel.cancelled() => return Replenished::Cancelled,
                }
                self.outcome
            })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn replenish_policy_resizes_ceiling_and_resets_counter_as_one_transition() {
        let policy =
            RecordingPolicy::new(Replenished::Ceiling(nz(4900)), Duration::from_secs(3600));
        let gate = Arc::new(
            BudgetGate::new(1, Duration::from_secs(10))
                .with_replenish_policy(Arc::clone(&policy) as Arc<dyn ReplenishPolicy>),
        );
        policy.watch(&gate);
        let cancel = CancellationToken::new();

        assert!(gate.acquire(&cancel).await);

        let g = Arc::clone(&gate);
        let waiter_cancel = cancel.clone();
        let waiter = tokio::spawn(async move { g.acquire(&waiter_cancel).await });

        tokio::time::advance(Duration::from_secs(3599)).await;
        tokio::task::yield_now().await;
        assert!(
            !waiter.is_finished(),
            "the gate must not undercut the policy's stated wait"
        );
        assert_eq!(
            gate.epoch_usage(),
            EpochUsage {
                calls_made: 1,
                epoch_limit: 1
            },
            "no resize may land before the policy resolves"
        );

        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(waiter.await.unwrap());

        assert_eq!(
            gate.epoch_usage(),
            EpochUsage {
                calls_made: 1,
                epoch_limit: 4900
            }
        );
        assert_eq!(
            policy.observations(),
            vec![EpochUsage {
                calls_made: 1,
                epoch_limit: 1
            }],
            "policy is consulted at the transition, before the counter reset"
        );

        let next = tokio::time::timeout(Duration::from_millis(100), gate.acquire(&cancel))
            .await
            .expect("a resized epoch must admit the next permit immediately");
        assert!(next);
        assert_eq!(gate.calls_made(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn replenish_unavailable_fails_closed_without_resetting_the_epoch() {
        let policy = RecordingPolicy::new(Replenished::Unavailable, Duration::from_secs(60));
        let gate = Arc::new(
            BudgetGate::new(1, Duration::from_secs(10))
                .with_replenish_policy(Arc::clone(&policy) as Arc<dyn ReplenishPolicy>),
        );
        let cancel = CancellationToken::new();

        assert!(gate.acquire(&cancel).await);

        let g = Arc::clone(&gate);
        let waiter_cancel = cancel.clone();
        let waiter = tokio::spawn(async move { g.acquire(&waiter_cancel).await });
        tokio::time::advance(Duration::from_secs(61)).await;

        assert!(
            !waiter.await.unwrap(),
            "an unavailable ceiling must fail closed, never resume on the stale one"
        );
        assert_eq!(
            gate.epoch_usage(),
            EpochUsage {
                calls_made: 1,
                epoch_limit: 1
            },
            "neither counter nor ceiling may move when the policy reports Unavailable"
        );
    }

    /// Panics on its first consultation, then records the gate state it
    /// is handed on every later one.
    struct PanicOncePolicy {
        consultations: AtomicU64,
        delay: Duration,
        seen: StdMutex<Vec<EpochUsage>>,
        gate: StdMutex<Option<Weak<BudgetGate>>>,
    }

    impl PanicOncePolicy {
        fn new(delay: Duration) -> Arc<Self> {
            Arc::new(Self {
                consultations: AtomicU64::new(0),
                delay,
                seen: StdMutex::new(Vec::new()),
                gate: StdMutex::new(None),
            })
        }

        fn watch(&self, gate: &Arc<BudgetGate>) {
            *self.gate.lock().unwrap() = Some(Arc::downgrade(gate));
        }

        fn observations(&self) -> Vec<EpochUsage> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl ReplenishPolicy for PanicOncePolicy {
        fn replenish<'a>(
            &'a self,
            _cancel: &'a CancellationToken,
        ) -> Pin<Box<dyn Future<Output = Replenished> + Send + 'a>> {
            Box::pin(async move {
                let nth = self.consultations.fetch_add(1, Ordering::AcqRel);
                if nth == 0 {
                    tokio::time::sleep(self.delay).await;
                    panic!("replenish policy panicked");
                }
                if let Some(gate) = self.gate.lock().unwrap().as_ref().and_then(Weak::upgrade) {
                    self.seen.lock().unwrap().push(gate.epoch_usage());
                }
                Replenished::Unavailable
            })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn replenish_policy_panic_releases_the_election_without_moving_the_epoch() {
        let policy = PanicOncePolicy::new(Duration::from_secs(1));
        let gate = Arc::new(
            BudgetGate::new(1, Duration::from_secs(10))
                .with_replenish_policy(Arc::clone(&policy) as Arc<dyn ReplenishPolicy>),
        );
        policy.watch(&gate);
        let cancel = CancellationToken::new();

        assert!(gate.acquire(&cancel).await);

        let elected_gate = Arc::clone(&gate);
        let elected_cancel = cancel.clone();
        let elected = tokio::spawn(async move { elected_gate.acquire(&elected_cancel).await });
        tokio::task::yield_now().await;

        let successor_gate = Arc::clone(&gate);
        let successor_cancel = cancel.clone();
        let successor =
            tokio::spawn(async move { successor_gate.acquire(&successor_cancel).await });
        tokio::task::yield_now().await;
        assert!(
            !successor.is_finished(),
            "the successor must be parked behind the elected worker, not racing it"
        );

        tokio::time::advance(Duration::from_secs(2)).await;

        let elected_result = elected.await;
        assert!(
            elected_result
                .expect_err("the panic must surface to the elected caller")
                .is_panic(),
            "a panicking policy must unwind out of acquire, never be swallowed"
        );

        let reopened = tokio::time::timeout(Duration::from_secs(5), successor)
            .await
            .expect("the unwind must clear the election and wake a waiter, not deadlock the epoch")
            .expect("the successor must not itself panic");
        assert!(
            !reopened,
            "the successor re-runs the transition; this policy still reports Unavailable"
        );

        assert_eq!(
            policy.observations(),
            vec![EpochUsage {
                calls_made: 1,
                epoch_limit: 1
            }],
            "the panic must leave both counter and ceiling exactly as the successor inherits them"
        );
        assert_eq!(
            gate.epoch_usage(),
            EpochUsage {
                calls_made: 1,
                epoch_limit: 1
            },
            "a panicking policy may never move the epoch counter or ceiling"
        );
    }

    #[tokio::test]
    async fn replenish_policy_honours_cancellation_promptly() {
        let policy =
            RecordingPolicy::new(Replenished::Ceiling(nz(4900)), Duration::from_secs(3600));
        let gate = Arc::new(
            BudgetGate::new(1, Duration::from_secs(3600))
                .with_replenish_policy(Arc::clone(&policy) as Arc<dyn ReplenishPolicy>),
        );
        let pause = Arc::new(Notify::new());
        gate.set_pause_notify(Arc::clone(&pause));
        let cancel = CancellationToken::new();

        assert!(gate.acquire(&cancel).await);

        let g = Arc::clone(&gate);
        let waiter_cancel = cancel.clone();
        let waiter = tokio::spawn(async move { g.acquire(&waiter_cancel).await });

        pause.notified().await;
        cancel.cancel();

        let acquired = tokio::time::timeout(Duration::from_millis(100), waiter)
            .await
            .expect("a cancelled replenish must return promptly")
            .expect("waiter task should not panic");
        assert!(!acquired);
        assert_eq!(
            gate.epoch_usage(),
            EpochUsage {
                calls_made: 1,
                epoch_limit: 1
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn no_replenish_policy_retains_legacy_fixed_cooldown() {
        let gate = Arc::new(BudgetGate::new(2, Duration::from_secs(10)));
        let cancel = CancellationToken::new();

        assert!(gate.acquire(&cancel).await);
        assert!(gate.acquire(&cancel).await);

        let g = Arc::clone(&gate);
        let waiter_cancel = cancel.clone();
        let waiter = tokio::spawn(async move { g.acquire(&waiter_cancel).await });
        tokio::time::advance(Duration::from_secs(11)).await;

        assert!(waiter.await.unwrap());
        assert_eq!(
            gate.epoch_usage(),
            EpochUsage {
                calls_made: 1,
                epoch_limit: 2
            }
        );
    }

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
            let _ = gate2.acquire(&waiter_cancel).await;
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
                let _ = g.acquire(&worker_cancel).await;
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

    #[tokio::test]
    async fn refund_decrements_calls_but_not_total_calls() {
        let gate = BudgetGate::new(5, Duration::from_secs(1));
        let cancel = CancellationToken::new();
        assert!(gate.acquire(&cancel).await);
        assert!(gate.acquire(&cancel).await);
        assert_eq!(gate.calls_made(), 2);

        gate.refund();

        assert_eq!(gate.calls_made(), 1);
        assert_eq!(
            gate.total_calls_made(),
            2,
            "total_calls_made is a lifetime audit trail, unaffected by refund"
        );
    }

    #[test]
    fn refund_on_empty_gate_saturates_at_zero() {
        let gate = BudgetGate::new(5, Duration::from_secs(1));
        gate.refund();
        assert_eq!(gate.calls_made(), 0);
    }

    #[tokio::test]
    async fn refund_never_underflows_below_zero_after_epoch_reset() {
        tokio::time::pause();
        let gate = Arc::new(BudgetGate::new(1, Duration::from_mins(1)));
        let cancel = CancellationToken::new();

        assert!(gate.acquire(&cancel).await);
        assert_eq!(gate.calls_made(), 1);

        let g = Arc::clone(&gate);
        let waiter_cancel = cancel.clone();
        let handle = tokio::spawn(async move { g.acquire(&waiter_cancel).await });
        tokio::time::advance(Duration::from_secs(61)).await;
        assert!(handle.await.unwrap());
        assert_eq!(gate.calls_made(), 1);

        gate.refund();
        gate.refund();
        assert_eq!(
            gate.calls_made(),
            0,
            "a second refund past zero must saturate, never wrap/underflow"
        );
    }

    #[tokio::test]
    async fn concurrent_refund_never_underflows() {
        let gate = Arc::new(BudgetGate::new(20, Duration::from_secs(1)));
        let cancel = CancellationToken::new();
        for _ in 0..20 {
            assert!(gate.acquire(&cancel).await);
        }
        assert_eq!(gate.calls_made(), 20);

        let mut handles = Vec::new();
        for _ in 0..30 {
            let g = Arc::clone(&gate);
            handles.push(tokio::spawn(async move { g.refund() }));
        }
        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(
            gate.calls_made(),
            0,
            "30 refunds against 20 acquired permits must saturate at 0, never underflow"
        );
    }
}
