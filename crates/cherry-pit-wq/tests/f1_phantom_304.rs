//! Acceptance test for ghr-fe9bb970 / CHE-0055:R17 — the F1 phantom-304
//! property on the LIVE `run_worker_pool_regulated` path.
//!
//! Historically `worker_loop_regulated` hardcoded
//! `regulator.settle(SettleOutcome::Charged)` for every admitted job,
//! regardless of whether the executor's result actually consumed the
//! guarded resource. This drove the F1 34x overcount / spurious 1h freeze
//! bug: a job whose real-world effect was free (e.g. GitHub 304
//! not-modified) still charged the budget permit.
//!
//! This test drives the REAL `run_worker_pool_regulated` (no mock of the
//! settle wiring) with an executor whose `charge_of` reports every
//! outcome as [`SettleOutcome::Free`], and asserts the budget conserves:
//! with an epoch limit of 1 and a long cooldown, a regression back to the
//! hardcoded `Charged` settle would exhaust the budget after the first
//! job and stall the remaining 9 admissions for the full cooldown,
//! blowing the bounded timeout below.

use std::sync::Arc;
use std::time::Duration;

use cherry_pit_core::{CorrelationContext, DomainKey, JobOutcome, JobSource};
use cherry_pit_wq::{
    BudgetGate, BudgetRegulator, JobExecutor, JobSpec, Regulator, SettleOutcome, WorkQueue,
    WorkerPoolConfig, run_worker_pool_regulated,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Executor whose every outcome settles as [`SettleOutcome::Free`] —
/// the phantom-304 shape: the job runs and succeeds, but the executor
/// reports that no real budget was spent.
struct AlwaysFreeExecutor;

impl JobExecutor for AlwaysFreeExecutor {
    type Context = String;
    type Result = String;

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "mock executor returns immediately with no I/O to await; the `async` keyword is dictated by the JobExecutor trait signature"
    )]
    async fn execute<'a>(
        &'a self,
        domain_key: &'a DomainKey,
        _context: &'a Self::Context,
    ) -> Result<Self::Result, String> {
        Ok(domain_key.clone())
    }

    fn charge_of(&self, _domain_key: &DomainKey, _result: &Self::Result) -> SettleOutcome {
        SettleOutcome::Free
    }
}

fn make_job(key: &str) -> JobSpec<String> {
    JobSpec::new(
        key.to_string(),
        format!("ctx-{key}"),
        JobSource::ScheduledBatch,
        CorrelationContext::none(),
    )
}

#[tokio::test]
async fn phantom_304_free_outcomes_do_not_exhaust_the_live_regulated_path() {
    const JOB_COUNT: usize = 10;

    let queue = Arc::new(WorkQueue::new(JOB_COUNT));
    for i in 0..JOB_COUNT {
        queue.enqueue(make_job(&format!("k{i}")));
    }
    queue.close();

    let regression_cooldown_that_would_stall_a_charged_regression = Duration::from_secs(30);
    let bounded_test_timeout_shorter_than_the_cooldown = Duration::from_secs(2);
    let gate = Arc::new(BudgetGate::new(
        1,
        regression_cooldown_that_would_stall_a_charged_regression,
    ));
    let regulators: Arc<[Arc<dyn Regulator>]> =
        Arc::from(vec![
            Arc::new(BudgetRegulator::new(Arc::clone(&gate))) as Arc<dyn Regulator>
        ]);

    let (tx, mut rx) = mpsc::channel(JOB_COUNT + 4);

    let mut config = WorkerPoolConfig::default();
    config.worker_count = 1;
    let run = run_worker_pool_regulated(
        Arc::clone(&queue),
        Arc::new(AlwaysFreeExecutor),
        regulators,
        config,
        CancellationToken::new(),
        tx,
    );

    tokio::time::timeout(bounded_test_timeout_shorter_than_the_cooldown, run)
        .await
        .expect(
            "run_worker_pool_regulated stalled — budget was exhausted on free \
             outcomes; the F1 phantom-304 property regressed",
        );

    let mut successes = 0usize;
    while let Some(outcome) = rx.recv().await {
        match outcome {
            JobOutcome::Success { .. } => successes += 1,
            other => panic!("expected success, got {other:?}"),
        }
    }
    assert_eq!(successes, JOB_COUNT, "every job should have succeeded");
    assert_eq!(
        gate.calls_made(),
        0,
        "all-Free settlement must conserve the budget: 0 calls charged"
    );
}
