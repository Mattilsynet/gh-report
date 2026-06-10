//! Shared application state for the service.
//!
//! ## State topology
//!
//! `AppState` holds three focused sub-aggregates plus cross-cutting fields:
//!
//! - **[`WebhookState`]** — webhook secret, replay protection, debounce.
//! - **[`GithubState`]** — budget gate, rate limit, API client,
//!   repo detail cache.
//! - **[`EvidenceState`]** — evidence store, HTML cache, WebSocket
//!   broadcast, org summary, batch tracker.
//!
//! Cross-cutting fields (run metadata, work queue, worker pool guard,
//! event bus) remain directly on `AppState`.
//!
//! ## Credential lifecycle
//!
//! GitHub App tokens auto-refresh via `ensure_credential()` on the
//! long-lived client. PAT credential changes via environment variable
//! require a daemon restart.
//!
//! [`REPO_CACHE_TTL_HOURS`]: crate::config::REPO_CACHE_TTL_HOURS

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;

use arc_swap::ArcSwap;
use cherry_pit_agent::InProcessEventBus;
#[cfg(test)]
use cherry_pit_core::testing::InMemoryEventStore;
use cherry_pit_core::{AggregateId, ListableEventStore};
use cherry_pit_gateway::MsgpackFileStore;
use cherry_pit_projection::FileProjectionStore;
use jiff::Timestamp;

pub use crate::infra::server::state::{CachedPage, PageUpdateEvent};

pub use crate::domain::events::DomainEvent;

/// Concrete event-store type wired into gh-report.
///
/// The persistent file-per-aggregate backend
/// [`cherry_pit_gateway::MsgpackFileStore`] writes one `<id>.msgpack`
/// file per aggregate under `<events_dir>` and holds a `.lock` flock
/// per CHE-0043:R1 (acquired lazily on first write). All production
/// paths construct the store via [`AppState::with_stores`]; test paths
/// that don't need durable persistence construct an `AppState` without
/// an `event_store` (see [`AppState::new`] under `#[cfg(test)]`).
pub type EventStoreImpl = MsgpackFileStore<DomainEvent>;

pub use crate::app::evidence_service::EvidenceState;
pub use crate::app::github_infra::GithubState;
pub use crate::app::services::repo_service::RepoService;
pub use crate::app::services::run_service::RunService;
pub use crate::app::services::webhook_service::WebhookService;
pub use crate::app::services::{Merger, MergerCommand};
pub use crate::app::webhook_context::WebhookState;

use crate::app::collect::JobContext;
use crate::app::work_queue::WorkQueue;
use crate::domain::run::RunMetadata;

/// Embedded CSS stylesheet, compiled into the binary at build time.
const STYLESHEET: &str = include_str!("../../templates/style.css");

/// Embedded WebSocket client script, compiled into the binary at build time.
const WS_CLIENT_JS: &str = include_str!("../../templates/ws.js");

/// Pre-computed `CachedPage` for `style.css`.
///
/// Zstd compression and SHA-256 hashing are performed once at first
/// access (process startup), not on every publish cycle. Subsequent
/// publishes clone via `Bytes` refcount increment (~1 ns).
pub static CACHED_STYLESHEET: LazyLock<CachedPage> =
    LazyLock::new(|| CachedPage::new("style.css", STYLESHEET.as_bytes().to_vec()));

/// Pre-computed `CachedPage` for `ws.js`.
///
/// Same rationale as [`CACHED_STYLESHEET`]: compute once, clone cheaply.
pub static CACHED_WS_JS: LazyLock<CachedPage> =
    LazyLock::new(|| CachedPage::new("ws.js", WS_CLIENT_JS.as_bytes().to_vec()));

/// Shared application state.
///
/// Passed via `Arc<AppState>` to all axum handlers and the collection pipeline.
/// Implements [`crate::infra::server::state::ServerState`] so that the
/// generic in-memory HTTP server can serve pages, health probes, and
/// WebSocket updates without any governance-specific knowledge.
///
/// ## Sub-aggregates
///
/// Access grouped fields via accessor methods:
/// - [`webhook()`](Self::webhook) — webhook secret, replay, debounce
/// - [`github()`](Self::github) — budget gate, rate limit, client, cache
/// - [`evidence()`](Self::evidence) — evidence store, HTML cache, WS broadcast, org summary, batch tracker
pub(crate) type WorkerPoolHandles =
    std::sync::Mutex<Option<(tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()>)>>;

pub struct AppState {
    /// When this service instance started.
    pub started_at: Timestamp,
    /// Currently running collection, if any.
    pub current_run: ArcSwap<Option<RunMetadata>>,
    /// Last successfully completed collection run.
    pub last_completed_run: ArcSwap<Option<RunMetadata>>,
    /// Work queue for the reactor. Webhook-triggered jobs are enqueued
    /// here and processed by the long-lived worker pool. Scheduled batch
    /// collection still uses the existing pipeline (adapter approach).
    pub(crate) work_queue: Arc<WorkQueue<JobContext>>,
    /// Guard ensuring the worker pool + delivery task are started exactly once.
    /// Initialized by `ensure_worker_pool()` after the first successful
    /// credential resolution. The outer `OnceCell` enforces single-init; the
    /// inner `Mutex<Option<...>>` lets the shutdown path *take* both handles
    /// (`tokio::sync::OnceCell` exposes no owning-take through `&self`) so
    /// they can be awaited to drain. Tuple: (`worker_pool_handle`,
    /// `delivery_task_handle`).
    pub(crate) worker_pool_started: tokio::sync::OnceCell<WorkerPoolHandles>,

    /// Durable per-aggregate event store.
    ///
    /// Wired at WU-6 v2 B3' (charter `wu6v2-charter-1778415390`,
    /// `AdjustIntent` option 2). Constructed at
    /// `<store_dir>/events/<org>/`; the singleton aggregate id is
    /// [`crate::projection::ORG_GOVERNANCE_AGGREGATE_ID`] (Tension-2
    /// single-aggregate lock).
    ///
    /// `None` only in test-builder paths that don't supply a
    /// `store_dir`. Daemon construction always supplies it.
    pub event_store: Option<Arc<EventStoreImpl>>,

    /// Durable projection snapshot + checkpoint store.
    ///
    /// Wired at WU-6 v2 B4' (charter `wu6v2-charter-1778415390`).
    /// Constructed at `<store_dir>/projections/<org>/` with
    /// `projection_name = "evidence"`. cherry-pit-projection composes
    /// on-disk filenames as
    /// `<aggregate_id>-evidence.snapshot.msgpack` and
    /// `<aggregate_id>-evidence.checkpoint.msgpack` per CHE-0048:R1
    /// (file per `(aggregate, projection)`); the snapshot is written
    /// strictly before the sibling checkpoint per CHE-0048:R2. With
    /// the singleton [`crate::projection::ORG_GOVERNANCE_AGGREGATE_ID`]
    /// (`= 1`) the artefacts are
    /// `<store_dir>/projections/<org>/1-evidence.snapshot.msgpack` and
    /// `<store_dir>/projections/<org>/1-evidence.checkpoint.msgpack`.
    /// The events/ and projections/ subtrees are disjoint per BC-v2-13.
    ///
    /// **B4' is additive wiring only**: this handle is constructed and
    /// held but **not yet driven**. B5' wires the
    /// `ProjectionDriverExt` (CHE-0051:R5) that replays from the
    /// `event_store` and persists snapshots through this handle. Until
    /// then the field exists to surface a non-zero `cargo tree` dep on
    /// cherry-pit-projection and to lock the composition shape.
    ///
    /// `None` only in test-builder paths that don't supply a
    /// `store_dir`. Daemon construction always supplies it.
    pub projection_store: Option<Arc<FileProjectionStore<crate::projection::EvidenceProjection>>>,

