//! Composable admission/settlement seam for the worker pool.
//!
//! [`Regulator`] generalises the pacing role that [`BudgetGate`] and
//! [`RateLimitState`] already play in [`run_worker_pool`](crate::run_worker_pool),
//! as an ordered, dynamically-dispatched layer: [`run_worker_pool_regulated`]
//! walks an ordered `&[Arc<dyn Regulator>]`, calling [`Regulator::admit`]
//! before executing a job and [`Regulator::settle`] after.
//!
//! The admit/settle split generalises [`BudgetGate::refund`]: a job that
//! turns out to be free (no real charge against the guarded resource)
//! settles as [`SettleOutcome::Free`], releasing the permit it admitted
//! under, rather than leaving it charged.
//!
//! Vocabulary here is generic (admit/settle/reject) — no GitHub/HTTP terms
//! (CHE-0084, CHE-0055:R9). Concrete adapters ([`BudgetRegulator`],
//! [`RateLimitRegulator`]) wrap the existing primitives; they construct
//! no runtime (CHE-0055:R5) and run on the caller's ambient runtime.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::budget::BudgetGate;
use crate::rate_limit::RateLimitState;

/// Outcome of a [`Regulator::admit`] call.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// A permit was granted; the caller must eventually call
    /// [`Regulator::settle`] once the job's outcome is known.
    Admitted,
    /// The caller should stop waiting and not proceed with the job — e.g.
    /// cancellation fired while parked on this regulator.
    Cancelled,
}

/// What to report back to a [`Regulator`] once an admitted job has run.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettleOutcome {
    /// The admitted call genuinely consumed the guarded resource; the
    /// permit stays charged.
    Charged,
    /// The admitted call turned out not to consume the guarded resource
    /// (e.g. a conditional revalidation with no real upstream spend);
    /// the regulator releases the permit it granted.
    Free,
}

/// A composable admission/settlement gate in the worker pool's ordered
/// regulator chain.
///
/// Implementations must not construct a runtime (CHE-0055:R5) — they run
/// on the caller's ambient runtime. `admit` is asynchronous (an
/// implementation may need to park a caller, e.g. during a cooldown or a
/// rate-limit halt) but the trait stays dyn-compatible by boxing the
/// future explicitly at this boundary rather than requiring
/// `#[async_trait]`, which is forbidden fleet-wide (async-trait CI
/// tripwire).
pub trait Regulator: Send + Sync + 'static {
    /// Request a permit. Resolves to [`Admission::Admitted`] once granted,
    /// or [`Admission::Cancelled`] if `cancel` fired while waiting.
    fn admit<'a>(
        &'a self,
        cancel: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Admission> + Send + 'a>>;

    /// Report the outcome of a job that was admitted by this regulator.
    ///
    /// Default: no-op. Override only when the regulator carries a charge
    /// concept (e.g. [`BudgetRegulator`], which releases a permit on
    /// [`SettleOutcome::Free`]); a regulator with no charge concept (rate
    /// limiting, backoff) relies on this default.
    fn settle(&self, _outcome: SettleOutcome) {}
}

/// [`Regulator`] adapter over [`BudgetGate`].
///
/// `admit` acquires a budget permit (may park during an epoch-cooldown
/// sleep); `settle(SettleOutcome::Free)` generalises
/// [`BudgetGate::refund`] — releasing the permit for a call that turned
/// out not to spend real resource.
pub struct BudgetRegulator {
    gate: Arc<BudgetGate>,
}

impl BudgetRegulator {
    /// Wrap an existing [`BudgetGate`] as a [`Regulator`].
    #[must_use]
    pub fn new(gate: Arc<BudgetGate>) -> Self {
        Self { gate }
    }
}

impl Regulator for BudgetRegulator {
    fn admit<'a>(
        &'a self,
        cancel: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Admission> + Send + 'a>> {
        Box::pin(async move {
            if self.gate.acquire(cancel).await {
                Admission::Admitted
            } else {
                Admission::Cancelled
            }
        })
    }

    fn settle(&self, outcome: SettleOutcome) {
        if outcome == SettleOutcome::Free {
            self.gate.refund();
        }
    }
}

