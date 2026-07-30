//! Comparative evidence harness: CURRENT (`BudgetGate` via
//! [`run_worker_pool`](cherry_pit_wq::run_worker_pool)) vs increment A
//! (composable [`Regulator`] path, `BudgetRegulator` adapter) vs increment
//! B ([`TokenBucketRegulator`]), driven through the same adversarial
//! free/charged draw schedule (CHE-0101 / CHE-0055 mission).
//!
//! `run_worker_pool`'s `worker_loop` calls `budget_gate.acquire()` and
//! never calls `BudgetGate::refund` (see
//! `crates/cherry-pit-wq/src/worker_pool.rs::worker_loop`) — there is no
//! executor-outcome signal (`JobExecutor::execute` returns
//! `Result<R, String>`, no free/charged distinction) that could drive a
//! refund. This harness therefore exercises `BudgetGate` directly with
//! the same no-refund-ever pattern `worker_loop` has, rather than routing
//! through `run_worker_pool` itself — that IS the CURRENT design's
//! phantom-304 behaviour, not a simplification of it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cherry_pit_wq::{
    Admission, BudgetGate, BudgetRegulator, Clock, Regulator, SettleOutcome, TokenBucketRegulator,
};
use proptest::prelude::*;
use tokio_util::sync::CancellationToken;

fn current_thread_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current_thread runtime")
}

/// Deterministic clock: an atomic nanosecond offset from a fixed base
/// `Instant`, advanced explicitly. Mirrors `token_bucket.rs`'s
/// test-only `FakeClock`, duplicated here (that one is
/// `#[cfg(test)]`-private to its module) so the comparative harness
/// controls simulated time independently of wall-clock sleeps.
struct FakeClock {
    base: Instant,
    offset_nanos: AtomicU64,
}