    /// In-process domain-event bus driving the snapshot-fast-path
    /// projection runtime (B5', charter `wu6v2-charter-1778415390`).
    ///
    /// Per CHE-0024:§7 + CHE-0051:R2/R5: handler registered via
    /// [`crate::app::projection_runtime::register_projection_handler`]
    /// fans out each published [`DomainEvent`] envelope to the
    /// projection state. Synchronous within `publish` (no spawn).
    ///
    /// As of M5.B2.5 (`adr-fmt-587i`) this is the sole in-process
    /// domain event bus on `AppState`: the legacy tokio-broadcast
    /// `EventBus` field (formerly held by `AppState`) was removed and
    /// the logging subscriber rewritten onto this bus via
    /// [`crate::app::event_logging::register_logging_subscriber`]. A
    /// `CommandGateway` / `Aggregate` impl / `HandleCommand` remain
    /// locked-out for v2.
    ///
    /// Always present (even in test-builder paths); cheap to
    /// construct.
    pub bus: Arc<InProcessEventBus<DomainEvent>>,

    /// Materialised projection state shared with the bus handler.
    ///
    /// `Mutex` (not `RwLock`) because every bus delivery is a write —
    /// the read pattern (rendering / queries) is bursty and
    /// orthogonal in time. Poison recovery handled inside the
    /// registered handler via `PoisonError::into_inner`.
    ///
    /// Initialised to `EvidenceProjection::default()` by both
    /// constructors. Populated by [`Self::bootstrap_replay_state`]
    /// (called from [`Self::snapshot_fast_path_init`]) during
    /// daemon boot: a single unified replay folds every aggregate's
    /// events into both routing indices and projection state. The
    /// CHE-0048 line-24 replay-as-rebuild exemption applies — there
    /// is no on-disk snapshot/checkpoint surface; the durable event
    /// log under [`MsgpackFileStore`] is the SSOT (bd `adr-fmt-5rwbu`).
    pub(crate) projection_state: Arc<Mutex<crate::projection::EvidenceProjection>>,

    /// Last-applied envelope sequence for the projection state.
    ///
    /// Updated by the bus handler via `fetch_max(AcqRel)` so a future
    /// publisher that delivers out-of-order envelopes still leaves
    /// the atomic at the highest applied sequence (monotonic
    /// non-decreasing — see B5' tests). Future schedulers (snapshot
    /// persistence, lag metrics) read this without locking
    /// `projection_state`.
    ///
    /// `0` ⇒ no envelope applied yet
    /// (see [`crate::app::projection_runtime::NO_SEQUENCE_APPLIED`]).
    pub(crate) projection_checkpoint_seq: Arc<AtomicU64>,

    /// Webhook ingestion concerns (secret, replay, debounce).
    webhook: WebhookState,
    /// GitHub API infrastructure (budget, rate limit, client, cache).
    github: GithubState,
    /// Evidence data store and publication infrastructure.
    evidence: EvidenceState,

    /// `RunService` — load → handle → append → publish for the
    /// [`Run`](crate::domain::aggregates::run::Run) aggregate.
    /// Skeleton in B7'a; constructor + types wired in B7'b-1;
    /// method bodies wired in B7'b-2..3; production sites
    /// migrate in B7'c (CHE-0054:R4).
    pub run_service: Arc<RunService>,
    /// `RepoService` — same triad for the
    /// [`Repo`](crate::domain::aggregates::repo::Repo) aggregate.
    pub repo_service: Arc<RepoService>,
    /// `WebhookService` — same triad for the
    /// [`WebhookDelivery`](crate::domain::aggregates::webhook::WebhookDelivery)
    /// aggregate.
    pub webhook_service: Arc<WebhookService>,

    /// Lifetime guard for the Merger task spawned by
    /// [`Merger::spawn`]. Dropping [`AppState`] drops this
    /// [`tokio::task::JoinHandle`] which signals shutdown via the
    /// channel-closed branch in [`Merger`]'s loop. Never joined
    /// explicitly — process exit reclaims the task.
    #[expect(
        dead_code,
        reason = "lifetime guard; the task is kept alive by holding this handle"
    )]
    pub(crate) merger_handle: tokio::task::JoinHandle<()>,

    /// Shared per-aggregate last-applied-sequence tracker
    /// (CHE-0054:R6 / CHE-0042:R3). Populated by
    /// [`Self::bootstrap_replay_state`] at boot (Track 7.5, M3)
    /// and by service `append` paths during live operation.
    pub(crate) next_seq: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,

    pub(crate) runs_by_key: Arc<Mutex<HashMap<String, AggregateId>>>,
    pub(crate) repos_by_key: Arc<Mutex<HashMap<String, AggregateId>>>,
    pub(crate) deliveries_by_id: Arc<Mutex<HashMap<String, AggregateId>>>,

    /// In-process gate serialising concurrent
    /// [`crate::app::collect::run`] invocations against this
    /// `AppState` (mission `adr-fmt-cq7vb.8.2`).
    ///
    /// `run` acquires this `Arc<tokio::sync::Mutex<()>>` as its first
    /// action and holds an `OwnedMutexGuard` for the lifetime of the
    /// sweep — releasing only when the run completes (Ok or Err) or
    /// when the future is cancelled. Two concurrent in-process calls
    /// against the same `AppState` therefore execute strictly one
    /// after the other, eliminating the
    /// `state.evidence().org_summary` and
    /// `state.evidence().batch_tracker` clobber windows in
    /// [`crate::app::collect::SweepSaga::new`] and
    /// [`crate::app::collect::enqueue_and_await_batch`].
    ///
    /// The on-disk `lock::acquire` in
    /// [`crate::app::collect::prepare_collection`] is retained as the
    /// cross-process second line of defence (one daemon process can
    /// still race another against the same `store_dir`); this
    /// in-process lock guards the singleton `AppState` itself.
    pub sweep_lock: Arc<tokio::sync::Mutex<()>>,
}

impl AppState {
    /// Access webhook ingestion fields (secret, replay cache, debounce cache).
    #[inline]
    pub fn webhook(&self) -> &WebhookState {
        &self.webhook
    }

    /// Access GitHub API infrastructure (budget gate, rate limit, client, cache).
    #[inline]
    pub fn github(&self) -> &GithubState {
        &self.github
    }

    /// Access evidence service (store, HTML cache, WS broadcast, org summary, batch tracker).
    #[inline]
    pub fn evidence(&self) -> &EvidenceState {
        &self.evidence
    }