/// [`Regulator`] adapter over [`RateLimitState`].
///
/// `admit` returns immediately when the observer is not halted, or parks
/// (via [`crate::worker_pool::wait_for_rate_limit_reset`]) until the halt
/// clears or `cancel` fires. Rate limiting carries no charge concept, so
/// `settle` is a no-op — this regulator never rejects retroactively.
pub struct RateLimitRegulator {
    state: Arc<RateLimitState>,
}

impl RateLimitRegulator {
    /// Wrap an existing [`RateLimitState`] as a [`Regulator`].
    #[must_use]
    pub fn new(state: Arc<RateLimitState>) -> Self {
        Self { state }
    }
}

impl Regulator for RateLimitRegulator {
    fn admit<'a>(
        &'a self,
        cancel: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Admission> + Send + 'a>> {
        Box::pin(async move {
            if !self.state.should_halt() {
                return Admission::Admitted;
            }
            if crate::worker_pool::wait_for_rate_limit_reset(&self.state, cancel).await {
                Admission::Admitted
            } else {
                Admission::Cancelled
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn dyn_regulators(regs: Vec<Arc<dyn Regulator>>) -> Vec<Arc<dyn Regulator>> {
        regs
    }

    #[test]
    fn regulator_trait_is_dyn_compatible() {
        let budget = Arc::new(BudgetRegulator::new(Arc::new(BudgetGate::new(
            10,
            Duration::from_secs(1),
        ))));
        let rate = Arc::new(RateLimitRegulator::new(Arc::new(RateLimitState::default())));
        let regs: Vec<Arc<dyn Regulator>> = dyn_regulators(vec![budget, rate]);
        assert_eq!(regs.len(), 2);
    }

    #[tokio::test]
    async fn budget_regulator_admit_grants_within_limit() {
        let gate = Arc::new(BudgetGate::new(5, Duration::from_secs(1)));
        let regulator = BudgetRegulator::new(Arc::clone(&gate));
        let cancel = CancellationToken::new();
        assert_eq!(regulator.admit(&cancel).await, Admission::Admitted);
        assert_eq!(gate.calls_made(), 1);
    }

    #[tokio::test]
    async fn rate_limit_regulator_admit_grants_when_not_halted() {
        let state = Arc::new(RateLimitState::default());
        let regulator = RateLimitRegulator::new(Arc::clone(&state));
        let cancel = CancellationToken::new();
        assert_eq!(regulator.admit(&cancel).await, Admission::Admitted);
    }

    /// F1-class hardening: an admitted call that settles as
    /// [`SettleOutcome::Free`] releases its permit — generalising
    /// [`BudgetGate::refund`] through the admit/settle vocabulary rather
    /// than requiring callers to reach into the gate directly.
    #[tokio::test]
    async fn budget_regulator_settle_free_releases_permit() {
        let gate = Arc::new(BudgetGate::new(2, Duration::from_secs(1)));
        let regulator = BudgetRegulator::new(Arc::clone(&gate));
        let cancel = CancellationToken::new();

        assert_eq!(regulator.admit(&cancel).await, Admission::Admitted);
        assert_eq!(gate.calls_made(), 1);

        regulator.settle(SettleOutcome::Free);
        assert_eq!(
            gate.calls_made(),
            0,
            "a Free settlement must release the permit it admitted under"
        );
    }

    /// Contrast case: settling Charged leaves the permit consumed.
    #[tokio::test]
    async fn budget_regulator_settle_charged_keeps_permit_consumed() {
        let gate = Arc::new(BudgetGate::new(2, Duration::from_secs(1)));
        let regulator = BudgetRegulator::new(Arc::clone(&gate));
        let cancel = CancellationToken::new();

        assert_eq!(regulator.admit(&cancel).await, Admission::Admitted);
        regulator.settle(SettleOutcome::Charged);
        assert_eq!(gate.calls_made(), 1);
    }
}
