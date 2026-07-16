//! Host-testable discrete-event simulation core of gh-report's queue
//! network (adr-fmt-223sd, adr-fmt-t63uo). Pure Rust, no `web-sys`/`wasm`
//! leakage — this module compiles and tests on any host target;
//! [`crate::view`] (wasm32-only) drives it frame-by-frame and renders
//! its state.
//!
//! Models THREE distinct triggers converging on a shared write side,
//! joined by a barrier, feeding a per-RUN read side, plus a continuous
//! serve path:
//!
//! - scheduled sweep (`spawn_collection_loop`, daemon.rs:336): batches
//!   many [`JobSource::ScheduledBatch`] jobs, tracked by [`SweepPhase`].
//! - webhook (`webhook_handler`, webhook/mod.rs:61): a single
//!   [`JobSource::External`] job per event, ungated by any barrier.
//! - warm start (`warm_start_from_baseline`, collect.rs:1428): bypasses
//!   the queue/workers entirely, rendering straight from
//!   [`EvidenceProjection`].
//!
//! Write side: [`WorkQueue`] → worker pool (`worker_loop`,
//! `worker_pool.rs:125`) → `LiveEvaluator::evaluate` → [`JobOutcome`] →
//! `delivery_loop` (daemon.rs:478) → `record_repo` → folds an
//! [`EvidenceProjectionEvent`] into [`EvidenceProjection`].
//!
//! Barrier: [`BatchTracker`] (wq:263) gates scheduled runs only.
//!
//! Read side (per RUN, not per packet): `finalize_and_publish`
//! (collect.rs:1165) → `build_cached_pages` → [`CachedBody`] compress →
//! `commit_cached_pages` (`ArcSwap` swap, generation++) →
//! [`PageUpdateEvent`].
//!
//! Serve path (continuous, per request): `cache_fallback`
//! (serve/runtime.rs:488) reads the current `ArcSwap` generation.

use std::collections::{HashSet, VecDeque};

/// Mirrors the domain key `WorkQueue` dedups on (wq:105).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DomainKey(pub u32);

/// Mirrors the webhook event kind carried by `JobSource::External`
/// (cp-core/work.rs:29). Kept as a small host-pure enum — no web-sys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookKind {
    Push,
    Other,
}

/// Mirrors `JobSource` (cp-core/work.rs:25): tags job origin without
/// affecting queue ordering. `ScheduledBatch` arrives via the timer
/// sweep, `External` via `webhook_handler`, `InitialLoad` is reserved
/// for the warm-start analogue (warm start itself never enqueues).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobSource {
    ScheduledBatch,
    External { id: u64, kind: WebhookKind },
    InitialLoad,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobSpec {
    pub domain_key: DomainKey,
    pub source: JobSource,
    pub enqueued_at: u64,
}

/// Mirrors `EnqueueResult` (wq:88): `WorkQueue::enqueue`'s outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueResult {
    Accepted,
    Deduplicated,
    QueueFull,
}

/// Mirrors `JobOutcome` (cp-core/work.rs:41): sent on the worker→
/// delivery mpsc channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobOutcome {
    Success,
    Failure,
}

/// Mirrors `SweepPhase` (collect.rs:657): the state machine `SweepSaga`
/// walks for a single scheduled-sweep run, `Init` through `Completed`
/// (or `Failed`) as the run's `BatchTracker` fills and drains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepPhase {
    Init,
    Resumed,
    BaselineReused,
    AwaitingBatch,
    BatchDrained,
    Completed,
    Failed { error: &'static str },
}

/// Mirrors `EvidenceProjectionEvent` (projection.rs:67): the fold event
/// `record_repo` writes into [`EvidenceProjection`] per completed job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceProjectionEvent {
    RepositoryStateCaptured,
    RepositoryDeleted,
    OrgStateCaptured,
}

/// Mirrors `CachedBody` (cp-web serve/state.rs:141): the page body
/// produced by `build_cached_pages`, zstd-compressed unless too small
/// to be worth it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachedBody {
    RawOnly(usize),
    Compressed(usize),
}

