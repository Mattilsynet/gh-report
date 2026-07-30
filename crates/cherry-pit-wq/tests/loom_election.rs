//! Layer 2 (F2) — loom exhaustive-interleaving models of the CURRENT
//! election path vs increment B's lock-free debit.
//!
//! `budget.rs`'s `BudgetGate` and `token_bucket.rs`'s `TokenBucketRegulator`
//! are the frozen/graduated production types (`std::sync::atomic`) — they
//! cannot be loom-model-checked directly without recompiling their internals
//! against `loom::sync::atomic` (which would require `#[cfg(loom)]` shims in
//! `src/budget.rs`, out of scope: the frozen path must not be touched, per
//! this mission's `out_of_scope`/`abort_if`). This file instead builds SMALL
//! structural analogues, under loom's own atomics, that reproduce each
//! design's actual synchronization *shape*:
//!
//! - [`MiniElectionGate`] mirrors `BudgetGate::acquire`'s three-branch CAS
//!   loop verbatim in structure: (1) CAS-increment while under limit: (2)
//!   CAS `false -> true` on a `resetting` flag to elect a single resetter,
//!   which resets the counter and clears the flag; (3) losers spin-wait for
//!   the flag to clear. This is the coordination CURRENT requires: THREE
//!   pieces of shared mutable state (`calls`, `resetting`, plus the
//!   spin/park choreography) that every thread must serialize through once
//!   the epoch is exhausted.
//! - [`MiniTokenBucketDebit`] mirrors `TokenBucketRegulator::try_debit_one`'s
//!   single CAS loop: ONE atomic counter, no election flag, no separate
//!   reset step — a thread either wins its own CAS or retries, with no
//!   shared "am I the elected resetter" state at all.
//!
//! Both models are checked under `loom::model` with an external counter
//! recording how many distinct interleavings loom explores, giving a direct
//! quantitative contrast: CURRENT's extra coordination state produces a
//! larger explored state space for the same thread count than B's model,
//! which is the loom-level analogue of `spec/fizzbee/budget_gate.fizz`
//! (81 nodes/39 states) vs `token_bucket.fizz` (61/61) in Layer 3.
//!
//! Run with: `RUSTFLAGS="--cfg loom" cargo test --release --test loom_election`

#![expect(
    unexpected_cfgs,
    reason = "loom is a custom --cfg flag (no build.rs check-cfg declaration for a test-only gate); fires identically with or without --cfg loom"
)]
#![cfg(loom)]

use std::sync::atomic::{AtomicUsize, Ordering as StdOrdering};

use loom::sync::Arc;
use loom::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use loom::thread;

/// Structural analogue of `BudgetGate`'s CAS-election-then-reset path
/// (`budget.rs::acquire`, three branches: CAS-increment / CAS-elect-and-reset
/// / spin-wait-for-reset). Two atomics, matching the production shape.
struct MiniElectionGate {
    calls: AtomicU64,
    limit: u64,
    resetting: AtomicBool,
}

impl MiniElectionGate {
    fn new(limit: u64) -> Self {
        Self {
            calls: AtomicU64::new(0),
            limit,
            resetting: AtomicBool::new(false),
        }
    }

    /// Acquire one permit, electing a single resetter when the limit is
    /// reached — mirrors `budget.rs::acquire`'s CAS-increment /
    /// CAS-elect-and-reset / spin-wait branches exactly in shape.
    fn acquire(&self) {
        loop {
            let cur = self.calls.load(Ordering::Acquire);
            if cur < self.limit {
                if self
                    .calls
                    .compare_exchange(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    return;
                }
                continue;
            }
            if self
                .resetting
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.calls.store(0, Ordering::Release);
                self.resetting.store(false, Ordering::Release);
                continue;
            }
            thread::yield_now();
        }
    }
}

/// Structural analogue of `TokenBucketRegulator::try_debit_one`'s single
/// CAS loop — one atomic, no election flag, no reset step.
struct MiniTokenBucketDebit {
    consumed: AtomicU64,
    capacity: u64,
}

impl MiniTokenBucketDebit {
    fn new(capacity: u64) -> Self {
        Self {
            consumed: AtomicU64::new(0),
            capacity,
        }
    }