    /// Acquire the projection-state lock, panicking on poison.
    ///
    /// Idiom-collapse helper post-M2.cd (brief `.ooda/brief-m2cd-1-tidy.md`,
    /// linus M2.cd Low finding F-LOW-1): replaces ~30 call sites that spelt
    /// `state.projection_state.lock().expect("projection_state mutex poisoned")`
    /// inline. Panic semantics match every replaced site verbatim.
    ///
    /// Sole writer to `projection_state` is the bus-driven `Projection::apply`
    /// path (CHE-0048:R2). Callers must follow D-CD-3: never hold the returned
    /// `MutexGuard` across an `.await`.
    pub(crate) fn lock_projection(
        &self,
    ) -> std::sync::MutexGuard<'_, crate::projection::EvidenceProjection> {
        self.projection_state
            .lock()
            .expect("projection_state mutex poisoned")
    }

    /// Number of repositories materialised in `projection_state`.
    ///
    /// Lock-and-release accessor: acquires the projection mutex,
    /// reads `len`, releases. Safe to call from async contexts —
    /// no `MutexGuard` escapes (D-CD-3). Panics on poisoned mutex
    /// to match [`Self::lock_projection`].
    #[cfg(test)]
    pub(crate) fn projection_len(&self) -> usize {
        self.lock_projection().len()
    }

    /// Look up evidence for `key` in `projection_state`, returning an
    /// owned clone.
    ///
    /// Lock-and-release accessor over
    /// [`crate::projection::EvidenceProjection::get`]; the guard does
    /// not escape (D-CD-3). Panics on poisoned mutex.
    pub(crate) fn projection_get(
        &self,
        key: &str,
    ) -> Option<crate::domain::evidence::RepositoryEvidence> {
        self.lock_projection().get(key)
    }

    /// True when `key` is materialised in `projection_state`.
    ///
    /// Lock-and-release accessor; equivalent to
    /// `self.projection_get(key).is_some()` but avoids the clone.
    /// Guard does not escape (D-CD-3); panics on poisoned mutex.
    pub(crate) fn projection_contains(&self, key: &str) -> bool {
        self.lock_projection().get(key).is_some()
    }

    /// Sorted snapshot of all evidence in `projection_state`.
    ///
    /// Lock-and-release wrapper over
    /// [`crate::projection::EvidenceProjection::sorted_snapshot`]; the
    /// guard does not escape (D-CD-3). Panics on poisoned mutex. Cost
    /// is `O(n log n)` per call; see the underlying method for
    /// ordering rationale.
    pub(crate) fn projection_snapshot(&self) -> Vec<crate::domain::evidence::RepositoryEvidence> {
        self.lock_projection().sorted_snapshot()
    }

    /// Test-only accessor for the `runs_by_key` routing index.
    ///
    /// Returns the shared `Arc<Mutex<_>>` handle so integration tests
    /// (`crates/gh-report/tests/bootstrap_replay.rs`) can assert
    /// post-bootstrap population. Not intended for production callers:
    /// production code routes through the Merger task which holds its
    /// own `Arc<Mutex<_>>` clone (see `merger.rs:230-232`).
    #[doc(hidden)]
    pub fn runs_by_key_for_test(&self) -> Arc<Mutex<HashMap<String, AggregateId>>> {
        Arc::clone(&self.runs_by_key)
    }

    /// Test-only accessor for the `repos_by_key` routing index.
    /// See [`Self::runs_by_key_for_test`] for the doctrinal rationale.
    #[doc(hidden)]
    pub fn repos_by_key_for_test(&self) -> Arc<Mutex<HashMap<String, AggregateId>>> {
        Arc::clone(&self.repos_by_key)
    }

    /// Test-only accessor for the `deliveries_by_id` routing index.
    /// See [`Self::runs_by_key_for_test`] for the doctrinal rationale.
    #[doc(hidden)]
    pub fn deliveries_by_id_for_test(&self) -> Arc<Mutex<HashMap<String, AggregateId>>> {
        Arc::clone(&self.deliveries_by_id)
    }

    /// Test-only accessor for the `next_seq` per-aggregate tracker.
    /// See [`Self::runs_by_key_for_test`] for the doctrinal rationale.
    #[doc(hidden)]
    pub fn next_seq_for_test(&self) -> Arc<Mutex<HashMap<AggregateId, NonZeroU64>>> {
        Arc::clone(&self.next_seq)
    }

    /// Test-only accessor for the materialised `projection_state`.
    /// See [`Self::runs_by_key_for_test`] for the doctrinal rationale.
    ///
    /// Integration test `tests/bootstrap_replay.rs::
    /// restart_rehydrates_projection_state` asserts that the
    /// cross-aggregate boot replay (bd `adr-fmt-5rwbu`) populates
    /// this state from every aggregate, not just the
    /// `ORG_GOVERNANCE_AGGREGATE_ID` singleton.
    #[doc(hidden)]
    pub fn projection_state_for_test(&self) -> Arc<Mutex<crate::projection::EvidenceProjection>> {
        Arc::clone(&self.projection_state)
    }
}

/// Build the three `ApplicationService` surfaces over a shared
/// [`Merger`] command channel.
///
/// Post-Track-4.0/5 every service is a thin channel-send wrapper
/// over `merger_tx`; the Merger task is the sole holder of the
/// [`EventStore`] write handles and routing indices. The function
/// returns `(run_service, repo_service, webhook_service)` already
/// wrapped in `Arc` for direct assignment to `AppState` fields.
///
/// [`Merger`]: super::services::merger::Merger
/// [`EventStore`]: cherry_pit_core::EventStore
fn build_services(
    merger_tx: tokio::sync::mpsc::Sender<MergerCommand>,
) -> (Arc<RunService>, Arc<RepoService>, Arc<WebhookService>) {
    let run = Arc::new(RunService::with_merger_tx(merger_tx.clone()));
    let repo = Arc::new(RepoService::with_merger_tx(merger_tx.clone()));
    let webhook = Arc::new(WebhookService::with_merger_tx(merger_tx));
    (run, repo, webhook)
}

/// Per-construction unique tempdir + [`MsgpackFileStore`].
/// Used by test-only constructors ([`AppState::new`],
/// [`AppStateBuilder::build`]) which don't model a real persistence
/// scope but need a live `EventStore` handle for the Merger task.
///
/// The directory is leaked (`TempDir::keep`) so the CHE-0043:R1 flock
/// held by [`MsgpackFileStore`] (acquired lazily on first write)
/// survives for the lifetime of the test; same pollution profile as
/// the previous `noop_events_dir` helper. `/tmp` cleanup is the OS's
/// problem.
#[cfg(test)]
#[expect(clippy::unused_async, reason = "preserves .await callers")]
async fn noop_event_store() -> Arc<MsgpackFileStore<DomainEvent>> {
    let dir = tempfile::tempdir().expect("test tempdir");
    let path = dir.keep();
    Arc::new(MsgpackFileStore::<DomainEvent>::new(path))
}