/// Mirrors `CachedPage` (cp-web serve/state.rs:215): the unit
/// `commit_cached_pages` atomically swaps into the `ArcSwap` `html_cache`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachedPage {
    pub body: CachedBody,
    pub generation: usize,
}

/// Mirrors `PageUpdateEvent` (cp-web serve/state.rs:65): broadcast on
/// the WS channel by `commit_cached_pages` after each finalize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageUpdateEvent {
    pub generation: usize,
}

/// Mirrors `WorkQueue` (wq:105): bounded FIFO + dedup on [`DomainKey`].
/// Backpressure returns `QueueFull`; a job already queued for a key is
/// dropped as `Deduplicated`; the key clears on dequeue.
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

/// Mirrors `BatchTracker` (wq:263, `AtomicUsize` + `Notify` in the real
/// code): the join barrier a scheduled sweep's `tracker.wait()` blocks
/// on until every `ScheduledBatch` job of the run has drained
/// (collect.rs:1125). Webhook/`External` jobs never touch this.
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

/// Mirrors `EvidenceProjection` (projection.rs:49): the write-side
/// store folded from [`EvidenceProjectionEvent`]s. Tracks whether the
/// last finalized page-set is stale relative to the projection so the
/// memo (`build_cached_pages`) can tell hit from rebuild.
#[derive(Default)]
pub struct EvidenceProjection {
    captured_keys: HashSet<DomainKey>,
    stale: bool,
}

impl EvidenceProjection {
    pub fn fold(&mut self, domain_key: DomainKey, event: EvidenceProjectionEvent) {
        match event {
            EvidenceProjectionEvent::RepositoryStateCaptured => {
                self.captured_keys.insert(domain_key);
                self.stale = true;
            }
            EvidenceProjectionEvent::RepositoryDeleted => {
                self.captured_keys.remove(&domain_key);
                self.stale = true;
            }
            EvidenceProjectionEvent::OrgStateCaptured => {
                self.stale = true;
            }
        }
    }

    #[must_use]
    pub fn repositories_captured(&self) -> usize {
        self.captured_keys.len()
    }

    #[must_use]
    pub fn is_stale(&self) -> bool {
        self.stale
    }

    fn mark_fresh(&mut self) {
        self.stale = false;
    }
}

/// Mirrors the event stream `record_repo` appends to (projection.rs:67)
/// before folding into [`EvidenceProjection`] — one entry per completed
/// job, independent of whether a finalize ever runs.
#[derive(Default)]
pub struct StreamLog {
    events_written: usize,
}

impl StreamLog {
    pub fn write_event(&mut self) {
        self.events_written += 1;
    }

    #[must_use]
    pub fn events_written(&self) -> usize {
        self.events_written
    }
}

/// Mirrors the memoized build inside `build_cached_pages`
/// (report/html.rs:253): a rebuild only happens when the projection
/// generation moved since the last build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildResult {
    Rebuild,
    Hit,
}

pub struct MemoBuilder {
    last_built_generation: Option<usize>,
    hits: usize,
    rebuilds: usize,
}

impl MemoBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_built_generation: None,
            hits: 0,
            rebuilds: 0,
        }
    }

    pub fn build(&mut self, projection_generation: usize) -> BuildResult {
        let result = if self.last_built_generation == Some(projection_generation) {
            self.hits += 1;
            BuildResult::Hit
        } else {
            self.rebuilds += 1;
            BuildResult::Rebuild
        };
        self.last_built_generation = Some(projection_generation);
        result
    }

    #[must_use]
    pub fn hits(&self) -> usize {
        self.hits
    }

    #[must_use]
    pub fn rebuilds(&self) -> usize {
        self.rebuilds
    }
}

impl Default for MemoBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Mirrors the zstd compression step inside `build_cached_pages`
/// (cp-web serve/state.rs:141) that produces [`CachedBody::Compressed`].
pub struct Compressor;

impl Compressor {
    const BYTES_PER_CAPTURED_REPOSITORY: usize = 256;
    const COMPRESSED_NUMERATOR: usize = 3;
    const COMPRESSED_DENOMINATOR: usize = 10;

