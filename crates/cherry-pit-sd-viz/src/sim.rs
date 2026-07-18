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

    /// Read-only value-snapshot iterator over queued jobs, oldest
    /// (front of the FIFO) to newest. Copies each [`JobSpec`] out;
    /// never exposes a handle back into `self.jobs`, so it structurally
    /// cannot mutate queue depth (mirrors [`crate::sd::Connector`]'s
    /// "copied value, never a handle" discipline).
    pub fn jobs(&self) -> impl Iterator<Item = JobSpec> + '_ {
        self.jobs.iter().copied()
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

/// Mirrors `Repository.updated_at` (domain/repository.rs:40): the
/// GitHub-reported last-modified marker the sweep diffs against its
/// projection baseline. `None` mirrors an absent value; two equal
/// non-empty values mean "unchanged since last sweep".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdatedAt(pub Option<u64>);

/// The per-repo decision `reuse_from_baseline` (collect.rs:1602)
/// reaches by calling `baseline::should_reuse(baseline_updated_at,
/// repo.updated_at)` (collect.rs:1633; infra/baseline.rs:65).
/// `ReuseCached` mirrors `should_reuse == true` (both `updated_at`
/// non-empty AND byte-equal, baseline.rs:70) — the cached evidence is
/// reused and NO job is spawned. `SpawnJob` mirrors the fall-through:
/// a differing or absent `updated_at` (or `force_refresh`,
/// collect.rs:1609) enqueues a [`JobSource::ScheduledBatch`] job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaselineDecision {
    ReuseCached,
    SpawnJob,
}

/// Mirrors `baseline::should_reuse` (infra/baseline.rs:65,70): reuse
/// iff both `updated_at` values are present AND byte-equal. Any
/// difference, or either side absent, falls through to spawning a job.
#[must_use]
pub fn should_reuse(baseline: UpdatedAt, current: UpdatedAt) -> bool {
    match (baseline.0, current.0) {
        (Some(b), Some(c)) => b == c,
        _ => false,
    }
}

/// The outcome counts of one inventory sweep's per-repo `should_reuse`
/// gate — mirrors the split the sweep makes between repos reused from
/// the projection baseline (no job) and repos whose `updated_at`
/// changed (a [`JobSource::ScheduledBatch`] job spawned). See
/// `build_inventory_from_api` (inventory.rs:50),
/// `reuse_from_baseline` (collect.rs:1602), and the pending-set build
/// (collect.rs:859-866,1073-1091).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct InventoryOutcome {
    pub inventoried: usize,
    pub reused_unchanged: usize,
    pub jobs_spawned: usize,
}

/// Mirrors `build_inventory_from_api` (inventory.rs:50) + the sweep's
/// per-repo `reuse_from_baseline`/`should_reuse` gate
/// (collect.rs:1602,1633; baseline.rs:65). A synchronous
/// `api.github.com` listing (`GET /orgs/{org}/repos?type=all`,
/// inventory.rs:56-61) returns an `InventoryLoad` of active repos
/// (collect.rs:215,1328); each is diffed against its projection
/// baseline `updated_at`. Unchanged repos are reused from the
/// projection (no job); changed/absent (or `force_refresh`,
/// collect.rs:1609) repos spawn a [`JobSource::ScheduledBatch`] job.
/// This is a LISTING step inside the sweep, NOT a queued job.
#[derive(Default)]
pub struct InventoryGate {
    last: InventoryOutcome,
}

impl InventoryGate {
    /// Runs the `should_reuse` gate over an inventory of
    /// `(baseline_updated_at, current_updated_at)` pairs. When
    /// `force_refresh` is set (collect.rs:1609) the baseline is
    /// skipped and every repo spawns a job. Returns the per-sweep
    /// counts.
    pub fn sweep(
        &mut self,
        repos: &[(UpdatedAt, UpdatedAt)],
        force_refresh: bool,
    ) -> InventoryOutcome {
        let mut outcome = InventoryOutcome {
            inventoried: repos.len(),
            reused_unchanged: 0,
            jobs_spawned: 0,
        };
        for &(baseline, current) in repos {
            let decision = if !force_refresh && should_reuse(baseline, current) {
                BaselineDecision::ReuseCached
            } else {
                BaselineDecision::SpawnJob
            };
            match decision {
                BaselineDecision::ReuseCached => outcome.reused_unchanged += 1,
                BaselineDecision::SpawnJob => outcome.jobs_spawned += 1,
            }
        }
        self.last = outcome;
        outcome
    }