/// Register the projection handler on the bus using a transient
/// [`InMemoryEventStore`] as the driver substrate.
///
/// Test paths only ([`AppState::new`], [`AppStateBuilder::build`]) —
/// production wires the durable store via
/// [`AppState::snapshot_fast_path_init`] which constructs its own
/// `SharedStore` over the `AppState::event_store` Arc.
///
/// M2.cd — post-cutover the projection is the sole read-model
/// authority (CHE-0048:R2). Every `AppState` constructor must wire
/// the bus → projection handler so that published `RepoEvaluated` /
/// `RepoRemoved` envelopes materialise into `projection_state`.
/// Without this wiring the read-model would stay empty in any path
/// that does not subsequently call
/// [`AppState::snapshot_fast_path_init`] (every test path, plus
/// `webhook-listen`-style entry points).
///
/// The transient store is allocated over a unique tempdir path
/// (`noop_events_dir`) and never written to —
/// `ProjectionDriverExt::apply_one`'s default impl delegates to
/// `Projection::apply` and never invokes `EventStore::append`. The
/// driver lifetime is therefore decoupled from durable persistence;
/// callers that need durable rebuild (`with_stores` →
/// `snapshot_fast_path_init`) replace the projection state and
/// re-register a handler over the durable store at startup.
#[cfg(test)]
fn register_default_projection_handler(
    bus: &InProcessEventBus<DomainEvent>,
    projection_state: &Arc<Mutex<crate::projection::EvidenceProjection>>,
    checkpoint_seq: &Arc<AtomicU64>,
) {
    use crate::app::projection_runtime::{SharedStore, register_projection_handler};
    use cherry_pit_projection::ProjectionDriver;

    let transient_store = Arc::new(InMemoryEventStore::<DomainEvent>::new());
    let driver = Arc::new(
        ProjectionDriver::<crate::projection::EvidenceProjection, _>::new(SharedStore::new(
            transient_store,
        )),
    );
    register_projection_handler(
        bus,
        driver,
        Arc::clone(projection_state),
        Arc::clone(checkpoint_seq),
    );
}

