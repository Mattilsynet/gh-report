//! Bounded per-run budget-consumption checkpoints.
//!
//! The per-request events emitted at the GitHub chokepoint are `debug`
//! (COM-0031:R4), so they are silent at the default log level. This module
//! provides the one INFO-level signal that makes a budget-exhaustion
//! incident answerable from default-level logs alone: a milestone crossed
//! at fixed fractions of the epoch ceiling, carrying the top consuming
//! routes.
//!
//! Volume is `O(thresholds)`, not `O(requests)` — [`MAX_CHECKPOINTS_PER_RUN`]
//! is a hard per-run cap enforced by a monotone threshold cursor, so this is
//! an operational state transition rather than per-request chatter.
//!
//! Aggregation keys on [`Route`], a fieldless enum. No runtime request
//! target can reach a checkpoint field, by construction (SEC-0007:R1).

use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::github::route_template::Route;

/// Hard upper bound on checkpoints emitted by one [`BudgetCheckpoints`].
pub(crate) const MAX_CHECKPOINTS_PER_RUN: usize = 8;

const ASCENDING_THRESHOLD_PERMILLE_OF_CEILING: [u64; MAX_CHECKPOINTS_PER_RUN] =
    [250, 500, 750, 1000, 2000, 4000, 8000, 16000];

const ROUTES_NAMED_PER_CHECKPOINT: usize = 3;

/// The most consuming routes of one checkpoint, most consuming first, ranked
/// from independently loaded relaxed per-route counters.
///
/// The ranking is therefore a best-effort mixed-time approximation, not a
/// coherent instant: counters may advance between the loads that produce it.
///
/// Holds [`Route`] values, never strings: its [`fmt::Display`] output is
/// built from `&'static str` templates and integers only.
pub(crate) struct ApproxTopRoutes([Option<(Route, u64)>; ROUTES_NAMED_PER_CHECKPOINT]);

impl fmt::Display for ApproxTopRoutes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for (route, calls) in self.0.iter().flatten() {
            if !first {
                f.write_str(" ")?;
            }
            first = false;
            write!(f, "{}={calls}", route.template())?;
        }
        if first {
            f.write_str("none")?;
        }
        Ok(())
    }
}

/// One budget-consumption milestone.
pub(crate) struct BudgetCheckpoint {
    /// Calls made by this run when the milestone was crossed.
    pub(crate) calls: u64,
    /// The epoch ceiling in force at that moment.
    pub(crate) ceiling: u64,
    /// Wall-clock time since this run's tracker was created.
    pub(crate) elapsed: Duration,
    /// Approximate ranking of this run's most consuming routes.
    pub(crate) approx_top_routes: ApproxTopRoutes,
}

/// Lock-free per-route call aggregation plus a monotone threshold cursor,
/// scoped to exactly one collection run.
///
/// A tracker is never reset in place: the run boundary installs a fresh one,
/// so `baseline_calls` and `started` are immutable for the tracker's whole
/// life and every checkpoint is measured against the run that owns it.
///
/// Every operation on the request hot path is a relaxed atomic add and an
/// acquire load; no lock is taken.
pub(crate) struct BudgetCheckpoints {
    counts: [AtomicU64; Route::COUNT],
    next_threshold: AtomicUsize,
    baseline_calls: u64,
    started: Instant,
}