    #[must_use]
    pub fn last(&self) -> InventoryOutcome {
        self.last
    }
}

/// Mirrors `PardosaBackend` (config/runtime.rs:41-46): the operator-
/// selectable durable-store backend behind `--pardosa-backend`. `Pgno`
/// is the `#[default]` (bin/gh-report.rs:180-181); `Nats` is the
/// selectable alternate (state.rs:569,591 `open_or_create_jetstream`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PardosaBackend {
    #[default]
    Pgno,
    Nats,
}

/// Mirrors `JetStreamAckPosition` (pardosa-nats handle.rs:23,35): the
/// sequence carried by a `PubAck.seq`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JetStreamAckPosition(pub u64);

/// Mirrors `JetStreamAppendAck` (pardosa-nats handle.rs:49): the ack
/// returned from `JetStreamHandle::append` (handle.rs:177) after
/// `js.publish_with_headers` (handle.rs:534).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JetStreamAppendAck {
    pub ack: JetStreamAckPosition,
    pub duplicate: bool,
}

/// Mirrors `NativeStore`/`NativeOrgStore` (gh-report/src/store/mod.rs:
/// 19,25) over pardosa's `PgnoBackend` (store/mod.rs:429,1755), backed
/// by `events.pgno`/`org-events.pgno` (state.rs:562,584) — the DEFAULT
/// durable-store write path.
#[derive(Default)]
pub struct NativeStore {
    events_written: usize,
}

impl NativeStore {
    fn append(&mut self) -> usize {
        self.events_written += 1;
        self.events_written
    }
}

/// Mirrors `JetStreamBackend` (pardosa-nats handle.rs:352) over
/// `JetStreamHandle` (handle.rs:120) — the ALTERNATE durable-store
/// write path selected by `PardosaBackend::Nats`. `append` mirrors
/// `JetStreamHandle::append` (handle.rs:177); the running sequence
/// mirrors `PubAck.seq` (handle.rs:23,35).
#[derive(Default)]
pub struct JetStreamBackend {
    sequence: u64,
}

impl JetStreamBackend {
    fn append(&mut self) -> JetStreamAppendAck {
        self.sequence += 1;
        JetStreamAppendAck {
            ack: JetStreamAckPosition(self.sequence),
            duplicate: false,
        }
    }
}

/// The append result of [`DurableStore::append`], shaped per active
/// [`PardosaBackend`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableAppendResult {
    Pgno(usize),
    Nats(JetStreamAppendAck),
}

/// Mirrors the durable event-store write side switched by
/// `--pardosa-backend` (bin/gh-report.rs:180-181): every write-side
/// completion appends to whichever backend is active. `Pgno`
/// ([`NativeStore`]) is the default; `Nats` ([`JetStreamBackend`]) is
/// the operator-selectable alternate — never depicted as the default
/// path.
#[derive(Default)]
pub struct DurableStore {
    backend: PardosaBackend,
    native: NativeStore,
    jetstream: JetStreamBackend,
}

impl DurableStore {
    #[must_use]
    pub fn backend(&self) -> PardosaBackend {
        self.backend
    }

    pub fn set_backend(&mut self, backend: PardosaBackend) {
        self.backend = backend;
    }

    pub fn append(&mut self) -> DurableAppendResult {
        match self.backend {
            PardosaBackend::Pgno => DurableAppendResult::Pgno(self.native.append()),
            PardosaBackend::Nats => DurableAppendResult::Nats(self.jetstream.append()),
        }
    }

    #[must_use]
    pub fn native_events_written(&self) -> usize {
        self.native.events_written
    }

    #[must_use]
    pub fn jetstream_sequence(&self) -> u64 {
        self.jetstream.sequence
    }
}

/// Mirrors the connect/refuse outcome of a `ws_session`'s
/// `try_acquire_owned` against `ws_semaphore` (serve/runtime.rs:229,
/// 252,669) — `Refused` is the 503 analogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectResult {
    Connected,
    Refused,
}