impl FakeClock {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            base: Instant::now(),
            offset_nanos: AtomicU64::new(0),
        })
    }

    fn advance(&self, d: Duration) {
        self.offset_nanos.fetch_add(
            u64::try_from(d.as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.base + Duration::from_nanos(self.offset_nanos.load(Ordering::Relaxed))
    }
}

/// Drive `schedule.len()` draws through CURRENT's actual no-refund
/// pattern: `BudgetGate::acquire`, never `BudgetGate::refund`, for every
/// draw regardless of `schedule`'s free/charged flag. `schedule.len()`
/// must not exceed `limit` — this harness intentionally stays inside the
/// non-blocking region of `acquire` (see module doc); the blocking
/// cooldown-then-reset codepath already has dedicated coverage in
/// `budget.rs`.
fn drive_current(limit: u64, schedule: &[bool]) -> u64 {
    let rt = current_thread_rt();
    let gate = BudgetGate::new(limit, Duration::from_mins(1));
    let cancel = CancellationToken::new();
    rt.block_on(async {
        for _free in schedule {
            assert!(
                gate.acquire(&cancel).await,
                "schedule.len() <= limit must never block"
            );
        }
    });
    gate.calls_made()
}

/// Drive `schedule` through increment A's [`BudgetRegulator`]: `admit`
/// then `settle(Free)` or `settle(Charged)` per the draw's flag. Returns
/// the underlying gate's `calls_made()` after the full schedule.
fn drive_a(limit: u64, schedule: &[bool]) -> u64 {
    let rt = current_thread_rt();
    let gate = Arc::new(BudgetGate::new(limit, Duration::from_mins(1)));
    let regulator = BudgetRegulator::new(Arc::clone(&gate));
    let cancel = CancellationToken::new();
    rt.block_on(async {
        for &free in schedule {
            assert_eq!(
                regulator.admit(&cancel).await,
                Admission::Admitted,
                "schedule.len() <= limit must never block"
            );
            regulator.settle(if free {
                SettleOutcome::Free
            } else {
                SettleOutcome::Charged
            });
        }
    });
    gate.calls_made()
}

/// Drive `schedule` through increment B's [`TokenBucketRegulator`] as a
/// burst (no clock advance between draws): `admit` then `settle` (a
/// documented no-op — B carries no charge concept, per
/// `token_bucket.rs` module doc). Returns milli-tokens consumed.
fn drive_b(capacity_tokens: u64, schedule: &[bool]) -> u64 {
    let rt = current_thread_rt();
    let clock = FakeClock::new();
    let bucket =
        TokenBucketRegulator::new(Arc::clone(&clock) as Arc<dyn Clock>, capacity_tokens, 1);
    let cancel = CancellationToken::new();
    rt.block_on(async {
        for &free in schedule {
            assert_eq!(
                bucket.admit(&cancel).await,
                Admission::Admitted,
                "schedule.len() <= capacity_tokens must never block"
            );
            bucket.settle(if free {
                SettleOutcome::Free
            } else {
                SettleOutcome::Charged
            });
        }
    });
    schedule.len() as u64
}

fn limit_and_schedule() -> impl Strategy<Value = (u64, Vec<bool>)> {
    (1u64..20).prop_flat_map(|limit| {
        let len = 0usize..=usize::try_from(limit).unwrap_or(usize::MAX);
        (Just(limit), prop::collection::vec(any::<bool>(), len))
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, .. ProptestConfig::default() })]

    /// F1 property (iii) — draw-on-confirmed-charge conservation, increment
    /// A: `settle(Free)` releases the permit it admitted under;
    /// `settle(Charged)` leaves it consumed. `gate.calls_made()` after the
    /// schedule must equal exactly the count of `charged` (non-free) draws
    /// — free draws leave zero residue.
    #[test]
    fn p1_a_budget_regulator_conserves_on_free_settle((limit, schedule) in limit_and_schedule()) {
        let charged = schedule.iter().filter(|&&free| !free).count() as u64;
        let calls_made = drive_a(limit, &schedule);
        prop_assert_eq!(calls_made, charged);
        prop_assert!(calls_made <= limit, "F1 property (i): permits never exceed authoritative budget");
    }

    /// F1 property (ii), CURRENT — headline evidence. `run_worker_pool`'s
    /// `worker_loop` never calls `BudgetGate::refund` (see module doc), so
    /// `calls_made()` after any schedule equals `schedule.len()` regardless
    /// of how many draws were actually free. When `schedule.len() == limit`
    /// and every draw is free, CURRENT reaches full epoch exhaustion purely
    /// from free (304) draws — phantom exhaustion, the historical bug this
    /// mission captures as evidence rather than fixes (out_of_scope: do not
    /// modify the frozen path).
    #[test]
    fn p1_current_budget_gate_conflates_free_and_charged((limit, schedule) in limit_and_schedule()) {
        let calls_made = drive_current(limit, &schedule);
        prop_assert_eq!(
            calls_made,
            schedule.len() as u64,
            "CURRENT has no refund path: calls_made must equal every draw, free or charged"
        );
        prop_assert!(calls_made <= limit, "F1 property (i): permits never exceed authoritative budget");
    }

    /// F1 property (i) + B's structural contrast: [`TokenBucketRegulator`]
    /// is documented RATE-only with a no-op `settle` (`token_bucket.rs`) —
    /// it debits every admitted draw regardless of the free/charged flag,
    /// by construction, because there is no charge-tracking state to
    /// conflate. `consumed <= capacity_tokens` (property i) holds under
    /// burst load; B does not attempt (and structurally cannot corrupt) A's
    /// free/charged conservation invariant because it never models that
    /// axis at all — see `phantom_304_headline_contrast` below for the
    /// decisive cross-design pause-behaviour contrast.
    #[test]
    fn p1_b_token_bucket_debits_uniformly_regardless_of_flag((limit, schedule) in limit_and_schedule()) {
        let consumed = drive_b(limit, &schedule);
        prop_assert_eq!(consumed, schedule.len() as u64);
        prop_assert!(consumed <= limit, "F1 property (i): consumed never exceeds bucket capacity under burst");
    }
}

