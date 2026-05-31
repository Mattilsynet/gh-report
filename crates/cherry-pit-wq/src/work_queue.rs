//! Bounded, deduplicated FIFO work queue.
//!
//! Domain-agnostic: the reactor never inspects domain keys or context values.
//! Producers (scheduled batch, external triggers, startup loader) create
//! [`JobSpec`] values and submit them via [`WorkQueue::enqueue`]. Workers
//! dequeue via [`WorkQueue::dequeue`].
//!
//! ## Dedup semantics
//!
//! If a job with the same `domain_key` is already **in the queue** (not yet
//! dequeued), new jobs for that key are silently dropped. This is safe because
//! every job queries the source-of-truth for current state — the queued job
//! will see all changes that occurred since it was enqueued.
//!
//! Once a job is **dequeued**, the key is removed from the pending set. A new
//! job for the same key CAN then be enqueued (handles the case where the
//! resource changes again while the previous job is executing).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cherry_pit_core::CorrelationContext;
use scc::HashSet as SccHashSet;
use tokio::sync::mpsc;

/// Type alias for domain-specific identifiers. The reactor never inspects
/// this value — domain code assigns semantic meaning (e.g., numeric repo ID).
pub type DomainKey = String;

/// A unit of work submitted to the reactor.
///
/// The reactor guarantees:
/// 1. At most one `JobSpec` per `domain_key` is queued under typical load
///    (benign duplicate possible under high contention — see dedup note)
/// 2. Jobs are processed in FIFO order
/// 3. The `source` field has zero effect on processing order (observability only)
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct JobSpec<C: Send + Sync + 'static> {
    /// The domain key identifying the target of this job.
    pub domain_key: DomainKey,
    /// Opaque context the worker needs to execute the job.
    pub context: C,
    /// Where this job originated (observability only).
    pub source: JobSource,
    /// When this job was enqueued (set by the queue, not the producer).
    pub enqueued_at: Option<tokio::time::Instant>,
    /// Correlation chain carried end-to-end through worker-pool emission
    /// into [`crate::JobOutcome`]. Set by the producer (CHE-0055 G5,
    /// reinstating CHE-0052:R4 in v0.1).
    pub correlation: CorrelationContext,
}

impl<C: Send + Sync + 'static> JobSpec<C> {
    /// Create a new job specification.
    ///
    /// `enqueued_at` is set to `None`; the queue stamps it on enqueue.
    /// `correlation` propagates end-to-end into the emitted
    /// [`crate::JobOutcome`] per CHE-0055 G5 (no `Default`, no synthesis
    /// at the worker boundary; pass [`CorrelationContext::none`] for
    /// uncorrelated user-initiated work).
    #[must_use]
    pub fn new(
        domain_key: DomainKey,
        context: C,
        source: JobSource,
        correlation: CorrelationContext,
    ) -> Self {
        Self {
            domain_key,
            context,
            source,
            enqueued_at: None,
            correlation,
        }
    }
}

/// Origin of a job (observability only — zero effect on queue ordering).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum JobSource {
    /// Part of a scheduled batch scrape.
    ScheduledBatch,
    /// Triggered by an external event (e.g., webhook, notification).
    External { id: String, kind: String },
    /// Initial load at startup.
    InitialLoad,
}

/// Result of an enqueue attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EnqueueResult {
    /// Job accepted and added to the queue.
    Accepted,
    /// Job rejected: a job with the same domain key is already queued.
    Deduplicated,
    /// Job rejected: the queue is at capacity (or closed).
    QueueFull,
}

/// Bounded, deduplicated FIFO work queue.
///
/// Thread-safe for concurrent producers and a single consumer (or multiple
/// consumers serialized via the internal `tokio::sync::Mutex` on the receiver).
///
/// Call [`close`](Self::close) to signal shutdown: workers' `dequeue()` will
/// return `None` after remaining items are drained.
#[non_exhaustive]
pub struct WorkQueue<C: Send + Sync + 'static> {
    sender: std::sync::Mutex<Option<mpsc::Sender<JobSpec<C>>>>,
    receiver: tokio::sync::Mutex<mpsc::Receiver<JobSpec<C>>>,
    /// Domain keys currently in the queue (between enqueue and dequeue).
    pending: SccHashSet<DomainKey>,
}