    #[must_use]
    pub fn page_size(repositories_captured: usize) -> usize {
        repositories_captured * Self::BYTES_PER_CAPTURED_REPOSITORY
    }

    #[must_use]
    pub fn compress(page_size: usize) -> usize {
        (page_size * Self::COMPRESSED_NUMERATOR)
            .div_ceil(Self::COMPRESSED_DENOMINATOR)
            .max(1)
    }
}

/// Mirrors the `ArcSwap` `html_cache` (collect.rs:1004): `commit_cached_pages`
/// atomically swaps in a new generation once per finalize — never once
/// per job.
#[derive(Default)]
pub struct ArcSwapPublisher {
    generation: usize,
}

impl ArcSwapPublisher {
    pub fn publish(&mut self) -> usize {
        self.generation += 1;
        self.generation
    }

    #[must_use]
    pub fn generation(&self) -> usize {
        self.generation
    }
}

/// Mirrors `cache_fallback` (serve/runtime.rs:488): the continuous,
/// per-request serve path reading whatever generation the `ArcSwap`
/// currently holds.
#[derive(Default)]
pub struct DeliveryTail {
    publisher: ArcSwapPublisher,
    served_pages: usize,
}

impl DeliveryTail {
    pub fn publish(&mut self) -> usize {
        self.publisher.publish()
    }

    /// # Panics
    ///
    /// Panics if called before any [`Self::publish`] this delivery tail has
    /// seen — serving a page that was never published is a causal-ordering
    /// bug, not a recoverable state.
    pub fn serve(&mut self) -> usize {
        assert!(
            self.served_pages < self.publisher.generation(),
            "serve() called without a preceding publish(): served {} >= generation {}",
            self.served_pages,
            self.publisher.generation()
        );
        self.served_pages += 1;
        self.served_pages
    }

    #[must_use]
    pub fn served_pages(&self) -> usize {
        self.served_pages
    }

    #[must_use]
    pub fn arcswap_generation(&self) -> usize {
        self.publisher.generation()
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
    pub events_written: u64,
    pub memo_hits: u64,
    pub memo_rebuilds: u64,
    pub compressed_bytes_total: usize,
    pub raw_bytes_total: usize,
    pub arcswap_generation: usize,
    pub worker_executions: u64,
}

#[derive(Debug, Default, Clone)]
pub struct StepEvents {
    pub arrivals: Vec<(JobSource, EnqueueResult)>,
    pub completions: Vec<(JobSource, JobOutcome)>,
    pub page_updates: Vec<PageUpdateEvent>,
}

/// Discrete-event sim of the whole queue network. Owns one "current
/// run" of [`SweepPhase`]: a scheduled sweep transitions it
/// `Init` -> `AwaitingBatch` as `ScheduledBatch` jobs are submitted,
/// then `BatchDrained` -> `Completed` when its [`BatchTracker`] empties
/// — the moment `finalize_and_publish` fires, exactly once per run.
/// `External` jobs finalize their own single-packet run immediately on
/// completion, ungated by the scheduled tracker. [`Self::warm_start`]
/// finalizes directly from the current projection with zero worker
/// involvement.
pub struct Sim {
    config: SimConfig,
    queue: WorkQueue,
    workers: Vec<WorkerSlot>,
    batch_tracker: BatchTracker,
    projection: EvidenceProjection,
    delivery: DeliveryTail,
    stream: StreamLog,
    memo: MemoBuilder,
    metrics: Metrics,
    sweep_phase: SweepPhase,
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
            stream: StreamLog::default(),
            memo: MemoBuilder::new(),
            metrics: Metrics::default(),
            sweep_phase: SweepPhase::Completed,
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
                    if matches!(self.sweep_phase, SweepPhase::Completed) {
                        self.sweep_phase = SweepPhase::Init;
                    }
                    self.batch_tracker.increment();
                    self.sweep_phase = SweepPhase::AwaitingBatch;
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
        self.submit(JobSource::External {
            id,
            kind: WebhookKind::Push,
        })
    }

    /// Mirrors `warm_start_from_baseline` (collect.rs:1428): renders a
    /// page-set from the current [`EvidenceProjection`] without
    /// enqueuing any job — zero worker involvement, one `ArcSwap`
    /// generation bump.
    pub fn warm_start(&mut self) -> PageUpdateEvent {
        self.finalize_and_publish()
    }