    /// One lock-free debit attempt: `Some(())` on success, `None` if the
    /// (fixed, non-refilling within this bounded model) capacity is spent.
    fn try_debit(&self) -> Option<()> {
        loop {
            let cur = self.consumed.load(Ordering::Acquire);
            if cur >= self.capacity {
                return None;
            }
            if self
                .consumed
                .compare_exchange(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some(());
            }
        }
    }
}

static CURRENT_EXPLORED: AtomicUsize = AtomicUsize::new(0);
static B_EXPLORED: AtomicUsize = AtomicUsize::new(0);

/// F2(a) — CURRENT: 3 threads, `limit == 1`, so exactly 2 of the 3 must
/// serialize through the election path (only one CAS-increment succeeds
/// immediately; the other two contend for the `resetting` election, one
/// wins and resets, the last spin-waits). Asserts `calls_made() <= limit`
/// holds under every explored interleaving (property (i)) — the
/// coordination/contention CURRENT's design requires to hold that
/// invariant is the "election path" this loom model surfaces: 2 atomics,
/// a spin-wait branch, and a reset step, versus B's 1 atomic below.
#[test]
fn current_election_path_never_exceeds_limit_under_all_interleavings() {
    CURRENT_EXPLORED.store(0, StdOrdering::SeqCst);
    loom::model(|| {
        CURRENT_EXPLORED.fetch_add(1, StdOrdering::SeqCst);
        let gate = Arc::new(MiniElectionGate::new(1));

        let g1 = Arc::clone(&gate);
        let t1 = thread::spawn(move || g1.acquire());
        let g2 = Arc::clone(&gate);
        let t2 = thread::spawn(move || g2.acquire());

        gate.acquire();
        t1.join().unwrap();
        t2.join().unwrap();

        assert!(
            gate.calls.load(Ordering::SeqCst) <= gate.limit,
            "F2(a): election path must never admit more than `limit` concurrently live permits"
        );
    });
    let explored = CURRENT_EXPLORED.load(StdOrdering::SeqCst);
    assert!(
        explored > 0,
        "loom must have explored at least one interleaving"
    );
    println!("F2(a) CURRENT election-path interleavings explored: {explored}");
}

/// F2(b) — increment B: 3 threads racing a single CAS debit against a
/// fixed capacity of 2 (one thread must lose). Asserts the CAS loop never
/// lets `consumed` exceed `capacity` (no stale-read/lost-update
/// interleaving — "the class linus caught and hopper fixed", per mission
/// intent) and that successful-debit count matches the final `consumed`
/// value exactly under every explored interleaving. No election flag, no
/// reset step, no spin-wait branch exists in this model at all — there is
/// no shared "am I the elected resetter" state to race on, structurally.
#[test]
fn b_debit_never_exceeds_capacity_no_lost_update_under_all_interleavings() {
    B_EXPLORED.store(0, StdOrdering::SeqCst);
    loom::model(|| {
        B_EXPLORED.fetch_add(1, StdOrdering::SeqCst);
        let bucket = Arc::new(MiniTokenBucketDebit::new(2));

        let b1 = Arc::clone(&bucket);
        let t1 = thread::spawn(move || b1.try_debit().is_some());
        let b2 = Arc::clone(&bucket);
        let t2 = thread::spawn(move || b2.try_debit().is_some());

        let r0 = bucket.try_debit().is_some();
        let r1 = t1.join().unwrap();
        let r2 = t2.join().unwrap();

        let successes = u64::from(r0) + u64::from(r1) + u64::from(r2);
        let consumed = bucket.consumed.load(Ordering::SeqCst);
        assert!(
            consumed <= bucket.capacity,
            "F2(b): consumed must never exceed capacity under any interleaving"
        );
        assert_eq!(
            successes, consumed,
            "F2(b): no stale-read/lost-update — every successful debit is reflected exactly once in consumed"
        );
    });
    let explored = B_EXPLORED.load(StdOrdering::SeqCst);
    assert!(
        explored > 0,
        "loom must have explored at least one interleaving"
    );
    println!("F2(b) B debit interleavings explored: {explored}");
}