impl<C: Send + Sync + 'static> WorkQueue<C> {
    /// Create a new work queue with the given capacity.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is 0.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "queue capacity must be > 0");
        let (sender, receiver) = mpsc::channel(capacity);
        Self {
            sender: std::sync::Mutex::new(Some(sender)),
            receiver: tokio::sync::Mutex::new(receiver),
            pending: SccHashSet::new(),
        }
    }

    /// Attempt to enqueue a job.
    ///
    /// **Concurrent insert note:** Two concurrent `enqueue()` calls for the
    /// same domain key can both pass the `insert()` check before either
    /// reaches `try_send()`. This is a benign race — the second job
    /// performs a redundant re-evaluation producing the same result.
    pub fn enqueue(&self, mut job: JobSpec<C>) -> EnqueueResult {
        // Dedup: reject if this key is already queued.
        if self.pending.insert_sync(job.domain_key.clone()).is_err() {
            tracing::info!(key = %job.domain_key, source = ?job.source, "job deduplicated");
            return EnqueueResult::Deduplicated;
        }
        // Stamp enqueue time.
        job.enqueued_at = Some(tokio::time::Instant::now());

        let guard = self
            .sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(sender) = guard.as_ref() else {
            let _ = self.pending.remove_sync(&job.domain_key);
            tracing::warn!(key = %job.domain_key, "enqueue rejected: queue closed");
            return EnqueueResult::QueueFull;
        };

        match sender.try_send(job) {
            Ok(()) => EnqueueResult::Accepted,
            Err(mpsc::error::TrySendError::Full(job)) => {
                let _ = self.pending.remove_sync(&job.domain_key);
                tracing::warn!(key = %job.domain_key, "enqueue rejected: queue full");
                EnqueueResult::QueueFull
            }
            Err(mpsc::error::TrySendError::Closed(job)) => {
                let _ = self.pending.remove_sync(&job.domain_key);
                tracing::warn!(key = %job.domain_key, "enqueue rejected: channel closed");
                EnqueueResult::QueueFull
            }
        }
    }

    /// Dequeue the next job. Blocks until one is available.
    ///
    /// Returns `None` when the channel is closed (via [`close`](Self::close)
    /// or dropping the `WorkQueue`) and all remaining items are drained.
    pub async fn dequeue(&self) -> Option<JobSpec<C>> {
        let mut rx = self.receiver.lock().await;
        let job = rx.recv().await?;
        let _ = self.pending.remove_sync(&job.domain_key);
        Some(job)
    }

    /// Close the queue. After this, `dequeue()` returns `None` once
    /// remaining items are drained. New `enqueue()` calls return `QueueFull`.
    pub fn close(&self) {
        let _ = self
            .sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }

    /// Number of jobs currently in the queue (approximate).
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Whether the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// Enqueue jobs for all domain keys from a batch source.
///
/// Returns the number of jobs accepted (vs. deduplicated or rejected).
///
/// All jobs in the batch share the same `correlation` chain — batch is the
/// correlation unit per CHE-0055 G5 (cf. CHE-0052:R4 batch-as-correlation-unit).
pub fn enqueue_batch<C: Send + Sync + Clone + 'static>(
    queue: &WorkQueue<C>,
    items: Vec<(DomainKey, C)>,
    source: &JobSource,
    correlation: &CorrelationContext,
) -> BatchEnqueueResult {
    let total = items.len();
    let mut accepted = 0;
    let mut deduplicated = 0;
    let mut rejected = 0;

    for (key, context) in items {
        let job = JobSpec {
            domain_key: key,
            context,
            source: source.clone(),
            enqueued_at: None,
            correlation: correlation.clone(),
        };
        match queue.enqueue(job) {
            EnqueueResult::Accepted => accepted += 1,
            EnqueueResult::Deduplicated => deduplicated += 1,
            EnqueueResult::QueueFull => rejected += 1,
        }
    }

    tracing::info!(
        total,
        accepted,
        deduplicated,
        rejected,
        "batch enqueue complete"
    );

    BatchEnqueueResult {
        total,
        accepted,
        deduplicated,
        rejected,
    }
}