#[cfg(test)]
impl AppState {
    /// Create a new `AppState` (for daemon mode).
    ///
    /// Constructs `BudgetGate` and `RateLimitState` eagerly (always needed).
    /// `GitHubClient` is lazily constructed on the first collection run.
    ///
    /// **No `event_store` or `projection_store`.** This constructor
    /// leaves both `None` — used by test paths that don't need
    /// durable persistence. Daemon construction calls
    /// [`Self::with_stores`] instead.
    ///
    /// # Panics
    ///
    /// Panics if the unique tempdir-based noop event-store directory
    /// cannot acquire the CHE-0043:R1 advisory flock at `open` time.
    /// This is an infrastructure-level failure (disk full, permissions,
    /// no `/tmp`) at startup of a test path; halting is appropriate.
    pub async fn new() -> Arc<Self> {
        let bus = Arc::new(InProcessEventBus::new());
        let runs_by_key = Arc::new(Mutex::new(HashMap::new()));
        let repos_by_key = Arc::new(Mutex::new(HashMap::new()));
        let deliveries_by_id = Arc::new(Mutex::new(HashMap::new()));
        let next_seq = Arc::new(Mutex::new(HashMap::new()));
        let rs = noop_event_store().await;
        let (merger_tx, merger_handle) = Merger::spawn(
            rs,
            Arc::clone(&bus),
            Arc::clone(&runs_by_key),
            Arc::clone(&repos_by_key),
            Arc::clone(&deliveries_by_id),
            Arc::clone(&next_seq),
        );
        let (run_service, repo_service, webhook_service) = build_services(merger_tx);
        let projection_state =
            Arc::new(Mutex::new(crate::projection::EvidenceProjection::default()));
        let projection_checkpoint_seq = Arc::new(AtomicU64::new(0));
        register_default_projection_handler(
            bus.as_ref(),
            &projection_state,
            &projection_checkpoint_seq,
        );
        Arc::new(Self {
            started_at: Timestamp::now(),
            current_run: ArcSwap::from_pointee(None),
            last_completed_run: ArcSwap::from_pointee(None),
            work_queue: Arc::new(WorkQueue::new(crate::config::WORK_QUEUE_CAPACITY)),
            worker_pool_started: tokio::sync::OnceCell::new(),
            event_store: None,
            projection_store: None,
            bus,
            projection_state,
            projection_checkpoint_seq,
            webhook: WebhookState::from_environment(),
            github: GithubState::new(),
            evidence: EvidenceState::new(),
            run_service,
            repo_service,
            webhook_service,
            merger_handle,
            next_seq,
            runs_by_key,
            repos_by_key,
            deliveries_by_id,
            sweep_lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }
}

impl AppState {
    /// Create a new `AppState` wired with both stores.
    ///
    /// Constructs a [`MsgpackFileStore`] over `<events_dir>` (the
    /// CHE-0043:R1 flock on `<events_dir>/.lock` is acquired lazily on
    /// first write; per-aggregate `<id>.msgpack` files materialise on
    /// first append) and constructs the durable
    /// [`FileProjectionStore`] over `<projections_dir>`. This is the
    /// only constructor that wires both durable stores; the daemon
    /// (`crate::app::daemon`) and the `--dump-baseline` branch of the
    /// CLI (`crate::bin::gh-report::main`) are the two production
    /// callers.
    ///
    /// File-layout note:
    ///
    /// - `<store_dir>/events/<org>/<aggregate-id>.msgpack` —
    ///   per-aggregate event log owned by [`MsgpackFileStore`]. The
    ///   singleton [`crate::projection::ORG_GOVERNANCE_AGGREGATE_ID`]
    ///   applies per Tension-2; an additional per-repo aggregate file
    ///   is created on first `RepoEvaluated` for each repo.
    /// - `<store_dir>/projections/<org>/1-evidence.snapshot.msgpack`
    ///   and `…1-evidence.checkpoint.msgpack` — paired snapshot +
    ///   checkpoint per CHE-0048:R1/R2.
    ///
    /// `<store_dir>/events/<org>/.lock` is held for the lifetime of the
    /// returned `AppState` (acquired lazily on first write); a second
    /// daemon process attempting to write to the same directory will
    /// fail at write time per CHE-0043:R1.
    ///
    /// # Errors
    ///
    /// Currently infallible — [`MsgpackFileStore::new`] is synchronous
    /// and infallible; the `Result` shape is retained so callers don't
    /// churn and future fallible-init variants remain a no-API-break.
    ///
    /// # Panics
    ///
    /// Panics if [`FileProjectionStore::new`] fails on `projections_dir`.
    #[expect(clippy::unused_async, reason = "preserves .await callers; brief S2")]
    pub async fn with_stores(
        events_dir: &Path,
        projections_dir: PathBuf,
    ) -> Result<Arc<Self>, std::io::Error> {
        let event_store = Arc::new(MsgpackFileStore::<DomainEvent>::new(events_dir));
        let projection_store = Arc::new(
            FileProjectionStore::<crate::projection::EvidenceProjection>::new(
                projections_dir,
                "evidence",
            ),
        );
        let bus = Arc::new(InProcessEventBus::new());
        let runs_by_key = Arc::new(Mutex::new(HashMap::new()));
        let repos_by_key = Arc::new(Mutex::new(HashMap::new()));
        let deliveries_by_id = Arc::new(Mutex::new(HashMap::new()));
        let next_seq = Arc::new(Mutex::new(HashMap::new()));
        let (merger_tx, merger_handle) = Merger::spawn(
            Arc::clone(&event_store),
            Arc::clone(&bus),
            Arc::clone(&runs_by_key),
            Arc::clone(&repos_by_key),
            Arc::clone(&deliveries_by_id),
            Arc::clone(&next_seq),
        );
        let (run_service, repo_service, webhook_service) = build_services(merger_tx);
        Ok(Arc::new(Self {
            started_at: Timestamp::now(),
            current_run: ArcSwap::from_pointee(None),
            last_completed_run: ArcSwap::from_pointee(None),
            work_queue: Arc::new(WorkQueue::new(crate::config::WORK_QUEUE_CAPACITY)),
            worker_pool_started: tokio::sync::OnceCell::new(),
            event_store: Some(event_store),
            projection_store: Some(projection_store),
            bus,
            projection_state: Arc::new(
                Mutex::new(crate::projection::EvidenceProjection::default()),
            ),
            projection_checkpoint_seq: Arc::new(AtomicU64::new(0)),
            webhook: WebhookState::from_environment(),
            github: GithubState::new(),
            evidence: EvidenceState::new(),
            run_service,
            repo_service,
            webhook_service,
            merger_handle,
            next_seq,
            runs_by_key,
            repos_by_key,
            deliveries_by_id,
            sweep_lock: Arc::new(tokio::sync::Mutex::new(())),
        }))
    }
}

impl AppState {
    /// Boot the projection runtime: replay events past the persisted
    /// checkpoint into [`Self::projection_state`] and register the bus
    /// handler that drives `apply_one` for every published envelope.
    ///
    /// **Call exactly once per process**, after [`Self::with_stores`]
    /// and **before** [`crate::app::collect::warm_start_from_baseline`]
    /// — the snapshot is the source of truth for evidence-projection
    /// state at boot (CHE-0048:R2 + CHE-0051:R5).
    ///
    /// No-op when either `event_store` or `projection_store` is
    /// `None` (test-builder paths) — the projection state remains
    /// `EvidenceProjection::default()` and no handler is registered.
    /// This preserves the existing test surface without forcing every
    /// builder caller through the durable-store path.
    ///
    /// The shared `Arc<EventStoreImpl>` held on `self.event_store` is
    /// the single canonical handle to the event log. The driver wraps
    /// that Arc via
    /// [`crate::app::projection_runtime::SharedStore`]; no separate
    /// directory path is threaded through, and the CHE-0043:R1 advisory
    /// lock acquired by [`MsgpackFileStore`] on first write in `with_stores`
    /// remains held for the lifetime of the `AppState` handle.
    ///
    /// # Errors
    ///
    /// Surfaces [`cherry_pit_projection::ProjectionError`] from
    /// snapshot/checkpoint load, infrastructure errors from event-store
    /// load, or `CorruptData` from envelope-stream validation. On
    /// error, the projection state is left unchanged
    /// (`EvidenceProjection::default()`); the caller decides whether
    /// to abort startup.
    pub async fn snapshot_fast_path_init(
        &self,
    ) -> Result<bool, cherry_pit_projection::ProjectionError> {
        use crate::app::projection_runtime::{SharedStore, register_projection_handler};
        use cherry_pit_projection::ProjectionDriver;

        let (Some(event_store), Some(_projection_store)) =
            (self.event_store.as_ref(), self.projection_store.as_ref())
        else {
            tracing::debug!(
                "snapshot_fast_path_init: no durable stores wired; skipping (test path)"
            );
            return Ok(false);
        };

        let last_applied_sequence = self.bootstrap_replay_state(Arc::clone(event_store)).await?;

        self.projection_checkpoint_seq
            .store(last_applied_sequence, std::sync::atomic::Ordering::Release);

        let driver = Arc::new(
            ProjectionDriver::<crate::projection::EvidenceProjection, _>::new(SharedStore::new(
                Arc::clone(event_store),
            )),
        );
        register_projection_handler(
            self.bus.as_ref(),
            driver,
            Arc::clone(&self.projection_state),
            Arc::clone(&self.projection_checkpoint_seq),
        );

        tracing::info!(
            last_applied_sequence,
            "projection runtime initialised via bootstrap_replay_state (B5'; \
             cpp-r-b-r-c / bd adr-fmt-5rwbu)"
        );
        Ok(true)
    }

    /// Memory-Image bootstrap: rebuild routing indices AND projection
    /// state from the durable event log (Track 7.5; CHE-0054:R5
    /// amended in M3 of `phase2-v2-completion-1779400000`;
    /// projection-fold added in mission `cpp-r-b-r-c` per bd
    /// `adr-fmt-5rwbu`).
    ///
    /// ## What this populates
    ///
    /// | `DomainEvent` variant     | Index populated     | Routing key            |
    /// |---------------------------|---------------------|------------------------|
    /// | `SweepStarted`            | `runs_by_key`       | `batch_id`             |
    /// | `RepoEvaluated`           | `repos_by_key`      | `domain_key`           |
    /// | (all variants)            | `next_seq`          | aggregate's max seq    |
    /// | (all variants)            | `projection_state`  | via `Projection::apply` per envelope |
    ///
    /// ## What this does NOT populate
    ///
    /// - `deliveries_by_id`: the `WebhookReceived` event payload does
    ///   not carry the `delivery_id` (it lives only on the originating
    ///   `RecordDelivery` command). The routing key is not on the
    ///   wire, so eager replay cannot rebuild this index. Per the
    ///   amended CHE-0054:R5, this index remains lazy-populated —
    ///   each restart starts with an empty `deliveries_by_id` and
    ///   subsequent `RecordDelivery` commands accumulate entries via
    ///   the merger's `handle_ingest_webhook`. The `WebhookDelivery`
    ///   aggregate is a one-shot (degenerate) aggregate per
    ///   `webhook_service.rs:35-40`; duplicate-`delivery_id`
    ///   detection is a call-site concern, not an index invariant.
    ///
    /// - The `OrgGovernance` singleton aggregate at
    ///   [`crate::projection::ORG_GOVERNANCE_AGGREGATE_ID`]
    ///   (= `AggregateId(1)`, reserved per CHE-0054:R11 added in M3):
    ///   its state is materialised into `projection_state` by
    ///   `snapshot_fast_path_startup` above; no routing index entry
    ///   needed (it is keyed by the singleton id, not a domain key).
    ///
    /// ## Why this is safe to run on every boot
    ///
    /// `Projection::apply` is idempotent over the same
    /// `EventEnvelope` sequence per CHE-0048:R3, and the routing-key
    /// extraction here is a pure function of the envelope payload —
    /// no derived state is fabricated (CHE-0022:R6).
    ///
    /// ## Errors
    ///
    /// Surfaces `cherry_pit_projection::ProjectionError::Infrastructure`
    /// on `list_aggregates` or `load` failures from the event store.
    async fn bootstrap_replay_state(
        &self,
        event_store: Arc<EventStoreImpl>,
    ) -> Result<u64, cherry_pit_projection::ProjectionError> {
        use cherry_pit_core::EventStore as _;

        let aggregate_ids = event_store.list_aggregates().map_err(|e| {
            cherry_pit_projection::ProjectionError::Infrastructure(
                format!("list_aggregates failed during bootstrap replay: {e}").into(),
            )
        })?;

        let mut global_max_seq: u64 = 0;

        for aggregate_id in aggregate_ids {
            let envelopes = event_store.load(aggregate_id).await.map_err(|e| {
                cherry_pit_projection::ProjectionError::Infrastructure(
                    format!("load({aggregate_id:?}) failed during bootstrap replay: {e}").into(),
                )
            })?;

            {
                use cherry_pit_core::Projection as _;
                let mut projection_guard = self
                    .projection_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for env in &envelopes {
                    projection_guard.apply(env);
                }
            }

            let mut runs = self
                .runs_by_key
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut repos = self
                .repos_by_key
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut next_seq = self
                .next_seq
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            let mut max_seq: Option<NonZeroU64> = None;
            for env in &envelopes {
                let seq = env.sequence();
                max_seq = Some(max_seq.map_or(seq, |m| m.max(seq)));

                #[allow(
                    clippy::match_same_arms,
                    reason = "per-variant rationale comments justify split arms"
                )]
                match env.payload() {
                    DomainEvent::SweepStarted { batch_id, .. } => {
                        runs.entry(batch_id.clone()).or_insert(aggregate_id);
                    }
                    DomainEvent::RepoEvaluated { domain_key, .. }
                    | DomainEvent::RepoRemoved { domain_key, .. } => {
                        repos.entry(domain_key.clone()).or_insert(aggregate_id);
                    }
                    DomainEvent::SweepCompleted { .. }
                    | DomainEvent::SweepFailed { .. }
                    | DomainEvent::SweepProgress { .. }
                    | DomainEvent::PartialEvidenceRendered { .. } => {}
                    DomainEvent::WebhookReceived { .. } => {}
                    DomainEvent::EvidencePublished { .. } => {}
                }
            }

            if let Some(seq) = max_seq {
                next_seq.insert(aggregate_id, seq);
                global_max_seq = global_max_seq.max(seq.get());
            }
        }

        let runs_len = self
            .runs_by_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        let repos_len = self
            .repos_by_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        let agg_len = self
            .next_seq
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        tracing::info!(
            runs_indexed = runs_len,
            repos_indexed = repos_len,
            aggregates_tracked = agg_len,
            last_applied_sequence = global_max_seq,
            "bootstrap replay populated routing indices and projection_state \
             (CHE-0054:R5, cpp-r-b-r-c / bd adr-fmt-5rwbu)"
        );
        Ok(global_max_seq)
    }

