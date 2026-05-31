//! Worker pool for processing jobs from a [`WorkQueue`].
//!
//! Domain code implements [`JobExecutor`] to define what "executing a job"
//! means. The worker pool handles concurrency, budget gating, and rate-limit
//! pausing — domain code never touches those concerns.
//!
//! ## Outcome delivery
//!
//! Workers send [`JobOutcome`] values to an `mpsc::Sender` channel. A dedicated
//! delivery task consumes outcomes asynchronously (EDA pattern). This decouples
//! workers from evidence stores, rendering, and broadcast concerns.
//!
//! ## Budget and rate-limit ordering
//!
//! Each worker acquires the budget gate **before** checking the rate limit.
//! This prevents a worker from consuming budget when it would immediately
//! stall on a rate limit, and ensures budget is spent only on actionable work.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use cherry_pit_core::CorrelationContext;

use crate::budget::BudgetGate;
use crate::rate_limit::RateLimitState;
use crate::work_queue::{DomainKey, JobSource, WorkQueue};

/// Result of processing a single job.
#[derive(Debug)]
#[non_exhaustive]
pub enum JobOutcome<R> {
    /// Job completed successfully.
    Success {
        domain_key: DomainKey,
        result: R,
        source: JobSource,
        duration: Duration,
        /// Correlation chain propagated from the originating
        /// [`crate::JobSpec`] (CHE-0055 G5 / CHE-0052:R6 reinstated v0.1).
        correlation: CorrelationContext,
    },
    /// Job failed.
    Failure {
        domain_key: DomainKey,
        error: String,
        source: JobSource,
        duration: Duration,
        /// Correlation chain propagated from the originating
        /// [`crate::JobSpec`] — equal to the spec's correlation so that
        /// dead-letter / failure consumers observe the same chain
        /// (CHE-0055 G5 / CHE-0052:R6 reinstated v0.1).
        correlation: CorrelationContext,
    },
}

/// Defines how to execute a job for a domain key.
///
/// # Correctness contract
///
/// - `execute` MUST produce a complete result (no partial results).
/// - `execute` MUST be idempotent.
/// - `execute` MUST be safe for concurrent invocation from multiple workers.
///
/// The executor does NOT call `budget_gate.acquire()` — the worker loop
/// handles budget acquisition before calling `execute()`.
pub trait JobExecutor: Send + Sync + 'static {
    /// The opaque context type carried by `JobSpec`.
    type Context: Send + Sync + Clone + 'static;
    /// The result type produced on success.
    type Result: Send + 'static;

    /// Execute the job. Query the source-of-truth for current state of `domain_key`.
    ///
    /// Returns an anonymous opaque future. The compiler places the state
    /// machine on the stack (or in the parent async frame) per CHE-0025:R2 —
    /// no per-call heap allocation.
    fn execute<'a>(
        &'a self,
        domain_key: &'a DomainKey,
        context: &'a Self::Context,
    ) -> impl Future<Output = Result<Self::Result, String>> + Send + 'a;
}

/// Configuration for the worker pool.
#[non_exhaustive]
pub struct WorkerPoolConfig {
    /// Number of concurrent workers.
    pub worker_count: usize,
}

impl Default for WorkerPoolConfig {
    fn default() -> Self {
        Self { worker_count: 16 }
    }
}