    /// Mirrors `finalize_and_publish` (collect.rs:1165): the per-RUN
    /// read-side action — `build_cached_pages` then `commit_cached_pages`
    /// — producing exactly one [`PageUpdateEvent`] per call, never per
    /// packet.
    fn finalize_and_publish(&mut self) -> PageUpdateEvent {
        let page = self.build_cached_pages();
        self.commit_cached_pages(page)
    }

    /// Mirrors `build_cached_pages` (report/html.rs:253): memoized HTML
    /// build + zstd compression, gated on whether the projection moved
    /// since the last build.
    fn build_cached_pages(&mut self) -> CachedPage {
        let repositories_captured = self.projection.repositories_captured();
        let build = self.memo.build(repositories_captured);
        match build {
            BuildResult::Rebuild => self.metrics.memo_rebuilds += 1,
            BuildResult::Hit => self.metrics.memo_hits += 1,
        }
        let page_size = Compressor::page_size(repositories_captured);
        let compressed_bytes = Compressor::compress(page_size);
        self.metrics.raw_bytes_total += page_size;
        self.metrics.compressed_bytes_total += compressed_bytes;
        self.projection.mark_fresh();
        CachedPage {
            body: CachedBody::Compressed(compressed_bytes),
            generation: self.delivery.arcswap_generation(),
        }
    }

    /// Mirrors `commit_cached_pages` (collect.rs:1004): the atomic
    /// `ArcSwap` swap + `PageUpdateEvent` broadcast, generation++ exactly
    /// once per call.
    fn commit_cached_pages(&mut self, page: CachedPage) -> PageUpdateEvent {
        let _ = page;
        let generation = self.delivery.publish();
        self.delivery.serve();
        self.metrics.arcswap_generation = generation;
        PageUpdateEvent { generation }
    }

    /// Mirrors `cache_fallback` (serve/runtime.rs:488): a continuous,
    /// per-request read of whatever generation the `ArcSwap` currently
    /// holds. Independent of any run finalizing.
    #[must_use]
    pub fn cache_fallback(&self) -> usize {
        self.delivery.arcswap_generation()
    }