/// Mirrors the anonymous web-client population on the `/ws` route
/// (serve/runtime.rs:679,218): each connected client is a `ws_session`
/// holding an `OwnedSemaphorePermit` off `ws_semaphore =
/// Semaphore::new(ws_max_connections())` (runtime.rs:669, default 200,
/// config.rs:243) plus a `broadcast::Receiver<PageUpdateEvent>`
/// (`state.ws_broadcast().subscribe()`, runtime.rs:255). There is NO
/// live connected-client count in production; [`Self::connected`] is a
/// SIM-owned quantity only, never a real gh-report metric — prefer
/// reading permits-in-use vs [`Self::max_connections`].
pub struct ClientPool {
    max_connections: usize,
    connected: usize,
}

impl ClientPool {
    #[must_use]
    pub fn new(max_connections: usize) -> Self {
        Self {
            max_connections,
            connected: 0,
        }
    }

    pub fn connect(&mut self) -> ConnectResult {
        if self.connected >= self.max_connections {
            return ConnectResult::Refused;
        }
        self.connected += 1;
        ConnectResult::Connected
    }

    pub fn disconnect(&mut self) {
        self.connected = self.connected.saturating_sub(1);
    }

    #[must_use]
    pub fn permits_in_use(&self) -> usize {
        self.connected
    }

    #[must_use]
    pub fn max_connections(&self) -> usize {
        self.max_connections
    }

    /// Mirrors `AppState::send_page_update` (gh-report state.rs:323)
    /// fanning a [`PageUpdateEvent`] out to every live
    /// `broadcast::Receiver` (runtime.rs:288) on `commit_cached_pages`.
    /// Returns the delivery count — the number of currently-subscribed
    /// sim clients that received this push.
    #[must_use]
    pub fn broadcast(&self, _event: PageUpdateEvent) -> usize {
        self.connected
    }
}

/// Mirrors `RateLimitState` + `BudgetGate` (gh-report `github/`
/// `rate_limit.rs`, `github/budget.rs`; acquired via `budget_gate.acquire`
/// in `worker_pool.rs`) — the self-imposed call budget `worker_loop`
/// must acquire before `LiveEvaluator::evaluate` (collect.rs:133)
/// issues its `api.github.com` calls. Exhaustion halts further calls
/// until [`Self::reset_epoch`].
pub struct BudgetGate {
    budget_per_epoch: u32,
    used_this_epoch: u32,
}

impl BudgetGate {
    #[must_use]
    pub fn new(budget_per_epoch: u32) -> Self {
        Self {
            budget_per_epoch,
            used_this_epoch: 0,
        }
    }

    /// Attempts to acquire `calls` units of budget atomically — either
    /// all `calls` are granted or none are, mirroring `tokio::join!`
    /// issuing the six concurrent collector calls as one bounded unit
    /// (collect.rs:143-149).
    pub fn acquire(&mut self, calls: u32) -> bool {
        if self.used_this_epoch + calls > self.budget_per_epoch {
            return false;
        }
        self.used_this_epoch += calls;
        true
    }

    pub fn reset_epoch(&mut self) {
        self.used_this_epoch = 0;
    }

    /// Returns previously-acquired budget, e.g. when the acquiring
    /// worker found no job left to dispatch after all.
    pub fn release(&mut self, calls: u32) {
        self.used_this_epoch = self.used_this_epoch.saturating_sub(calls);
    }

    #[must_use]
    pub fn used(&self) -> u32 {
        self.used_this_epoch
    }

    #[must_use]
    pub fn budget(&self) -> u32 {
        self.budget_per_epoch
    }
}

/// The bounded set of concurrent `api.github.com` REST calls
/// `LiveEvaluator::evaluate` issues per job after `repo_details`
/// (collect.rs:133,143-149, client.rs:897): `security_policy::evaluate`,
/// `ghas_scanning::evaluate`, `dependabot::evaluate`,
/// `branch_protection::evaluate`, `codeowners::evaluate`,
/// `last_commit::fetch_last_commit` — six calls, gated as one unit by
/// [`BudgetGate`].
pub const GITHUB_CALLS_PER_JOB: u32 = 6;

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
    /// Mirrors `ws_max_connections()` (serve/config.rs:243, default 200).
    pub ws_max_connections: usize,
    /// Per-epoch `BudgetGate` budget (github/budget.rs). Generous by
    /// default so it does not bind ordinary sim runs; tests exercising
    /// gate exhaustion set this explicitly.
    pub github_budget_per_epoch: u32,
    /// Ticks between `BudgetGate` epoch resets.
    pub github_epoch_ticks: u64,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            queue_capacity: 32,
            worker_count: 16,
            service_ticks: 4,
            domain_key_span: 24,
            ws_max_connections: 200,
            github_budget_per_epoch: 10_000,
            github_epoch_ticks: 1_000,
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
    pub ws_deliveries_total: u64,
    pub ws_connects_refused: u64,
    pub github_calls_stalled: u64,
}