/// Run the worker pool. Returns when the queue is closed and all workers
/// have finished processing their current jobs.
///
/// Outcomes are sent to `outcome_tx`. When all workers exit, the sender is
/// dropped, causing the receiver to return `None` — signalling the delivery
/// task to drain and exit.
/// # Panics
///
/// Panics if `config.worker_count` is 0.
pub async fn run_worker_pool<C, R, E>(
    queue: Arc<WorkQueue<C>>,
    executor: Arc<E>,
    budget_gate: Arc<BudgetGate>,
    rate_limit_state: Arc<RateLimitState>,
    config: WorkerPoolConfig,
    outcome_tx: mpsc::Sender<JobOutcome<R>>,
) where
    C: Send + Sync + Clone + 'static,
    R: Send + 'static,
    E: JobExecutor<Context = C, Result = R>,
{
    assert!(config.worker_count > 0, "worker_count must be > 0");
    let mut handles: Vec<JoinHandle<()>> = Vec::with_capacity(config.worker_count);

    for worker_id in 0..config.worker_count {
        let queue = Arc::clone(&queue);
        let executor = Arc::clone(&executor);
        let budget = Arc::clone(&budget_gate);
        let rate_limit = Arc::clone(&rate_limit_state);
        let tx = outcome_tx.clone();

        handles.push(tokio::spawn(async move {
            worker_loop(worker_id, queue, executor, budget, rate_limit, tx).await;
        }));
    }

    // Drop our copy so the channel closes when all workers exit.
    drop(outcome_tx);

    for handle in handles {
        if let Err(e) = handle.await {
            tracing::error!(error = %e, "worker task panicked");
        }
    }
}

async fn worker_loop<C, R, E>(
    worker_id: usize,
    queue: Arc<WorkQueue<C>>,
    executor: Arc<E>,
    budget_gate: Arc<BudgetGate>,
    rate_limit_state: Arc<RateLimitState>,
    outcome_tx: mpsc::Sender<JobOutcome<R>>,
) where
    C: Send + Sync + Clone + 'static,
    R: Send + 'static,
    E: JobExecutor<Context = C, Result = R>,
{
    loop {
        let Some(job) = queue.dequeue().await else {
            tracing::debug!(worker = worker_id, "queue closed, worker exiting");
            break;
        };

        let domain_key = job.domain_key.clone();
        let source = job.source.clone();
        let correlation = job.correlation.clone();
        let start = std::time::Instant::now();

        // Budget gate — blocks if epoch budget exhausted.
        budget_gate.acquire().await;

        // Rate limit gate — wait if API rate limit exhausted.
        if rate_limit_state.should_halt() {
            tracing::warn!(
                worker = worker_id,
                key = %domain_key,
                "rate limit halt — waiting for reset"
            );
            wait_for_rate_limit_reset(&rate_limit_state).await;
        }

        // Execute the job. catch_unwind guards synchronous future construction
        // (executor panics during `execute()` call). Panics inside the async
        // body are caught by the tokio::spawn task boundary.
        let exec_key = domain_key.clone();
        let future_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            executor.execute(&exec_key, &job.context)
        }));

        let outcome = match future_result {
            Ok(future) => match future.await {
                Ok(result) => JobOutcome::Success {
                    domain_key,
                    result,
                    source,
                    duration: start.elapsed(),
                    correlation,
                },
                Err(error) => JobOutcome::Failure {
                    domain_key,
                    error,
                    source,
                    duration: start.elapsed(),
                    correlation,
                },
            },
            Err(panic) => JobOutcome::Failure {
                domain_key,
                error: format!("executor panicked: {panic:?}"),
                source,
                duration: start.elapsed(),
                correlation,
            },
        };

        if outcome_tx.send(outcome).await.is_err() {
            tracing::debug!(worker = worker_id, "outcome channel closed, worker exiting");
            break;
        }
    }
}