/// Result of a batch enqueue operation.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BatchEnqueueResult {
    pub total: usize,
    pub accepted: usize,
    pub deduplicated: usize,
    pub rejected: usize,
}

/// Token for tracking batch completion.
///
/// Created by the batch producer, counted down by the delivery handler.
#[non_exhaustive]
pub struct BatchTracker {
    remaining: AtomicUsize,
    done: tokio::sync::Notify,
}

impl BatchTracker {
    #[must_use]
    pub fn new(count: usize) -> Arc<Self> {
        Arc::new(Self {
            remaining: AtomicUsize::new(count),
            done: tokio::sync::Notify::new(),
        })
    }

    /// Mark one job as completed. If this was the last one, notify waiters.
    ///
    /// # Panics
    ///
    /// Panics if called more times than the initial count (underflow guard).
    pub fn complete_one(&self) {
        let prev = self.remaining.fetch_sub(1, Ordering::AcqRel);
        assert!(
            prev > 0,
            "BatchTracker::complete_one called more times than initial count"
        );
        if prev == 1 {
            self.done.notify_waiters();
        }
    }

    /// Wait until all tracked jobs are complete.
    pub async fn wait(&self) {
        // Register the Notified future BEFORE checking the count to avoid
        // the race where remaining transitions 1→0 between load and await.
        let notified = self.done.notified();
        if self.remaining.load(Ordering::Acquire) == 0 {
            return;
        }
        notified.await;
    }

    /// Number of remaining jobs.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.remaining.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    async fn fifo_ordering() {
        let q = WorkQueue::new(10);
        assert_eq!(q.enqueue(make_job("a")), EnqueueResult::Accepted);
        assert_eq!(q.enqueue(make_job("b")), EnqueueResult::Accepted);
        assert_eq!(q.enqueue(make_job("c")), EnqueueResult::Accepted);