    #[must_use]
    pub fn sweep_phase(&self) -> SweepPhase {
        self.sweep_phase
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
                kind: WebhookKind::Push,
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
                self.metrics.worker_executions += 1;
            }
        }

        let mut finished = Vec::new();
        for slot in &mut self.workers {
            let Some(job) = slot.job else { continue };
            slot.remaining_ticks = slot.remaining_ticks.saturating_sub(1);
            if slot.remaining_ticks == 0 {
                finished.push(job);
                slot.job = None;
            }
        }

        for job in finished {
            let outcome = if (job.enqueued_at + u64::from(job.domain_key.0)).is_multiple_of(10) {
                JobOutcome::Failure
            } else {
                JobOutcome::Success
            };
            events.completions.push((job.source, outcome));
            self.metrics.completed += 1;
            if outcome == JobOutcome::Failure {
                self.metrics.failures += 1;
            } else {
                self.stream.write_event();
                self.metrics.events_written += 1;
                self.projection.fold(
                    job.domain_key,
                    EvidenceProjectionEvent::RepositoryStateCaptured,
                );
            }
            match job.source {
                JobSource::ScheduledBatch => {
                    self.batch_tracker.decrement();
                    if self.batch_tracker.is_drained()
                        && matches!(self.sweep_phase, SweepPhase::AwaitingBatch)
                    {
                        self.sweep_phase = SweepPhase::BatchDrained;
                        let update = self.finalize_and_publish();
                        events.page_updates.push(update);
                        self.sweep_phase = SweepPhase::Completed;
                    }
                }
                JobSource::External { .. } => {
                    let update = self.finalize_and_publish();
                    events.page_updates.push(update);
                }
                JobSource::InitialLoad => {}
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
    pub fn events_written(&self) -> usize {
        self.stream.events_written()
    }

    #[must_use]
    pub fn memo_hits(&self) -> usize {
        self.memo.hits()
    }

    #[must_use]
    pub fn memo_rebuilds(&self) -> usize {
        self.memo.rebuilds()
    }

    #[must_use]
    pub fn compressed_bytes_total(&self) -> usize {
        self.metrics.compressed_bytes_total
    }

    #[must_use]
    pub fn raw_bytes_total(&self) -> usize {
        self.metrics.raw_bytes_total
    }

    #[must_use]
    pub fn arcswap_generation(&self) -> usize {
        self.delivery.arcswap_generation()
    }

    #[must_use]
    pub fn worker_executions(&self) -> u64 {
        self.metrics.worker_executions
    }

    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.queue.depth() == 0 && self.in_flight() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BatchTracker, BuildResult, DomainKey, EnqueueResult, JobSource, JobSpec, MemoBuilder, Sim,
        SimConfig, SweepPhase, WebhookKind, WorkQueue,
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
            queue.enqueue(job(
                7,
                JobSource::External {
                    id: 1,
                    kind: WebhookKind::Push
                }
            )),
            EnqueueResult::Deduplicated
        );
        let dequeued = queue.dequeue().expect("job present");
        assert_eq!(dequeued.domain_key, DomainKey(7));
        assert_eq!(
            queue.enqueue(job(
                7,
                JobSource::External {
                    id: 2,
                    kind: WebhookKind::Push
                }
            )),
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

    #[test]
    fn events_written_equals_successes() {
        let config = SimConfig {
            queue_capacity: 8,
            worker_count: 16,
            service_ticks: 3,
            domain_key_span: 500,
        };
        let mut sim = Sim::new(config, 1234);
        for tick in 0..300u64 {
            sim.step(tick % 2 == 0, tick % 3 == 0);
        }
        for _ in 0..64 {
            sim.step(false, false);
        }
        assert!(sim.is_idle(), "sim must drain before checking event count");
        let successes = sim.metrics().completed - sim.metrics().failures;
        assert_eq!(
            u64::try_from(sim.events_written()).expect("event count fits u64"),
            successes,
            "one stream event per successful job, none for failures"
        );
    }

    #[test]
    fn build_after_no_projection_change_is_a_hit_not_a_rebuild() {
        let mut memo = MemoBuilder::new();
        assert_eq!(
            memo.build(5),
            BuildResult::Rebuild,
            "first build against a new generation is always a rebuild"
        );
        assert_eq!(
            memo.build(5),
            BuildResult::Hit,
            "repeating the same generation with no projection change is a hit"
        );
        assert_eq!(
            memo.build(6),
            BuildResult::Rebuild,
            "a changed generation forces a rebuild"
        );
        assert_eq!(memo.hits(), 1);
        assert_eq!(memo.rebuilds(), 2);
    }

    #[test]
    fn arcswap_generation_increments_monotonically_on_publish() {
        let config = SimConfig {
            queue_capacity: 8,
            worker_count: 16,
            service_ticks: 3,
            domain_key_span: 500,
        };
        let mut sim = Sim::new(config, 1234);
        let mut last_generation = sim.arcswap_generation();
        for tick in 0..300u64 {
            sim.step(tick % 2 == 0, tick % 3 == 0);
            let generation = sim.arcswap_generation();
            assert!(
                generation >= last_generation,
                "arcswap generation regressed from {last_generation} to {generation}"
            );
            last_generation = generation;
        }
        assert!(
            sim.arcswap_generation() > 0,
            "at least one publish must have occurred over 300 ticks"
        );
    }

    #[test]
    fn compression_ratio_stays_under_100_percent_over_cumulative_pages() {
        let config = SimConfig {
            queue_capacity: 8,
            worker_count: 16,
            service_ticks: 3,
            domain_key_span: 500,
        };
        let mut sim = Sim::new(config, 1234);
        for tick in 0..300u64 {
            sim.step(tick % 2 == 0, tick % 3 == 0);
        }
        let m = sim.metrics();
        assert!(m.raw_bytes_total > 0, "raw bytes must accumulate");
        let percent = m.compressed_bytes_total * 100 / m.raw_bytes_total;
        assert!(percent > 0, "compression percent must be positive");
        assert!(percent < 100, "compression percent must stay under 100%");
    }

    #[test]
    fn served_pages_never_exceeds_arcswap_generation() {
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
                sim.served_pages() <= sim.arcswap_generation(),
                "served {} pages before publishing generation {}",
                sim.served_pages(),
                sim.arcswap_generation()
            );
        }
    }

    /// New model test (a): scheduled read-side finalize does not occur
    /// until the run's `BatchTracker` is drained.
    #[test]
    fn scheduled_finalize_waits_for_batch_tracker_drain() {
        let config = SimConfig {
            queue_capacity: 64,
            worker_count: 4,
            service_ticks: 3,
            domain_key_span: 500,
        };
        let mut sim = Sim::new(config, 7);
        for _ in 0..12 {
            let _ignored = sim.submit(JobSource::ScheduledBatch);
        }
        assert_eq!(sim.sweep_phase(), SweepPhase::AwaitingBatch);
        let mut finalized_early = false;
        for _ in 0..1000 {
            let events = sim.step(false, false);
            if sim.batch_remaining() > 0 && !events.page_updates.is_empty() {
                finalized_early = true;
            }
            if sim.batch_remaining() == 0 {
                break;
            }
        }
        assert!(
            !finalized_early,
            "read-side finalized before the run's BatchTracker drained"
        );
        assert_eq!(sim.batch_remaining(), 0);
        assert_eq!(sim.sweep_phase(), SweepPhase::Completed);
    }

    /// New model test (b): warm start produces a page-set with zero
    /// worker executions.
    #[test]
    fn warm_start_produces_page_set_with_zero_worker_executions() {
        let mut sim = Sim::new(SimConfig::default(), 3);
        assert_eq!(sim.worker_executions(), 0);
        let before_generation = sim.arcswap_generation();
        let update = sim.warm_start();
        assert_eq!(sim.worker_executions(), 0, "warm start touches no worker");
        assert_eq!(update.generation, before_generation + 1);
        assert_eq!(sim.arcswap_generation(), before_generation + 1);
    }

    /// New model test (c): an External job flows queue -> worker ->
    /// projection without gating on any batch barrier.
    #[test]
    fn external_job_bypasses_batch_barrier() {
        let config = SimConfig {
            queue_capacity: 8,
            worker_count: 4,
            service_ticks: 2,
            domain_key_span: 500,
        };
        let mut sim = Sim::new(config, 11);
        assert_eq!(sim.batch_remaining(), 0);
        let _ignored = sim.submit_external();
        assert_eq!(
            sim.batch_remaining(),
            0,
            "external submit must not touch the batch tracker"
        );
        let mut saw_completion = false;
        for _ in 0..20 {
            let events = sim.step(false, false);
            assert_eq!(
                sim.batch_remaining(),
                0,
                "external job progressing must never move the batch tracker"
            );
            if events
                .completions
                .iter()
                .any(|(source, _)| matches!(source, JobSource::External { .. }))
            {
                saw_completion = true;
                break;
            }
        }
        assert!(saw_completion, "external job never completed");
    }

    /// New model test (d): `ArcSwap` generation increments once per
    /// finalize (per run / per warm start), not once per job.
    #[test]
    fn arcswap_generation_increments_once_per_run_not_per_job() {
        let config = SimConfig {
            queue_capacity: 64,
            worker_count: 16,
            service_ticks: 2,
            domain_key_span: 500,
        };
        let mut sim = Sim::new(config, 55);
        for _ in 0..20 {
            let _ignored = sim.submit(JobSource::ScheduledBatch);
        }
        let jobs_submitted = 20u64;
        let generation_before = sim.arcswap_generation();
        for _ in 0..1000 {
            sim.step(false, false);
            if sim.batch_remaining() == 0 && sim.is_idle() {
                break;
            }
        }
        assert!(
            sim.metrics().completed >= jobs_submitted,
            "all scheduled jobs must complete"
        );
        assert_eq!(
            sim.arcswap_generation(),
            generation_before + 1,
            "one run of many jobs must bump the generation exactly once"
        );
    }
}