/// Wait for the rate limit to reset with exponential backoff.
async fn wait_for_rate_limit_reset(state: &RateLimitState) {
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_mins(1);

    while state.should_halt() {
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

/// Shut down the worker pool gracefully.
///
/// Waits up to `timeout` for workers to complete, then aborts remaining.
/// Intended for use when the caller manages `JoinHandle`s directly.
pub async fn shutdown_worker_pool(handles: Vec<JoinHandle<()>>, timeout: Duration) {
    // Collect AbortHandles before consuming JoinHandles via join_all.
    let abort_handles: Vec<_> = handles.iter().map(JoinHandle::abort_handle).collect();

    if tokio::time::timeout(timeout, futures_util::future::join_all(handles))
        .await
        .is_ok()
    {
        tracing::info!("worker pool drained gracefully");
    } else {
        tracing::warn!(
            timeout_secs = timeout.as_secs(),
            "worker pool shutdown timed out, aborting remaining workers"
        );
        for ah in &abort_handles {
            ah.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_queue::{JobSource, JobSpec, WorkQueue};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Mock executor that returns the domain key as the result.
    struct EchoExecutor;

    impl JobExecutor for EchoExecutor {
        type Context = String;
        type Result = String;

        async fn execute<'a>(
            &'a self,
            domain_key: &'a DomainKey,
            _context: &'a Self::Context,
        ) -> Result<Self::Result, String> {
            Ok(domain_key.clone())
        }
    }

    /// Mock executor that always fails.
    struct FailExecutor;

    impl JobExecutor for FailExecutor {
        type Context = String;
        type Result = String;

        async fn execute<'a>(
            &'a self,
            _domain_key: &'a DomainKey,
            _context: &'a Self::Context,
        ) -> Result<Self::Result, String> {
            Err("simulated failure".to_string())
        }
    }

    fn make_job(key: &str) -> JobSpec<String> {
        JobSpec {
            domain_key: key.to_string(),
            context: format!("ctx-{key}"),
            source: JobSource::ScheduledBatch,
            enqueued_at: None,
            correlation: CorrelationContext::none(),
        }
    }

    #[tokio::test]
    async fn single_worker_processes_job() {
        let queue = Arc::new(WorkQueue::new(10));
        queue.enqueue(make_job("key-1"));
        queue.close();

        let (tx, mut rx) = mpsc::channel(16);

        run_worker_pool(
            Arc::clone(&queue),
            Arc::new(EchoExecutor),
            Arc::new(BudgetGate::new(1000, Duration::from_secs(1))),
            Arc::new(RateLimitState::default()),
            WorkerPoolConfig { worker_count: 1 },
            tx,
        )
        .await;

        let mut outcomes = Vec::new();
        while let Some(o) = rx.recv().await {
            outcomes.push(o);
        }
        assert_eq!(outcomes.len(), 1);
        match &outcomes[0] {
            JobOutcome::Success {
                domain_key, result, ..
            } => {
                assert_eq!(domain_key, "key-1");
                assert_eq!(result, "key-1");
            }
            JobOutcome::Failure { .. } => panic!("expected success"),
        }
    }

    #[tokio::test]
    async fn multiple_workers_process_all_jobs() {
        let queue = Arc::new(WorkQueue::new(100));
        for i in 0..10 {
            queue.enqueue(make_job(&format!("k{i}")));
        }
        queue.close();

        let (tx, mut rx) = mpsc::channel(64);
        let count = Arc::new(AtomicUsize::new(0));

        run_worker_pool(
            Arc::clone(&queue),
            Arc::new(EchoExecutor),
            Arc::new(BudgetGate::new(1000, Duration::from_secs(1))),
            Arc::new(RateLimitState::default()),
            WorkerPoolConfig { worker_count: 4 },
            tx,
        )
        .await;

        while rx.recv().await.is_some() {
            count.fetch_add(1, Ordering::Relaxed);
        }
        assert_eq!(count.load(Ordering::Relaxed), 10);
    }

    #[tokio::test]
    async fn executor_error_produces_failure_outcome() {
        let queue = Arc::new(WorkQueue::new(10));
        queue.enqueue(make_job("fail-key"));
        queue.close();

        let (tx, mut rx) = mpsc::channel(16);

        run_worker_pool(
            Arc::clone(&queue),
            Arc::new(FailExecutor),
            Arc::new(BudgetGate::new(1000, Duration::from_secs(1))),
            Arc::new(RateLimitState::default()),
            WorkerPoolConfig { worker_count: 1 },
            tx,
        )
        .await;

        let mut outcomes = Vec::new();
        while let Some(o) = rx.recv().await {
            outcomes.push(o);
        }
        assert_eq!(outcomes.len(), 1);
        match &outcomes[0] {
            JobOutcome::Failure {
                domain_key, error, ..
            } => {
                assert_eq!(domain_key, "fail-key");
                assert!(error.contains("simulated"));
            }
            JobOutcome::Success { .. } => panic!("expected failure"),
        }
    }

    #[tokio::test]
    async fn worker_exits_on_channel_close() {
        let queue = Arc::new(WorkQueue::new(10));
        let (tx, _rx) = mpsc::channel::<JobOutcome<String>>(16);

        let q = Arc::clone(&queue);
        let handle = tokio::spawn(async move {
            run_worker_pool(
                q,
                Arc::new(EchoExecutor),
                Arc::new(BudgetGate::new(1000, Duration::from_secs(1))),
                Arc::new(RateLimitState::default()),
                WorkerPoolConfig { worker_count: 2 },
                tx,
            )
            .await;
        });

        // Close immediately — workers should exit cleanly.
        queue.close();

        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("workers should exit within timeout")
            .unwrap();
    }

    #[tokio::test]
    async fn outcome_channel_closed_workers_exit() {
        let queue = Arc::new(WorkQueue::new(10));
        // Enqueue a job but drop the receiver immediately.
        queue.enqueue(make_job("key-1"));
        let (tx, rx) = mpsc::channel::<JobOutcome<String>>(1);
        drop(rx); // close receiver

        let q = Arc::clone(&queue);
        let handle = tokio::spawn(async move {
            run_worker_pool(
                q,
                Arc::new(EchoExecutor),
                Arc::new(BudgetGate::new(1000, Duration::from_secs(1))),
                Arc::new(RateLimitState::default()),
                WorkerPoolConfig { worker_count: 1 },
                tx,
            )
            .await;
        });

        // Worker should detect channel closed and exit.
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("workers should exit when outcome channel closed")
            .unwrap();
    }

    #[tokio::test]
    async fn correlation_propagates_spec_to_success_outcome() {
        use cherry_pit_core::CorrelationContext;
        let corr_id = uuid::Uuid::now_v7();
        let ctx = CorrelationContext::correlated(corr_id);

        let queue = Arc::new(WorkQueue::new(10));
        queue.enqueue(JobSpec::new(
            "k-ok".to_string(),
            "ctx-ok".to_string(),
            JobSource::ScheduledBatch,
            ctx.clone(),
        ));
        queue.close();

        let (tx, mut rx) = mpsc::channel(16);
        run_worker_pool(
            Arc::clone(&queue),
            Arc::new(EchoExecutor),
            Arc::new(BudgetGate::new(1000, Duration::from_secs(1))),
            Arc::new(RateLimitState::default()),
            WorkerPoolConfig { worker_count: 1 },
            tx,
        )
        .await;

        let outcome = rx.recv().await.expect("expected one outcome");
        match outcome {
            JobOutcome::Success { correlation, .. } => {
                assert_eq!(correlation, ctx);
                assert_eq!(correlation.correlation_id(), Some(corr_id));
            }
            JobOutcome::Failure { .. } => panic!("expected success"),
        }
    }

    #[tokio::test]
    async fn correlation_propagates_spec_to_failure_outcome() {
        use cherry_pit_core::CorrelationContext;
        let corr_id = uuid::Uuid::now_v7();
        let ctx = CorrelationContext::correlated(corr_id);

        let queue = Arc::new(WorkQueue::new(10));
        queue.enqueue(JobSpec::new(
            "k-fail".to_string(),
            "ctx-fail".to_string(),
            JobSource::ScheduledBatch,
            ctx.clone(),
        ));
        queue.close();

        let (tx, mut rx) = mpsc::channel(16);
        run_worker_pool(
            Arc::clone(&queue),
            Arc::new(FailExecutor),
            Arc::new(BudgetGate::new(1000, Duration::from_secs(1))),
            Arc::new(RateLimitState::default()),
            WorkerPoolConfig { worker_count: 1 },
            tx,
        )
        .await;

        let outcome = rx.recv().await.expect("expected one outcome");
        match outcome {
            JobOutcome::Failure { correlation, .. } => {
                assert_eq!(correlation, ctx);
                assert_eq!(correlation.correlation_id(), Some(corr_id));
            }
            JobOutcome::Success { .. } => panic!("expected failure"),
        }
    }
}