    /// Render the current in-memory projection as a JSON-encoded
    /// [`crate::infra::baseline::Baseline`] suitable for stdout dump.
    ///
    /// δ.3c-ii: replaces the pre-pivot `infra::baseline::dump_baseline`
    /// which read `<store>/baseline.msgpack`. Callers
    /// (`--dump-baseline`) must run [`Self::snapshot_fast_path_init`]
    /// first so the projection reflects the event log.
    ///
    /// Held internally so the `lock_projection` `MutexGuard` does not
    /// escape `pub(crate)` visibility. Output shape is byte-equivalent
    /// to the pre-pivot dump (same `Baseline { schema_version,
    /// entries }`, same `serde_json::to_string_pretty` formatter).
    ///
    /// # Errors
    /// Surfaces `serde_json` serialization failure (extremely unlikely
    /// for owned, well-formed `Baseline` data).
    pub fn dump_baseline_json(&self) -> Result<String, serde_json::Error> {
        let repos: Vec<crate::domain::evidence::RepositoryEvidence> = self
            .lock_projection()
            .repositories
            .values()
            .cloned()
            .collect();
        let baseline = crate::infra::baseline::build_baseline(&repos);
        serde_json::to_string_pretty(&baseline)
    }
}

/// Builder for constructing `AppState` with explicit control
/// over cache capacity and webhook secret.
///
/// Consolidates the previous `new_with_cache_capacity`,
/// `new_with_webhook_secret`, and `new_test` constructors into a
/// single fluent API.
///
/// # Example
///
/// ```ignore
/// let state = AppStateBuilder::new()
///     .cache_capacity(10)
///     .webhook_secret("test-secret")
///     .build();
/// ```
#[cfg(test)]
pub struct AppStateBuilder {
    cache_capacity: Option<u64>,
    webhook_secret: Option<secrecy::SecretString>,
}