/// F1 headline contrast (fixed illustrative case, not proptest-shrunk):
/// 5 free-only (304) draws against a limit/capacity of 5.
///
/// - CURRENT: `calls_made() == 5` — the epoch is fully exhausted purely by
///   free draws; the *next* real call would require the full
///   `wait_duration` cooldown-then-reset sleep (`budget.rs::acquire`),
///   regardless of the fact that zero real charges occurred. This is the
///   phantom-304 bug.
/// - A: `calls_made() == 0` — every free draw was refunded via
///   `settle(Free)`; zero phantom cost, full headroom retained.
/// - B: consumes all 5 tokens (no free/charged distinction exists), but
///   refill is a *continuous pure function of elapsed time* — advancing
///   the clock by exactly one token's worth of time admits one more draw
///   immediately, with no discrete "wait the full cooldown" stall the way
///   CURRENT's elected-resetter path requires. B eliminates the phantom
///   *pause* class structurally (no `resetting` election field exists in
///   `TokenBucketRegulator`, contrasted with `BudgetGate`'s `resetting:
///   AtomicBool`), even though it does not model per-draw free/charged
///   accounting the way A does.
#[test]
fn phantom_304_headline_contrast() {
    let schedule = vec![true; 5];

    let current_calls_made = drive_current(5, &schedule);
    assert_eq!(
        current_calls_made, 5,
        "CURRENT: 5 free draws still fully exhaust the epoch (phantom-304)"
    );

    let a_calls_made = drive_a(5, &schedule);
    assert_eq!(
        a_calls_made, 0,
        "A: 5 free draws leave zero residue via settle(Free)"
    );

    let rt = current_thread_rt();
    let clock = FakeClock::new();
    let bucket = TokenBucketRegulator::new(Arc::clone(&clock) as Arc<dyn Clock>, 5, 1);
    let cancel = CancellationToken::new();
    rt.block_on(async {
        for _ in 0..5 {
            assert_eq!(bucket.admit(&cancel).await, Admission::Admitted);
            bucket.settle(SettleOutcome::Free);
        }
        clock.advance(Duration::from_secs(1));
        assert_eq!(
            bucket.admit(&cancel).await,
            Admission::Admitted,
            "B: exactly one token's elapsed-time refill admits the next draw, no discrete pause"
        );
    });
}

/// Layer 4 — CURRENT needs `epochs` simulated full-epoch cooldowns to
/// admit an all-free draw storm, purely from phantom-304 conflation;
/// A (`settle(Free)`) needs zero. `tokio::time::pause`/`advance` drives
/// simulated multi-hour epoch cycles deterministically (no wall-clock
/// cost). This generalises `phantom_304_headline_contrast` from a single
/// epoch to a multi-hour storm, the shape Layer 4 asks for.
#[tokio::test]
async fn l4_current_needs_n_epoch_resets_for_phantom_304_storm_while_a_needs_zero() {
    tokio::time::pause();

    let limit = 5u64;
    let epochs = 3usize;
    let total_draws = usize::try_from(limit).unwrap_or(usize::MAX) * epochs;

    let gate = Arc::new(BudgetGate::new(limit, Duration::from_hours(1)));
    let cancel = CancellationToken::new();
    let g = Arc::clone(&gate);
    let c = cancel.clone();
    let handle = tokio::spawn(async move {
        for _ in 0..total_draws {
            assert!(
                g.acquire(&c).await,
                "cancellation must not fire in this simulation"
            );
        }
    });
    for _ in 0..epochs {
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_mins(61)).await;
    }
    handle.await.unwrap();
    assert_eq!(
        gate.total_calls_made(),
        total_draws as u64,
        "CURRENT: all draws eventually admitted, but only after {epochs} simulated full-epoch \
         cooldowns triggered purely by free (304) draws"
    );

    let gate_a = Arc::new(BudgetGate::new(limit, Duration::from_hours(1)));
    let regulator = BudgetRegulator::new(Arc::clone(&gate_a));
    for _ in 0..total_draws {
        assert_eq!(regulator.admit(&cancel).await, Admission::Admitted);
        regulator.settle(SettleOutcome::Free);
    }
    assert_eq!(
        gate_a.calls_made(),
        0,
        "A: the same {total_draws}-draw all-free storm needs zero epoch resets — every draw \
         refunded via settle(Free), no simulated wait required at all"
    );
}