        let j1 = q.dequeue().await.unwrap();
        assert_eq!(j1.domain_key, "a");
        let j2 = q.dequeue().await.unwrap();
        assert_eq!(j2.domain_key, "b");
        let j3 = q.dequeue().await.unwrap();
        assert_eq!(j3.domain_key, "c");
    }

    #[tokio::test]
    async fn dedup_rejects_duplicate() {
        let q = WorkQueue::new(10);
        assert_eq!(q.enqueue(make_job("a")), EnqueueResult::Accepted);
        assert_eq!(q.enqueue(make_job("a")), EnqueueResult::Deduplicated);
        assert_eq!(q.len(), 1);
    }

    #[tokio::test]
    async fn dedup_cleared_on_dequeue() {
        let q = WorkQueue::new(10);
        assert_eq!(q.enqueue(make_job("a")), EnqueueResult::Accepted);
        let _ = q.dequeue().await.unwrap();
        // Key cleared — re-enqueue should succeed.
        assert_eq!(q.enqueue(make_job("a")), EnqueueResult::Accepted);
    }

    #[tokio::test]
    async fn capacity_enforced() {
        let q = WorkQueue::new(2);
        assert_eq!(q.enqueue(make_job("a")), EnqueueResult::Accepted);
        assert_eq!(q.enqueue(make_job("b")), EnqueueResult::Accepted);
        assert_eq!(q.enqueue(make_job("c")), EnqueueResult::QueueFull);
    }

    #[tokio::test]
    async fn channel_close_returns_none() {
        let q = WorkQueue::new(10);
        q.enqueue(make_job("a"));
        let j = q.dequeue().await.expect("queued item");
        assert_eq!(j.domain_key, "a");
        q.close();
        assert!(q.dequeue().await.is_none());
    }

    #[tokio::test]
    async fn concurrent_enqueue_distinct_keys() {
        let q = Arc::new(WorkQueue::new(100));
        let mut handles = Vec::new();

        for i in 0..16 {
            let q = Arc::clone(&q);
            handles.push(tokio::spawn(async move {
                q.enqueue(make_job(&format!("key-{i}")))
            }));
        }

        let results: Vec<_> = futures_util::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        let accepted = results
            .iter()
            .filter(|r| **r == EnqueueResult::Accepted)
            .count();
        assert_eq!(accepted, 16);
    }

    #[tokio::test]
    async fn concurrent_enqueue_same_key() {
        let q = Arc::new(WorkQueue::new(100));
        let mut handles = Vec::new();

        for _ in 0..16 {
            let q = Arc::clone(&q);
            handles.push(tokio::spawn(async move { q.enqueue(make_job("same-key")) }));
        }

        let results: Vec<_> = futures_util::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        let accepted = results
            .iter()
            .filter(|r| **r == EnqueueResult::Accepted)
            .count();
        // Benign race: 1 or 2 accepted is fine.
        assert!(
            (1..=2).contains(&accepted),
            "expected 1-2 accepted, got {accepted}"
        );
    }

    #[tokio::test]
    async fn len_accuracy() {
        let q = WorkQueue::new(10);
        for i in 0..5 {
            q.enqueue(make_job(&format!("k{i}")));
        }
        assert_eq!(q.len(), 5);

        let _ = q.dequeue().await;
        let _ = q.dequeue().await;
        assert_eq!(q.len(), 3);
    }

    #[tokio::test]
    async fn enqueue_stamps_time() {
        let q = WorkQueue::new(10);
        q.enqueue(make_job("a"));
        let j = q.dequeue().await.unwrap();
        assert!(j.enqueued_at.is_some());
    }

    // ── BatchTracker tests ─────────────────────────────────────────

    #[tokio::test]
    async fn batch_tracker_immediate_when_zero() {
        let tracker = BatchTracker::new(0);
        // Should return immediately.
        tracker.wait().await;
        assert_eq!(tracker.remaining(), 0);
    }

    #[tokio::test]
    async fn batch_tracker_counts_down() {
        let tracker = BatchTracker::new(3);
        assert_eq!(tracker.remaining(), 3);
        tracker.complete_one();
        assert_eq!(tracker.remaining(), 2);
        tracker.complete_one();
        assert_eq!(tracker.remaining(), 1);

        let t = Arc::clone(&tracker);
        let handle = tokio::spawn(async move { t.wait().await });

        tracker.complete_one();
        // Should unblock.
        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("should complete within timeout")
            .unwrap();
    }

    // ── enqueue_batch tests ────────────────────────────────────────

    #[tokio::test]
    async fn batch_enqueue_counts() {
        let q = WorkQueue::new(10);
        let items: Vec<(DomainKey, String)> = (0..5)
            .map(|i| (format!("k{i}"), format!("ctx{i}")))
            .collect();
        let result = enqueue_batch(
            &q,
            items,
            &JobSource::ScheduledBatch,
            &CorrelationContext::none(),
        );
        assert_eq!(result.total, 5);
        assert_eq!(result.accepted, 5);
        assert_eq!(result.deduplicated, 0);
        assert_eq!(result.rejected, 0);
    }

    #[test]
    #[should_panic(expected = "called more times than initial count")]
    fn batch_tracker_underflow_panics() {
        let tracker = BatchTracker::new(1);
        tracker.complete_one(); // ok
        tracker.complete_one(); // should panic
    }

    #[tokio::test]
    async fn enqueue_after_close_returns_queue_full() {
        let q = WorkQueue::new(10);
        q.close();
        assert_eq!(q.enqueue(make_job("a")), EnqueueResult::QueueFull);
    }

    #[tokio::test]
    async fn correlation_round_trips_through_queue() {
        use cherry_pit_core::CorrelationContext;
        let corr_id = uuid::Uuid::now_v7();
        let cause_id = uuid::Uuid::now_v7();
        let ctx = CorrelationContext::new(corr_id, cause_id);

        let q = WorkQueue::new(10);
        let job = JobSpec::new(
            "key-corr".to_string(),
            "ctx-corr".to_string(),
            JobSource::ScheduledBatch,
            ctx.clone(),
        );
        assert_eq!(q.enqueue(job), EnqueueResult::Accepted);
        let dequeued = q.dequeue().await.unwrap();
        assert_eq!(dequeued.correlation, ctx);
        assert_eq!(dequeued.correlation.correlation_id(), Some(corr_id));
        assert_eq!(dequeued.correlation.causation_id(), Some(cause_id));
    }
}