impl fmt::Debug for BudgetCheckpoints {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BudgetCheckpoints")
            .field("baseline_calls", &self.baseline_calls)
            .field(
                "next_threshold",
                &self.next_threshold.load(Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}

impl BudgetCheckpoints {
    /// Start a fresh aggregation for one collection run.
    ///
    /// `baseline_calls` is the budget gate's process-cumulative call count at
    /// the run boundary; every threshold is measured against calls made
    /// *after* it, so a later run reaches its own milestones exactly as the
    /// first one did.
    pub(crate) fn for_run(baseline_calls: u64) -> Self {
        Self {
            counts: [const { AtomicU64::new(0) }; Route::COUNT],
            next_threshold: AtomicUsize::new(0),
            baseline_calls,
            started: Instant::now(),
        }
    }

    /// Attribute one outbound call to `route` and report a milestone if this
    /// call crossed the next un-emitted threshold.
    ///
    /// `total_calls_made` is the budget gate's process-cumulative count and
    /// `ceiling` the epoch limit. Returns at most one checkpoint per call, and
    /// at most [`MAX_CHECKPOINTS_PER_RUN`] over this run, regardless of how
    /// many times it is invoked.
    pub(crate) fn record(
        &self,
        route: Route,
        total_calls_made: u64,
        ceiling: u64,
    ) -> Option<BudgetCheckpoint> {
        self.counts[route.index()].fetch_add(1, Ordering::Relaxed);
        let calls = total_calls_made.saturating_sub(self.baseline_calls);

        loop {
            let index = self.next_threshold.load(Ordering::Acquire);
            let permille = *ASCENDING_THRESHOLD_PERMILLE_OF_CEILING.get(index)?;
            if calls < threshold_calls(ceiling, permille) {
                return None;
            }
            if self
                .next_threshold
                .compare_exchange(
                    index,
                    index.saturating_add(1),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return Some(BudgetCheckpoint {
                    calls,
                    ceiling,
                    elapsed: self.started.elapsed(),
                    approx_top_routes: self.approx_top_routes(),
                });
            }
        }
    }

    fn approx_top_routes(&self) -> ApproxTopRoutes {
        let mut ranked: Vec<(Route, u64)> = Route::ALL
            .iter()
            .map(|route| (*route, self.counts[route.index()].load(Ordering::Relaxed)))
            .filter(|&(_, calls)| calls > 0)
            .collect();
        ranked.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        let mut top = [None; ROUTES_NAMED_PER_CHECKPOINT];
        for (slot, entry) in top.iter_mut().zip(ranked) {
            *slot = Some(entry);
        }
        ApproxTopRoutes(top)
    }
}

fn threshold_calls(ceiling: u64, permille: u64) -> u64 {
    ceiling.saturating_mul(permille).saturating_div(1000).max(1)
}

#[cfg(test)]
mod tests {
    use super::{BudgetCheckpoints, MAX_CHECKPOINTS_PER_RUN, Route};

    const CEILING: u64 = 1000;

    fn drive_from(baseline: u64, requests: u64, ceiling: u64) -> Vec<String> {
        let checkpoints = BudgetCheckpoints::for_run(baseline);
        (1..=requests)
            .filter_map(|call| {
                checkpoints
                    .record(Route::OrgRepos, baseline.saturating_add(call), ceiling)
                    .map(|c| format!("{}|{}|{}", c.calls, c.ceiling, c.approx_top_routes))
            })
            .collect()
    }

    fn drive(requests: u64, ceiling: u64) -> Vec<String> {
        drive_from(0, requests, ceiling)
    }

    #[test]
    fn a_later_run_reaches_its_own_milestones_from_its_own_baseline() {
        let first = drive_from(0, 4000, CEILING);
        let second = drive_from(4000, 4000, CEILING);
        assert!(!second.is_empty(), "the second run emitted no checkpoints");
        assert_eq!(first, second);
    }

    #[test]
    fn checkpoint_count_stays_bounded_under_four_thousand_requests() {
        let emitted = drive(4000, CEILING);
        assert!(
            emitted.len() <= MAX_CHECKPOINTS_PER_RUN,
            "emitted {} checkpoints, bound is {MAX_CHECKPOINTS_PER_RUN}",
            emitted.len()
        );
        assert_eq!(emitted.len(), 6);
    }

    #[test]
    fn checkpoint_count_does_not_grow_with_request_count() {
        let forty_thousand = drive(40_000, CEILING);
        let four_hundred_thousand = drive(400_000, CEILING);
        assert_eq!(forty_thousand.len(), MAX_CHECKPOINTS_PER_RUN);
        assert_eq!(four_hundred_thousand.len(), MAX_CHECKPOINTS_PER_RUN);
    }

    #[test]
    fn first_checkpoint_lands_at_a_quarter_of_the_ceiling() {
        let emitted = drive(4000, CEILING);
        let first = emitted.first().expect("at least one checkpoint");
        assert!(first.starts_with("250|1000|"), "unexpected first: {first}");
    }

    #[test]
    fn a_run_that_never_reaches_a_quarter_of_the_ceiling_emits_nothing() {
        assert!(drive(249, CEILING).is_empty());
    }

    #[test]
    fn breakdown_names_the_top_consuming_routes_with_counts() {
        let checkpoints = BudgetCheckpoints::for_run(0);
        for _ in 0..10 {
            let _ = checkpoints.record(Route::RepoCommits, 0, CEILING);
        }
        for _ in 0..5 {
            let _ = checkpoints.record(Route::OrgRepos, 0, CEILING);
        }
        for _ in 0..2 {
            let _ = checkpoints.record(Route::User, 0, CEILING);
        }
        let checkpoint = checkpoints
            .record(Route::RepoCommits, CEILING, CEILING)
            .expect("crossing the ceiling emits a checkpoint");
        assert_eq!(
            checkpoint.approx_top_routes.to_string(),
            "/repos/{owner}/{repo}/commits=11 /orgs/{org}/repos=5 /user=2"
        );
    }

    #[test]
    fn breakdown_is_built_only_from_closed_set_templates() {
        let checkpoints = BudgetCheckpoints::for_run(0);
        for route in Route::ALL {
            let _ = checkpoints.record(route, 0, CEILING);
        }
        let rendered = checkpoints
            .record(Route::User, CEILING, CEILING)
            .expect("crossing the ceiling emits a checkpoint")
            .approx_top_routes
            .to_string();
        for fragment in rendered.split(' ') {
            let (template, count) = fragment.split_once('=').expect("template=count pair");
            assert!(
                Route::ALL.iter().any(|r| r.template() == template),
                "breakdown named {template}, outside the closed set"
            );
            assert!(
                count.parse::<u64>().is_ok(),
                "count {count} is not a number"
            );
        }
    }
}