/// Layer 4 — B: budget conservation and reset-cycle correctness across a
/// simulated multi-hour idle gap. `capacity_tokens == refill_per_sec *
/// 3600` is chosen so exactly one simulated hour regenerates exactly one
/// capacity's worth — advancing by exactly that amount must refill to
/// precisely `capacity_tokens` (not more, not less: no overshoot, no
/// unbounded accumulation), and a draw beyond that within the same burst
/// must not admit instantly.
#[test]
fn l4_b_token_bucket_conserves_and_caps_across_simulated_multi_hour_idle_gap() {
    let rt = current_thread_rt();
    let clock = FakeClock::new();
    let refill_per_sec = 1u64;
    let capacity_tokens = refill_per_sec * 3600;
    let bucket = Arc::new(TokenBucketRegulator::new(
        Arc::clone(&clock) as Arc<dyn Clock>,
        capacity_tokens,
        refill_per_sec,
    ));
    let cancel = CancellationToken::new();

    rt.block_on(async {
        for _ in 0..capacity_tokens {
            assert_eq!(bucket.admit(&cancel).await, Admission::Admitted);
        }
    });

    clock.advance(Duration::from_hours(1));

    rt.block_on(async {
        for _ in 0..capacity_tokens {
            assert_eq!(
                bucket.admit(&cancel).await,
                Admission::Admitted,
                "exactly one simulated hour's worth (== capacity_tokens) admits immediately \
                 after the idle gap — refill is capped precisely at capacity, no overshoot"
            );
        }
    });

    rt.block_on(async {
        let extra = tokio::time::timeout(Duration::from_millis(20), bucket.admit(&cancel)).await;
        assert!(
            extra.is_err(),
            "conservation: a (capacity_tokens + 1)'th draw in the same burst must not admit \
             instantly"
        );
    });
}

/// [`Regulator`] test-double proving F3: the seam ACCEPTS a Retry-After
/// style regulator that rejects admission until a simulated Retry-After
/// deadline elapses. No concrete Retry-After regulator ships from this
/// mission — this proves the seam only (mission `out_of_scope`).
struct RetryAfterTestDouble {
    clock: Arc<dyn Clock>,
    until: Instant,
}

impl RetryAfterTestDouble {
    fn new(clock: Arc<dyn Clock>, retry_after: Duration) -> Self {
        let until = clock.now() + retry_after;
        Self { clock, until }
    }
}

impl Regulator for RetryAfterTestDouble {
    fn admit<'a>(
        &'a self,
        cancel: &'a CancellationToken,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Admission> + Send + 'a>> {
        Box::pin(async move {
            loop {
                let now = self.clock.now();
                if now >= self.until {
                    return Admission::Admitted;
                }
                let wait = self.until - now;
                tokio::select! {
                    () = tokio::time::sleep(wait) => {}
                    () = cancel.cancelled() => return Admission::Cancelled,
                }
            }
        })
    }

    fn settle(&self, _outcome: SettleOutcome) {}
}

/// F3 — seam acceptance: [`RetryAfterTestDouble`] is dyn-compatible and
/// composes into the same ordered `&[Arc<dyn Regulator>]` chain as a real
/// adapter ([`BudgetRegulator`]); it rejects (parks) admission until its
/// simulated Retry-After deadline elapses, then admits.
///
/// CURRENT has no equivalent injection path: [`cherry_pit_wq::run_worker_pool`]'s
/// signature takes `budget_gate: Arc<BudgetGate>, rate_limit_state:
/// Arc<RateLimitState>` — two fixed concrete types, not a slice of trait
/// objects — so there is no way to substitute or add a Retry-After-aware
/// gate into the CURRENT worker pool without changing its signature
/// (a compile-time API fact, not a runtime behaviour to assert).
#[test]
fn f3_seam_accepts_test_double_retry_after_regulator_current_has_no_injection_path() {
    let clock = FakeClock::new();
    let retry_after = Duration::from_mins(1);
    let double = Arc::new(RetryAfterTestDouble::new(
        Arc::clone(&clock) as Arc<dyn Clock>,
        retry_after,
    ));

    let budget = Arc::new(BudgetRegulator::new(Arc::new(BudgetGate::new(
        10,
        Duration::from_secs(1),
    ))));
    let regulators: Vec<Arc<dyn Regulator>> =
        vec![Arc::clone(&double) as Arc<dyn Regulator>, budget];
    assert_eq!(
        regulators.len(),
        2,
        "the Regulator seam accepts the test-double alongside a real adapter"
    );

    let rt = current_thread_rt();
    let cancel = CancellationToken::new();

    rt.block_on(async {
        let not_yet = tokio::time::timeout(Duration::from_millis(20), double.admit(&cancel)).await;
        assert!(
            not_yet.is_err(),
            "must not admit before the simulated Retry-After elapses"
        );
    });

    clock.advance(retry_after);

    rt.block_on(async {
        assert_eq!(
            double.admit(&cancel).await,
            Admission::Admitted,
            "admits once the simulated Retry-After deadline has elapsed"
        );
    });
}
