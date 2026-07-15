//! Host-testable discrete-event simulation core of gh-report's queue
//! network (adr-fmt-223sd). Pure Rust, no `web-sys`/`wasm` leakage —
//! this module compiles and tests on any host target; [`crate::view`]
//! (wasm32-only) drives it frame-by-frame and renders its state.

use std::collections::{HashSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DomainKey(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobSource {
    ScheduledBatch,
    External { id: u64 },
    InitialLoad,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobSpec {
    pub domain_key: DomainKey,
    pub source: JobSource,
    pub enqueued_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueResult {
    Accepted,
    Deduplicated,
    QueueFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobOutcome {
    Success,
    Failure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepPhase {
    Init,
    AwaitingBatch,
    BatchDrained,
    Completed,
}

pub struct WorkQueue {
    capacity: usize,
    jobs: VecDeque<JobSpec>,
    queued_keys: HashSet<DomainKey>,
}

impl WorkQueue {
    /// # Panics
    ///
    /// Panics if `capacity` is `0` (mirrors the real `WorkQueue`'s
    /// bounded-channel construction, wq:105).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "capacity 0 panics");
        Self {
            capacity,
            jobs: VecDeque::with_capacity(capacity),
            queued_keys: HashSet::new(),
        }
    }

    pub fn enqueue(&mut self, job: JobSpec) -> EnqueueResult {
        if self.queued_keys.contains(&job.domain_key) {
            return EnqueueResult::Deduplicated;
        }
        if self.jobs.len() >= self.capacity {
            return EnqueueResult::QueueFull;
        }
        self.queued_keys.insert(job.domain_key);
        self.jobs.push_back(job);
        EnqueueResult::Accepted
    }

    pub fn dequeue(&mut self) -> Option<JobSpec> {
        let job = self.jobs.pop_front()?;
        self.queued_keys.remove(&job.domain_key);
        Some(job)
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        self.jobs.len()
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

pub struct BatchTracker {
    remaining: usize,
}

impl BatchTracker {
    #[must_use]
    pub fn new() -> Self {
        Self { remaining: 0 }
    }

    pub fn increment(&mut self) {
        self.remaining += 1;
    }

    pub fn decrement(&mut self) {
        self.remaining = self.remaining.saturating_sub(1);
    }

    #[must_use]
    pub fn remaining(&self) -> usize {
        self.remaining
    }

    #[must_use]
    pub fn is_drained(&self) -> bool {
        self.remaining == 0
    }
}

impl Default for BatchTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
pub struct EvidenceProjection {
    repositories_captured: usize,
}

impl EvidenceProjection {
    pub fn fold_outcome(&mut self, outcome: JobOutcome) {
        if outcome == JobOutcome::Success {
            self.repositories_captured += 1;
        }
    }

    #[must_use]
    pub fn repositories_captured(&self) -> usize {
        self.repositories_captured
    }
}

#[derive(Default)]
pub struct DeliveryTail {
    rendered_pages: usize,
    published_events: usize,
}

impl DeliveryTail {
    pub fn publish(&mut self) {
        self.rendered_pages += 1;
        self.published_events += 1;
    }

    #[must_use]
    pub fn served_pages(&self) -> usize {
        self.rendered_pages
    }

    #[must_use]
    pub fn published_events(&self) -> usize {
        self.published_events
    }
}

struct WorkerSlot {
    job: Option<JobSpec>,
    remaining_ticks: u32,
}

impl WorkerSlot {
    const fn idle() -> Self {
        Self {
            job: None,
            remaining_ticks: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SimConfig {
    pub queue_capacity: usize,
    pub worker_count: usize,
    pub service_ticks: u32,
    pub domain_key_span: u32,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            queue_capacity: 32,
            worker_count: 16,
            service_ticks: 4,
            domain_key_span: 24,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Metrics {
    pub accepted: u64,
    pub queue_full: u64,
    pub deduplicated: u64,
    pub completed: u64,
    pub failures: u64,
}

#[derive(Debug, Default, Clone)]
pub struct StepEvents {
    pub arrivals: Vec<(JobSource, EnqueueResult)>,
    pub completions: Vec<(JobSource, JobOutcome)>,
}

pub struct Sim {
    config: SimConfig,
    queue: WorkQueue,
    workers: Vec<WorkerSlot>,
    batch_tracker: BatchTracker,
    projection: EvidenceProjection,
    delivery: DeliveryTail,
    metrics: Metrics,
    tick: u64,
    rng_state: u64,
    next_external_id: u64,
}

impl Sim {
    #[must_use]
    pub fn new(config: SimConfig, seed: u64) -> Self {
        let worker_count = config.worker_count;
        Self {
            queue: WorkQueue::new(config.queue_capacity),
            workers: (0..worker_count).map(|_| WorkerSlot::idle()).collect(),
            batch_tracker: BatchTracker::new(),
            projection: EvidenceProjection::default(),
            delivery: DeliveryTail::default(),
            metrics: Metrics::default(),
            tick: 0,
            rng_state: seed | 1,
            next_external_id: 0,
            config,
        }
    }

    fn next_rand(&mut self) -> u64 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng_state = x;
        x
    }

    fn random_domain_key(&mut self) -> DomainKey {
        let span = u64::from(self.config.domain_key_span.max(1));
        let key = self.next_rand() % span;
        DomainKey(u32::try_from(key).unwrap_or(u32::MAX))
    }

    pub fn submit(&mut self, source: JobSource) -> EnqueueResult {
        let domain_key = self.random_domain_key();
        let job = JobSpec {
            domain_key,
            source,
            enqueued_at: self.tick,
        };
        let result = self.queue.enqueue(job);
        match result {
            EnqueueResult::Accepted => {
                self.metrics.accepted += 1;
                if source == JobSource::ScheduledBatch {
                    self.batch_tracker.increment();
                }
            }
            EnqueueResult::QueueFull => self.metrics.queue_full += 1,
            EnqueueResult::Deduplicated => self.metrics.deduplicated += 1,
        }
        result
    }

    pub fn submit_external(&mut self) -> EnqueueResult {
        let id = self.next_external_id;
        self.next_external_id += 1;
        self.submit(JobSource::External { id })
    }

    pub fn step(&mut self, batch_arrival: bool, external_arrival: bool) -> StepEvents {
        let mut events = StepEvents::default();

        if batch_arrival {
            let result = self.submit(JobSource::ScheduledBatch);
            events.arrivals.push((JobSource::ScheduledBatch, result));
        }
        if external_arrival {
            let source = JobSource::External {
                id: self.next_external_id,
            };
            let result = self.submit_external();
            events.arrivals.push((source, result));
        }

        for slot in &mut self.workers {
            if slot.job.is_none()
                && let Some(job) = self.queue.dequeue()
            {
                slot.job = Some(job);
                slot.remaining_ticks = self.config.service_ticks.max(1);
            }
        }

        for slot in &mut self.workers {
            let Some(job) = slot.job else { continue };
            slot.remaining_ticks = slot.remaining_ticks.saturating_sub(1);
            if slot.remaining_ticks == 0 {
                let outcome = if (job.enqueued_at + u64::from(job.domain_key.0)).is_multiple_of(10)
                {
                    JobOutcome::Failure
                } else {
                    JobOutcome::Success
                };
                events.completions.push((job.source, outcome));
                self.projection.fold_outcome(outcome);
                self.delivery.publish();
                self.metrics.completed += 1;
                if outcome == JobOutcome::Failure {
                    self.metrics.failures += 1;
                }
                if job.source == JobSource::ScheduledBatch {
                    self.batch_tracker.decrement();
                }
                slot.job = None;
            }
        }

        self.tick += 1;
        events
    }

    #[must_use]
    pub fn queue_depth(&self) -> usize {
        self.queue.depth()
    }

    #[must_use]
    pub fn queue_capacity(&self) -> usize {
        self.queue.capacity()
    }

    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.workers
            .iter()
            .filter(|slot| slot.job.is_some())
            .count()
    }

    #[must_use]
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    #[must_use]
    pub fn batch_remaining(&self) -> usize {
        self.batch_tracker.remaining()
    }

    #[must_use]
    pub fn served_pages(&self) -> usize {
        self.delivery.served_pages()
    }

    #[must_use]
    pub fn repositories_captured(&self) -> usize {
        self.projection.repositories_captured()
    }

    #[must_use]
    pub fn metrics(&self) -> Metrics {
        self.metrics
    }

    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.queue.depth() == 0 && self.in_flight() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BatchTracker, DomainKey, EnqueueResult, JobSource, JobSpec, Sim, SimConfig, WorkQueue,
    };

    fn job(key: u32, source: JobSource) -> JobSpec {
        JobSpec {
            domain_key: DomainKey(key),
            source,
            enqueued_at: 0,
        }
    }

    #[test]
    fn queue_full_emitted_at_capacity() {
        let mut queue = WorkQueue::new(2);
        assert_eq!(
            queue.enqueue(job(1, JobSource::ScheduledBatch)),
            EnqueueResult::Accepted
        );
        assert_eq!(
            queue.enqueue(job(2, JobSource::ScheduledBatch)),
            EnqueueResult::Accepted
        );
        assert_eq!(
            queue.enqueue(job(3, JobSource::ScheduledBatch)),
            EnqueueResult::QueueFull
        );
    }

    #[test]
    fn deduplicated_on_duplicate_domain_key_while_queued() {
        let mut queue = WorkQueue::new(4);
        assert_eq!(
            queue.enqueue(job(7, JobSource::ScheduledBatch)),
            EnqueueResult::Accepted
        );
        assert_eq!(
            queue.enqueue(job(7, JobSource::External { id: 1 })),
            EnqueueResult::Deduplicated
        );
        let dequeued = queue.dequeue().expect("job present");
        assert_eq!(dequeued.domain_key, DomainKey(7));
        assert_eq!(
            queue.enqueue(job(7, JobSource::External { id: 2 })),
            EnqueueResult::Accepted,
            "key clears on dequeue"
        );
    }

    #[test]
    fn batch_tracker_reaches_zero_after_drain() {
        let mut tracker = BatchTracker::new();
        assert!(tracker.is_drained());
        tracker.increment();
        tracker.increment();
        assert_eq!(tracker.remaining(), 2);
        tracker.decrement();
        assert!(!tracker.is_drained());
        tracker.decrement();
        assert!(tracker.is_drained());
    }

    #[test]
    fn batch_tracker_drains_through_sim() {
        let config = SimConfig {
            queue_capacity: 64,
            worker_count: 16,
            service_ticks: 2,
            domain_key_span: 64,
        };
        let mut sim = Sim::new(config, 42);
        for _ in 0..8 {
            let _ignored = sim.submit(JobSource::ScheduledBatch);
        }
        assert!(sim.batch_remaining() > 0);
        for _ in 0..200 {
            sim.step(false, false);
            if sim.batch_remaining() == 0 {
                break;
            }
        }
        assert_eq!(sim.batch_remaining(), 0);
    }

    #[test]
    fn job_conservation_every_accepted_job_completes() {
        let config = SimConfig {
            queue_capacity: 8,
            worker_count: 16,
            service_ticks: 3,
            domain_key_span: 500,
        };
        let mut sim = Sim::new(config, 1234);
        for tick in 0..300u64 {
            let batch_arrival = tick % 2 == 0;
            let external_arrival = tick % 3 == 0;
            sim.step(batch_arrival, external_arrival);
        }
        for _ in 0..64 {
            sim.step(false, false);
        }
        assert!(
            sim.is_idle(),
            "sim must drain to idle before checking conservation"
        );
        assert_eq!(
            sim.metrics().accepted,
            sim.metrics().completed,
            "every accepted job must eventually be served, none vanish"
        );
    }

    #[test]
    fn sixteen_worker_concurrency_cap_never_exceeded() {
        let config = SimConfig {
            queue_capacity: 4,
            worker_count: 16,
            service_ticks: 5,
            domain_key_span: 1000,
        };
        let mut sim = Sim::new(config, 99);
        for tick in 0..500u64 {
            sim.step(true, tick % 2 == 0);
            assert!(
                sim.in_flight() <= sim.worker_count(),
                "in-flight {} exceeded worker cap {}",
                sim.in_flight(),
                sim.worker_count()
            );
        }
    }
}