#[cfg(test)]
impl Default for AppStateBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl AppStateBuilder {
    /// Create a builder with default values.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache_capacity: None,
            webhook_secret: None,
        }
    }

    /// Set the cross-run repo detail cache capacity.
    #[must_use]
    pub fn cache_capacity(mut self, capacity: u64) -> Self {
        self.cache_capacity = Some(capacity);
        self
    }

    /// Set the webhook HMAC secret.
    #[must_use]
    pub fn webhook_secret(mut self, secret: &str) -> Self {
        self.webhook_secret = Some(secrecy::SecretString::from(secret.to_string()));
        self
    }

    /// Build the `Arc<AppState>`.
    ///
    /// # Panics
    ///
    /// Panics if the unique tempdir-based noop event-store directory
    /// cannot acquire the CHE-0043:R1 advisory flock at `open` time.
    /// This is an infrastructure-level failure (disk full, permissions,
    /// no `/tmp`) at builder construction in a test path; halting is
    /// appropriate.
    pub async fn build(self) -> Arc<AppState> {
        let github = match self.cache_capacity {
            Some(cap) => GithubState::with_cache_capacity(cap),
            None => GithubState::new(),
        };
        let webhook = WebhookState::with_secret(self.webhook_secret);
        let bus = Arc::new(InProcessEventBus::new());
        let runs_by_key = Arc::new(Mutex::new(HashMap::new()));
        let repos_by_key = Arc::new(Mutex::new(HashMap::new()));
        let deliveries_by_id = Arc::new(Mutex::new(HashMap::new()));
        let next_seq = Arc::new(Mutex::new(HashMap::new()));
        let rs = noop_event_store().await;
        let (merger_tx, merger_handle) = Merger::spawn(
            rs,
            Arc::clone(&bus),
            Arc::clone(&runs_by_key),
            Arc::clone(&repos_by_key),
            Arc::clone(&deliveries_by_id),
            Arc::clone(&next_seq),
        );
        let (run_service, repo_service, webhook_service) = build_services(merger_tx);
        let projection_state =
            Arc::new(Mutex::new(crate::projection::EvidenceProjection::default()));
        let projection_checkpoint_seq = Arc::new(AtomicU64::new(0));
        register_default_projection_handler(
            bus.as_ref(),
            &projection_state,
            &projection_checkpoint_seq,
        );

        Arc::new(AppState {
            started_at: Timestamp::now(),
            current_run: ArcSwap::from_pointee(None),
            last_completed_run: ArcSwap::from_pointee(None),
            work_queue: Arc::new(WorkQueue::new(crate::config::WORK_QUEUE_CAPACITY)),
            worker_pool_started: tokio::sync::OnceCell::new(),
            event_store: None,
            projection_store: None,
            bus,
            projection_state,
            projection_checkpoint_seq,
            webhook,
            github,
            evidence: EvidenceState::new(),
            run_service,
            repo_service,
            webhook_service,
            merger_handle,
            next_seq,
            runs_by_key,
            repos_by_key,
            deliveries_by_id,
            sweep_lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }
}

/// Legacy convenience constructors (delegate to builder).
#[cfg(test)]
impl AppState {
    /// Create an `AppState` with a custom cache capacity (for testing).
    pub async fn new_with_cache_capacity(capacity: u64) -> Arc<Self> {
        AppStateBuilder::new()
            .cache_capacity(capacity)
            .build()
            .await
    }

    /// Create an `AppState` with a known webhook secret (for testing).
    pub async fn new_with_webhook_secret(secret: &str) -> Arc<Self> {
        AppStateBuilder::new().webhook_secret(secret).build().await
    }
}

impl AppState {
    /// Ensure the long-lived worker pool and delivery task are running.
    ///
    /// Idempotent: only the first call spawns tasks. Subsequent calls return
    /// immediately. Must be called after `github().client` is initialized
    /// (i.e., after `prepare_collection()` succeeds).
    ///
    /// Returns `true` if the pool was started by this call, `false` if
    /// already running.
    pub(crate) async fn ensure_worker_pool(self: &Arc<Self>) -> bool {
        let state = Arc::clone(self);
        let mut started_now = false;

        self.worker_pool_started
            .get_or_init(|| async {
                started_now = true;

                let client = state
                    .github()
                    .client
                    .get()
                    .expect("ensure_worker_pool called before github_client initialized")
                    .clone();

                let evaluator =
                    Arc::new(crate::app::collect::LiveEvaluator::with_shared_org_summary(
                        client,
                        Arc::clone(&state.evidence().org_summary),
                    ));

                let queue = Arc::clone(&state.work_queue);
                let budget = Arc::clone(&state.github().budget_gate);
                let rate_limit = Arc::clone(&state.github().rate_limit_state);

                let (outcome_tx, outcome_rx) = tokio::sync::mpsc::channel::<
                    crate::app::worker_pool::JobOutcome<
                        crate::domain::evidence::RepositoryEvidence,
                    >,
                >(1024);

                let delivery_state = Arc::clone(&state);
                let delivery_handle = tokio::spawn(crate::app::daemon::delivery_loop(
                    outcome_rx,
                    delivery_state,
                ));

                let pool_handle = tokio::spawn(async move {
                    crate::app::worker_pool::run_worker_pool(
                        queue,
                        evaluator,
                        budget,
                        rate_limit,
                        crate::app::worker_pool::WorkerPoolConfig::default(),
                        outcome_tx,
                    )
                    .await;
                });

                tracing::info!("worker pool started");
                std::sync::Mutex::new(Some((pool_handle, delivery_handle)))
            })
            .await;

        started_now
    }

    /// Drain the worker pool: `take()` both `JoinHandle`s from the
    /// `OnceCell` (if any were ever started) and `await` each with an
    /// individual timeout. Returns the pair of `(pool_drained,
    /// delivery_drained)` booleans where `true` means the task exited
    /// cleanly within the timeout. Caller logs the outcome.
    ///
    /// Idempotent: calling twice returns `(false, false)` the second
    /// time because the handles were already taken.
    pub(crate) async fn drain_worker_pool(
        &self,
        per_handle_timeout: std::time::Duration,
    ) -> (bool, bool) {
        let Some(slot) = self.worker_pool_started.get() else {
            return (false, false);
        };
        let taken = {
            let mut guard = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.take()
        };
        let Some((pool_handle, delivery_handle)) = taken else {
            return (false, false);
        };
        let pool_ok = tokio::time::timeout(per_handle_timeout, pool_handle)
            .await
            .is_ok();
        let delivery_ok = tokio::time::timeout(per_handle_timeout, delivery_handle)
            .await
            .is_ok();
        (pool_ok, delivery_ok)
    }
}

impl AppState {
    /// Build the JSON payload for the `/api/v1/status` endpoint.
    ///
    /// Returns current and last completed run metadata plus uptime.
    /// Registered as an extra route in [`crate::server::status_router`],
    /// not as a built-in route of the generic server module.
    pub(crate) fn status_payload(&self) -> serde_json::Value {
        let current = self.current_run.load();
        let last = self.last_completed_run.load();
        let uptime_duration = Timestamp::now().duration_since(self.started_at);
        let uptime = u64::try_from(uptime_duration.as_secs().max(0)).unwrap_or(0);
        serde_json::json!({
            "current_run": current.as_ref(),
            "last_completed_run": last.as_ref(),
            "uptime_secs": uptime,
        })
    }
}

impl crate::infra::server::state::ServerState for AppState {
    fn html_cache(&self) -> &ArcSwap<Option<HashMap<String, CachedPage>>> {
        &self.evidence().html_cache
    }

    fn ws_broadcast(&self) -> &tokio::sync::broadcast::Sender<PageUpdateEvent> {
        &self.evidence().ws_broadcast
    }

    fn is_ready(&self) -> bool {
        self.last_completed_run.load().is_some() || self.evidence().html_cache.load().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::cache::CachedRepoDetail;

    #[tokio::test]
    async fn cache_respects_max_capacity() {
        let state = AppState::new_with_cache_capacity(3).await;
        let cache = &state.github().repo_detail_cache;

        for i in 0..4 {
            cache
                .insert(
                    format!("repo-{i}"),
                    CachedRepoDetail {
                        default_branch: "main".into(),
                        updated_at: None,
                        security_and_analysis: None,
                        is_security_policy_enabled: None,
                        fetched_at: Timestamp::now(),
                        etag: None,
                    },
                )
                .await;
        }
        cache.run_pending_tasks().await;

        assert!(
            cache.entry_count() <= 3,
            "cache should not exceed max_capacity; got {}",
            cache.entry_count()
        );
    }

    #[tokio::test]
    async fn cache_stores_and_retrieves_details() {
        let state = AppState::new().await;
        let cache = &state.github().repo_detail_cache;

        let detail = CachedRepoDetail {
            default_branch: "develop".into(),
            updated_at: Some("2026-04-10T00:00:00Z".into()),
            security_and_analysis: None,
            is_security_policy_enabled: None,
            fetched_at: Timestamp::now(),
            etag: None,
        };
        cache.insert("my-repo".into(), detail).await;

        let retrieved = cache.get("my-repo").await.expect("should exist");
        assert_eq!(retrieved.default_branch, "develop");
        assert_eq!(
            retrieved.updated_at.as_deref(),
            Some("2026-04-10T00:00:00Z")
        );
    }

    #[tokio::test]
    async fn cache_iter_round_trip() {
        let state = AppState::new_with_cache_capacity(100).await;
        let cache = &state.github().repo_detail_cache;

        for i in 0..3 {
            cache
                .insert(
                    format!("repo-{i}"),
                    CachedRepoDetail {
                        default_branch: format!("branch-{i}"),
                        updated_at: Some(format!("2026-04-0{i}T00:00:00Z")),
                        security_and_analysis: None,
                        is_security_policy_enabled: None,
                        fetched_at: Timestamp::now(),
                        etag: None,
                    },
                )
                .await;
        }
        cache.run_pending_tasks().await;

        let exported: Vec<_> = cache
            .iter()
            .map(|(k, v)| ((*k).clone(), v.clone()))
            .collect();
        assert_eq!(exported.len(), 3);

        let new_cache = crate::app::github_infra::build_cache(100);
        for (k, v) in exported {
            new_cache.insert(k, v).await;
        }
        new_cache.run_pending_tasks().await;

        assert_eq!(new_cache.entry_count(), 3);
        let r1 = new_cache.get("repo-1").await.expect("should exist");
        assert_eq!(r1.default_branch, "branch-1");
    }

    #[tokio::test]
    async fn html_cache_starts_empty() {
        let state = AppState::new().await;
        assert!(state.evidence().html_cache.load().is_none());
    }

    #[tokio::test]
    async fn builder_default_produces_valid_state() {
        let state = AppStateBuilder::new().build().await;
        assert!(state.webhook().secret.is_none());
        assert!(state.evidence().html_cache.load().is_none());
    }

    #[tokio::test]
    async fn builder_with_webhook_secret() {
        let state = AppStateBuilder::new()
            .webhook_secret("test-secret")
            .build()
            .await;
        assert!(state.webhook().secret.is_some());
    }

    #[tokio::test]
    async fn builder_with_cache_capacity() {
        let state = AppStateBuilder::new().cache_capacity(5).build().await;
        let cache = &state.github().repo_detail_cache;
        for i in 0..6 {
            cache
                .insert(
                    format!("repo-{i}"),
                    CachedRepoDetail {
                        default_branch: "main".into(),
                        updated_at: None,
                        security_and_analysis: None,
                        is_security_policy_enabled: None,
                        fetched_at: Timestamp::now(),
                        etag: None,
                    },
                )
                .await;
        }
        cache.run_pending_tasks().await;
        assert!(cache.entry_count() <= 5);
    }

    #[tokio::test]
    async fn sub_aggregate_accessors_return_correct_references() {
        let state = AppStateBuilder::new().webhook_secret("s").build().await;
        let _wh: &WebhookState = state.webhook();
        let _gh: &GithubState = state.github();
        let _ev: &EvidenceState = state.evidence();
    }

    #[tokio::test]
    async fn builder_combined_cache_and_secret() {
        let state = AppStateBuilder::new()
            .cache_capacity(7)
            .webhook_secret("combo-secret")
            .build()
            .await;
        assert!(state.webhook().secret.is_some());
        let cache = &state.github().repo_detail_cache;
        for i in 0..8 {
            cache
                .insert(
                    format!("repo-{i}"),
                    CachedRepoDetail {
                        default_branch: "main".into(),
                        updated_at: None,
                        security_and_analysis: None,
                        is_security_policy_enabled: None,
                        fetched_at: Timestamp::now(),
                        etag: None,
                    },
                )
                .await;
        }
        cache.run_pending_tasks().await;
        assert!(cache.entry_count() <= 7);
    }

    #[tokio::test]
    async fn is_ready_false_when_no_run_and_no_cache() {
        use crate::infra::server::state::ServerState;
        let state = AppStateBuilder::new().build().await;
        assert!(
            !state.is_ready(),
            "should not be ready with no run and no cache"
        );
    }

    #[tokio::test]
    async fn is_ready_true_when_html_cache_populated() {
        use crate::infra::server::state::ServerState;
        let state = AppStateBuilder::new().build().await;
        let mut pages = HashMap::new();
        pages.insert(
            "index.html".to_string(),
            CachedPage::new("index.html", b"<html>test</html>".to_vec()),
        );
        state.evidence().html_cache.store(Arc::new(Some(pages)));
        assert!(state.is_ready(), "should be ready when html_cache is Some");
    }

    #[tokio::test]
    async fn sweep_lock_serialises_concurrent_acquirers() {
        use std::time::{Duration, Instant};

        fn empty_org_summary() -> crate::domain::metrics::OrgAlertSummary {
            crate::domain::metrics::OrgAlertSummary {
                collection_status: crate::domain::status::CollectionStatus::Success,
                collection_reason: None,
                per_repo: HashMap::new(),
                open_secret_alert_age_buckets: crate::config::empty_age_buckets(),
                total_open_secret_alerts: 0,
                oldest_open_secret_alert_created_at: None,
                newest_open_secret_alert_created_at: None,
            }
        }

        let state = AppStateBuilder::new().build().await;
        let sentinel_a = Arc::new(empty_org_summary());
        let sentinel_b = Arc::new(empty_org_summary());

        let hold_for = Duration::from_millis(120);
        let barrier = Arc::new(tokio::sync::Barrier::new(2));

        let state_a = Arc::clone(&state);
        let barrier_a = Arc::clone(&barrier);
        let summary_for_task_a = Arc::clone(&sentinel_a);
        let task_a = tokio::spawn(async move {
            let lock = Arc::clone(&state_a.sweep_lock);
            barrier_a.wait().await;
            let _guard = lock.lock_owned().await;
            let acquired_at = Instant::now();
            state_a
                .evidence()
                .org_summary
                .store(Arc::new(Some(Arc::clone(&summary_for_task_a))));
            tokio::time::sleep(hold_for).await;
            let guard_after_hold = state_a.evidence().org_summary.load_full();
            let observed_after_hold = (*guard_after_hold)
                .as_ref()
                .map(Arc::clone)
                .expect("set above");
            let released_at = Instant::now();
            (acquired_at, released_at, observed_after_hold)
        });

        let state_b = Arc::clone(&state);
        let barrier_b = Arc::clone(&barrier);
        let summary_for_task_b = Arc::clone(&sentinel_b);
        let task_b = tokio::spawn(async move {
            let lock = Arc::clone(&state_b.sweep_lock);
            barrier_b.wait().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
            let _guard = lock.lock_owned().await;
            let acquired_at = Instant::now();
            let guard_at_acquire = state_b.evidence().org_summary.load_full();
            let observed_at_acquire = (*guard_at_acquire)
                .as_ref()
                .map(Arc::clone)
                .expect("A set first");
            state_b
                .evidence()
                .org_summary
                .store(Arc::new(Some(Arc::clone(&summary_for_task_b))));
            (acquired_at, observed_at_acquire)
        });

        let (a_acquired, a_released, a_observed) = task_a.await.unwrap();
        let (b_acquired, b_observed_at_acquire) = task_b.await.unwrap();

        assert!(
            b_acquired >= a_released,
            "B must acquire only after A released; a_released={a_released:?}, b_acquired={b_acquired:?}"
        );
        assert!(
            Arc::ptr_eq(&a_observed, &sentinel_a),
            "A's own write must be visible to A across its hold (no concurrent overwrite)"
        );
        assert!(
            Arc::ptr_eq(&b_observed_at_acquire, &sentinel_a),
            "B must observe A's final state at acquire (B did not race A); \
             this proves the lock serialised the critical sections"
        );
        let _ = (a_acquired, sentinel_b);
    }
}