#[derive(Debug, Default, Clone)]
pub struct StepEvents {
    pub arrivals: Vec<(JobSource, EnqueueResult)>,
    pub completions: Vec<(JobSource, JobOutcome)>,
    pub page_updates: Vec<PageUpdateEvent>,
    /// Per-`PageUpdateEvent` fan-out delivery count (parallel to
    /// `page_updates`) — [`ClientPool::broadcast`] reaching every
    /// currently-subscribed sim client.
    pub ws_deliveries: Vec<usize>,
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
    durable_store: DurableStore,
    client_pool: ClientPool,
    github_gate: BudgetGate,
    inventory_gate: InventoryGate,
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
            durable_store: DurableStore::default(),
            client_pool: ClientPool::new(config.ws_max_connections),
            github_gate: BudgetGate::new(config.github_budget_per_epoch),
            inventory_gate: InventoryGate::default(),
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

    /// Mirrors the sweep's inventory-listing + `should_reuse` gate
    /// (`build_inventory_from_api` inventory.rs:50; `reuse_from_baseline`
    /// collect.rs:1602,1633; baseline.rs:65): lists all `repos` as
    /// `(baseline_updated_at, current_updated_at)` pairs, reuses cached
    /// evidence for unchanged repos (no job), and enqueues a
    /// [`JobSource::ScheduledBatch`] job for each changed/absent repo
    /// (all repos when `force_refresh`, collect.rs:1609). Returns the
    /// per-sweep counts; the spawned jobs enter the shared
    /// [`WorkQueue`].
    pub fn run_inventory_sweep(
        &mut self,
        repos: &[(UpdatedAt, UpdatedAt)],
        force_refresh: bool,
    ) -> InventoryOutcome {
        let outcome = self.inventory_gate.sweep(repos, force_refresh);
        for _ in 0..outcome.jobs_spawned {
            let _ignored = self.submit(JobSource::ScheduledBatch);
        }
        outcome
    }

    #[must_use]
    pub fn inventory_outcome(&self) -> InventoryOutcome {
        self.inventory_gate.last()
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

    #[must_use]
    pub fn durable_backend(&self) -> PardosaBackend {
        self.durable_store.backend()
    }

    pub fn set_durable_backend(&mut self, backend: PardosaBackend) {
        self.durable_store.set_backend(backend);
    }

    #[must_use]
    pub fn native_events_written(&self) -> usize {
        self.durable_store.native_events_written()
    }

    #[must_use]
    pub fn jetstream_sequence(&self) -> u64 {
        self.durable_store.jetstream_sequence()
    }

    pub fn connect_client(&mut self) -> ConnectResult {
        let result = self.client_pool.connect();
        if result == ConnectResult::Refused {
            self.metrics.ws_connects_refused += 1;
        }
        result
    }

    pub fn disconnect_client(&mut self) {
        self.client_pool.disconnect();
    }

    #[must_use]
    pub fn ws_permits_in_use(&self) -> usize {
        self.client_pool.permits_in_use()
    }

    #[must_use]
    pub fn ws_max_connections(&self) -> usize {
        self.client_pool.max_connections()
    }

    #[must_use]
    pub fn github_budget(&self) -> u32 {
        self.github_gate.budget()
    }

    #[must_use]
    pub fn github_calls_used(&self) -> u32 {
        self.github_gate.used()
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

        if self
            .tick
            .is_multiple_of(self.config.github_epoch_ticks.max(1))
        {
            self.github_gate.reset_epoch();
        }

        for slot in &mut self.workers {
            if slot.job.is_none() && self.github_gate.acquire(GITHUB_CALLS_PER_JOB) {
                if let Some(job) = self.queue.dequeue() {
                    slot.job = Some(job);
                    slot.remaining_ticks = self.config.service_ticks.max(1);
                    self.metrics.worker_executions += 1;
                } else {
                    self.github_gate.release(GITHUB_CALLS_PER_JOB);
                }
            } else if slot.job.is_none() {
                self.metrics.github_calls_stalled += 1;
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
                let _ = self.durable_store.append();
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
                        let delivered = self.client_pool.broadcast(update);
                        self.metrics.ws_deliveries_total += delivered as u64;
                        events.page_updates.push(update);
                        events.ws_deliveries.push(delivered);
                        self.sweep_phase = SweepPhase::Completed;
                    }
                }
                JobSource::External { .. } => {
                    let update = self.finalize_and_publish();
                    let delivered = self.client_pool.broadcast(update);
                    self.metrics.ws_deliveries_total += delivered as u64;
                    events.page_updates.push(update);
                    events.ws_deliveries.push(delivered);
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

    /// Read-only value-snapshot enumeration of in-queue jobs, FIFO
    /// order (oldest enqueued first), for a live "now" dots view.
    /// Copies each [`JobSpec`] out via [`WorkQueue::jobs`] — takes
    /// `&self`, never mutates queue depth.
    #[must_use]
    pub fn queue_jobs(&self) -> Vec<JobSpec> {
        self.queue.jobs().collect()
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
        BatchTracker, BuildResult, ConnectResult, DomainKey, EnqueueResult, GITHUB_CALLS_PER_JOB,
        JobSource, JobSpec, MemoBuilder, PardosaBackend, Sim, SimConfig, SweepPhase, UpdatedAt,
        WebhookKind, WorkQueue, should_reuse,
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
    fn work_queue_jobs_enumerates_fifo_order_without_mutating_depth() {
        let mut queue = WorkQueue::new(4);
        queue.enqueue(job(1, JobSource::ScheduledBatch));
        queue.enqueue(job(2, JobSource::ScheduledBatch));
        queue.enqueue(job(3, JobSource::ScheduledBatch));
        let depth_before = queue.depth();
        let keys: Vec<u32> = queue.jobs().map(|spec| spec.domain_key.0).collect();
        assert_eq!(
            keys,
            vec![1, 2, 3],
            "enumeration must be FIFO oldest->newest"
        );
        assert_eq!(
            queue.depth(),
            depth_before,
            "enumeration must not mutate depth"
        );
    }

    #[test]
    fn sim_queue_jobs_count_matches_depth_and_is_unaffected_by_enumeration() {
        let config = SimConfig {
            queue_capacity: 8,
            worker_count: 0,
            service_ticks: 3,
            domain_key_span: 500,
            ..SimConfig::default()
        };
        let mut sim = Sim::new(config, 99);
        for _ in 0..5 {
            let _ignored = sim.submit(JobSource::ScheduledBatch);
        }
        let depth_before = sim.queue_depth();
        let jobs = sim.queue_jobs();
        assert_eq!(jobs.len(), depth_before);
        assert_eq!(
            sim.queue_depth(),
            depth_before,
            "enumeration must not change depth"
        );
        let _ignored = sim.queue_jobs();
        assert_eq!(
            sim.queue_depth(),
            depth_before,
            "a second enumeration is still a no-op on depth"
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
            ..SimConfig::default()
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
            ..SimConfig::default()
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
            ..SimConfig::default()
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
            ..SimConfig::default()
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
            ..SimConfig::default()
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
            ..SimConfig::default()
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
            ..SimConfig::default()
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
            ..SimConfig::default()
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
            ..SimConfig::default()
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
            ..SimConfig::default()
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

    /// New component test (a): backend routing — `Nats` routes appends
    /// to the `JetStream` store and increments its `JetStreamAckPosition`
    /// / sequence; `Pgno` routes to the native store; default is `Pgno`.
    #[test]
    fn backend_routing_default_pgno_switch_nats_increments_sequence() {
        let mut sim = Sim::new(SimConfig::default(), 21);
        assert_eq!(
            sim.durable_backend(),
            PardosaBackend::Pgno,
            "default is Pgno"
        );
        let _ignored = sim.submit_external();
        for _ in 0..20 {
            let events = sim.step(false, false);
            if !events.completions.is_empty() {
                break;
            }
        }
        assert!(
            sim.native_events_written() > 0,
            "Pgno-routed append must land in the native store"
        );
        assert_eq!(
            sim.jetstream_sequence(),
            0,
            "Nats store untouched while Pgno active"
        );

        sim.set_durable_backend(PardosaBackend::Nats);
        assert_eq!(sim.durable_backend(), PardosaBackend::Nats);
        let native_before = sim.native_events_written();
        let _ignored = sim.submit_external();
        for _ in 0..20 {
            let events = sim.step(false, false);
            if !events.completions.is_empty() {
                break;
            }
        }
        assert!(
            sim.jetstream_sequence() > 0,
            "Nats-routed append must increment JetStreamAckPosition/sequence"
        );
        assert_eq!(
            sim.native_events_written(),
            native_before,
            "native store must not receive further appends once Nats is active"
        );
    }

    /// New component test (b): `PageUpdateEvent` on `commit_cached_pages`
    /// fans out to ALL currently-connected sim clients (N clients -> N
    /// deliveries; 0 connected -> 0).
    #[test]
    fn page_update_fans_out_to_all_connected_clients() {
        let mut sim = Sim::new(SimConfig::default(), 4);

        let update = sim.warm_start();
        let _ = update;
        assert_eq!(sim.ws_permits_in_use(), 0);

        for _ in 0..5 {
            assert_eq!(sim.connect_client(), ConnectResult::Connected);
        }
        assert_eq!(sim.ws_permits_in_use(), 5);

        let _ignored = sim.submit_external();
        let mut deliveries = None;
        for _ in 0..20 {
            let events = sim.step(false, false);
            if let Some(&count) = events.ws_deliveries.first() {
                deliveries = Some(count);
                break;
            }
        }
        assert_eq!(
            deliveries,
            Some(5),
            "5 connected clients must all receive the PageUpdateEvent push"
        );

        for _ in 0..5 {
            sim.disconnect_client();
        }
        assert_eq!(sim.ws_permits_in_use(), 0);
        let _ignored = sim.submit_external();
        let mut deliveries_after_disconnect = None;
        for _ in 0..20 {
            let events = sim.step(false, false);
            if let Some(&count) = events.ws_deliveries.first() {
                deliveries_after_disconnect = Some(count);
                break;
            }
        }
        assert_eq!(
            deliveries_after_disconnect,
            Some(0),
            "zero connected clients must receive zero deliveries"
        );
    }

    /// New component test (c): client connect respects the
    /// `ws_max_connections` cap (permits-in-use never exceeds cap;
    /// connect beyond cap is refused — the 503 analogue).
    #[test]
    fn client_connect_respects_ws_max_connections_cap() {
        let config = SimConfig {
            ws_max_connections: 3,
            ..SimConfig::default()
        };
        let mut sim = Sim::new(config, 8);
        assert_eq!(sim.connect_client(), ConnectResult::Connected);
        assert_eq!(sim.connect_client(), ConnectResult::Connected);
        assert_eq!(sim.connect_client(), ConnectResult::Connected);
        assert_eq!(sim.ws_permits_in_use(), 3);
        assert_eq!(
            sim.connect_client(),
            ConnectResult::Refused,
            "connect beyond cap must be refused (503 analogue)"
        );
        assert_eq!(
            sim.ws_permits_in_use(),
            3,
            "permits-in-use must never exceed the cap"
        );
        sim.disconnect_client();
        assert_eq!(
            sim.connect_client(),
            ConnectResult::Connected,
            "freed permit allows a new connect"
        );
    }

    /// New component test (d): worker GitHub calls respect the budget
    /// gate — calls in an epoch never exceed the budget; exhaustion
    /// halts further calls until reset.
    #[test]
    fn worker_github_calls_respect_budget_gate() {
        let config = SimConfig {
            queue_capacity: 64,
            worker_count: 4,
            service_ticks: 10,
            domain_key_span: 500,
            github_budget_per_epoch: GITHUB_CALLS_PER_JOB,
            github_epoch_ticks: 5,
            ..SimConfig::default()
        };
        let mut sim = Sim::new(config, 17);
        for _ in 0..4 {
            let _ignored = sim.submit(JobSource::ScheduledBatch);
        }
        sim.step(false, false);
        assert!(
            sim.github_calls_used() <= sim.github_budget(),
            "used {} must never exceed budget {}",
            sim.github_calls_used(),
            sim.github_budget()
        );
        assert!(
            sim.in_flight() <= 1,
            "budget for one job must halt further dispatch until reset: in_flight {}",
            sim.in_flight()
        );

        for _ in 0..200 {
            sim.step(false, false);
            assert!(sim.github_calls_used() <= sim.github_budget());
        }
        assert_eq!(
            sim.metrics().completed,
            4,
            "all 4 jobs eventually complete once the gate resets across epochs"
        );
    }

    /// New provenance test: `should_reuse` reuses cached evidence only
    /// when both `updated_at` values are present and byte-equal
    /// (baseline.rs:70).
    #[test]
    fn should_reuse_only_on_equal_present_updated_at() {
        assert!(should_reuse(UpdatedAt(Some(5)), UpdatedAt(Some(5))));
        assert!(!should_reuse(UpdatedAt(Some(5)), UpdatedAt(Some(6))));
        assert!(!should_reuse(UpdatedAt(None), UpdatedAt(Some(5))));
        assert!(!should_reuse(UpdatedAt(Some(5)), UpdatedAt(None)));
        assert!(!should_reuse(UpdatedAt(None), UpdatedAt(None)));
    }

    /// New provenance test: only repos whose `updated_at` differs from
    /// the baseline spawn jobs; unchanged repos spawn zero jobs and are
    /// counted as reused; the spawned jobs enter the queue.
    #[test]
    fn only_updated_at_changed_repos_spawn_jobs() {
        let mut sim = Sim::new(SimConfig::default(), 5);
        let repos = [
            (UpdatedAt(Some(1)), UpdatedAt(Some(1))),
            (UpdatedAt(Some(2)), UpdatedAt(Some(9))),
            (UpdatedAt(Some(3)), UpdatedAt(Some(3))),
            (UpdatedAt(None), UpdatedAt(Some(4))),
            (UpdatedAt(Some(5)), UpdatedAt(None)),
        ];
        let outcome = sim.run_inventory_sweep(&repos, false);
        assert_eq!(outcome.inventoried, 5);
        assert_eq!(
            outcome.reused_unchanged, 2,
            "two repos with equal updated_at are reused, no job"
        );
        assert_eq!(
            outcome.jobs_spawned, 3,
            "changed/absent updated_at repos spawn a ScheduledBatch job each"
        );
        assert_eq!(
            outcome.reused_unchanged + outcome.jobs_spawned,
            outcome.inventoried,
            "every inventoried repo is either reused or spawns a job"
        );
        assert_eq!(
            sim.queue_depth(),
            3,
            "only the three changed repos' jobs entered the queue"
        );
    }

    /// New provenance test: `force_refresh` skips the baseline so every
    /// inventoried repo spawns a job, even unchanged ones.
    #[test]
    fn force_refresh_spawns_jobs_for_all_repos() {
        let mut sim = Sim::new(SimConfig::default(), 5);
        let repos = [
            (UpdatedAt(Some(1)), UpdatedAt(Some(1))),
            (UpdatedAt(Some(2)), UpdatedAt(Some(2))),
            (UpdatedAt(Some(3)), UpdatedAt(Some(3))),
        ];
        let outcome = sim.run_inventory_sweep(&repos, true);
        assert_eq!(outcome.inventoried, 3);
        assert_eq!(outcome.reused_unchanged, 0, "force_refresh reuses nothing");
        assert_eq!(
            outcome.jobs_spawned, 3,
            "force_refresh spawns a job for every repo"
        );
        assert_eq!(sim.queue_depth(), 3);
    }

    /// New provenance test: a successful event write increments ONLY
    /// the active backend's counter (the single `record()` facade
    /// routes to one backend, never both). `Pgno` active: native store
    /// grows, `JetStream` sequence stays 0. `Nats` active: `JetStream`
    /// sequence grows, native store frozen.
    #[test]
    fn event_write_increments_only_the_active_backend() {
        let mut sim = Sim::new(SimConfig::default(), 21);
        assert_eq!(sim.durable_backend(), PardosaBackend::Pgno);
        let _ignored = sim.submit_external();
        for _ in 0..20 {
            if !sim.step(false, false).completions.is_empty() {
                break;
            }
        }
        let native_after_pgno = sim.native_events_written();
        assert!(native_after_pgno > 0, "Pgno write lands in native store");
        assert_eq!(
            sim.jetstream_sequence(),
            0,
            "JetStream untouched while Pgno active — no fan-out to both"
        );

        sim.set_durable_backend(PardosaBackend::Nats);
        let _ignored = sim.submit_external();
        for _ in 0..20 {
            if !sim.step(false, false).completions.is_empty() {
                break;
            }
        }
        assert!(
            sim.jetstream_sequence() > 0,
            "Nats write increments JetStream sequence"
        );
        assert_eq!(
            sim.native_events_written(),
            native_after_pgno,
            "native store frozen while Nats active — never both"
        );
    }
}
