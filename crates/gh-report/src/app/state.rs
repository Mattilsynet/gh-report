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
use cherry_pit_core::AggregateId;
use cherry_pit_gateway::MsgpackFileStore;
use cherry_pit_projection::FileProjectionStore;
use jiff::Timestamp;

// Re-export server-state types referenced via this module.
pub use crate::infra::server::state::{CachedPage, PageUpdateEvent};

// Re-export DomainEvent for convenience.
pub use crate::domain::events::DomainEvent;

// Re-export sub-aggregates for convenience.
pub use crate::app::evidence_service::EvidenceState;
pub use crate::app::github_infra::GithubState;
pub use crate::app::services::repo_service::RepoService;
pub use crate::app::services::run_service::RunService;
pub use crate::app::services::webhook_service::WebhookService;
pub use crate::app::webhook_context::WebhookState;

/// Concrete monomorphisation of [`RunService`] used throughout
/// `gh-report` (CHE-0005:R1: bind generic ports at the composition
/// root). [`MsgpackFileStore`] supplies the durable per-aggregate
/// event store; [`InProcessEventBus`] supplies synchronous in-process
/// fan-out.
pub type RunServiceConcrete =
    RunService<MsgpackFileStore<DomainEvent>, InProcessEventBus<DomainEvent>>;
/// Concrete monomorphisation of [`RepoService`]. See [`RunServiceConcrete`].
pub type RepoServiceConcrete =
    RepoService<MsgpackFileStore<DomainEvent>, InProcessEventBus<DomainEvent>>;
/// Concrete monomorphisation of [`WebhookService`]. See [`RunServiceConcrete`].
pub type WebhookServiceConcrete =
    WebhookService<MsgpackFileStore<DomainEvent>, InProcessEventBus<DomainEvent>>;

use crate::app::collect::JobContext;
use crate::app::work_queue::WorkQueue;
use crate::domain::run::RunMetadata;

// ── Pre-computed static assets ──────────────────────────────────────

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
pub struct AppState {
    // ── Cross-cutting fields ────────────────────────────────────
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
    /// credential resolution. Tuple: (`worker_pool_handle`, `delivery_task_handle`).
    pub(crate) worker_pool_started:
        tokio::sync::OnceCell<(tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()>)>,

    /// Durable per-aggregate event store.
    ///
    /// Wired at WU-6 v2 B3' (charter `wu6v2-charter-1778415390`,
    /// AdjustIntent option 2). Constructed at
    /// `<store_dir>/events/<org>/`; the singleton aggregate id is
    /// [`crate::projection::ORG_GOVERNANCE_AGGREGATE_ID`] (Tension-2
    /// single-aggregate lock).
    ///
    /// **B3' is additive wiring only**: this handle is constructed and
    /// held but is **not yet exercised** by collectors. B7' lands the
    /// collector rewrite that calls `event_store.append(...)` then
    /// `bus.publish(...)` per BC-v2-1 / CHE-0024:R1
    /// persist-then-publish ordering. Until B7', the field exists for
    /// B5' driver wiring (snapshot-fast-path replay) and to surface a
    /// non-zero `cargo tree` dep on cherry-pit-gateway.
    ///
    /// `None` only in test-builder paths that don't supply a
    /// `store_dir`. Daemon construction always supplies it.
    pub event_store: Option<Arc<MsgpackFileStore<DomainEvent>>>,

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
    /// constructors. Populated by
    /// [`crate::app::projection_runtime::snapshot_fast_path_startup`]
    /// which the daemon calls **after** `with_stores` and **before**
    /// warm-start (CHE-0048:R2 — the snapshot is the source of truth
    /// for state at boot).
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

    // ── Sub-aggregates ──────────────────────────────────────────
    /// Webhook ingestion concerns (secret, replay, debounce).
    webhook: WebhookState,
    /// GitHub API infrastructure (budget, rate limit, client, cache).
    github: GithubState,
    /// Evidence data store and publication infrastructure.
    evidence: EvidenceState,

    // ── ApplicationServices (CHE-0054:R4, B7'b wiring) ──────────
    /// `RunService` — load → handle → append → publish for the
    /// [`Run`](crate::domain::aggregates::run::Run) aggregate.
    /// Skeleton in B7'a; constructor + types wired in B7'b-1;
    /// method bodies wired in B7'b-2..3; production sites
    /// migrate in B7'c (CHE-0054:R4).
    pub run_service: Arc<RunServiceConcrete>,
    /// `RepoService` — same triad for the
    /// [`Repo`](crate::domain::aggregates::repo::Repo) aggregate.
    pub repo_service: Arc<RepoServiceConcrete>,
    /// `WebhookService` — same triad for the
    /// [`WebhookDelivery`](crate::domain::aggregates::webhook::WebhookDelivery)
    /// aggregate.
    pub webhook_service: Arc<WebhookServiceConcrete>,

    /// Shared per-aggregate last-applied-sequence tracker
    /// (CHE-0054:R6 / CHE-0042:R3). Populated by service `append`
    /// paths in B7'b-2..6 to support caller-tracked optimistic
    /// concurrency control.
    #[allow(dead_code, reason = "B7'b-2..6 populates and reads this tracker")]
    pub(crate) sequence_tracker: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,

    // ── Domain-key → AggregateId indices (CHE-0054:R5) ──────────
    //
    // Placeholder shape: `Mutex<HashMap<String, AggregateId>>`.
    //
    // **B7'b will replace** with `DashMap<DomainKey, AggregateId>`
    // where `DomainKey` is a typed newtype per aggregate (Run keyed
    // by `batch_id`, Repo by `(org, repo)`, WebhookDelivery by
    // `delivery_id`). String-keyed std-only placeholder is used
    // here to:
    //   1. avoid adding a new `dashmap` dep before there is an
    //      actual reader/writer (no churn on `cargo tree`),
    //   2. defer the typed-key design to B7'b where call-sites
    //      exist to constrain it,
    //   3. compile the AppState shape that B7'a-6 requires.
    //
    // Indices are constructed empty and never populated in B7'a;
    // the load path that consults them lands in B7'b.
    #[allow(dead_code, reason = "B7'b populates and reads these indices")]
    pub(crate) run_index: Arc<Mutex<HashMap<String, AggregateId>>>,
    #[allow(dead_code, reason = "B7'b populates and reads these indices")]
    pub(crate) repo_index: Arc<Mutex<HashMap<String, AggregateId>>>,
    #[allow(dead_code, reason = "B7'b populates and reads these indices")]
    pub(crate) delivery_index: Arc<Mutex<HashMap<String, AggregateId>>>,
}

// ── Sub-aggregate accessors ─────────────────────────────────────────

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
}

// ── Service-construction helper ─────────────────────────────────────

/// Build the three ApplicationServices from a shared bus, a per-service
/// `MsgpackFileStore<DomainEvent>` (typically all pointing at the same
/// `events_dir`; `MsgpackFileStore` is a stateless handle), the three
/// domain-key indices, and the shared sequence tracker.
///
/// Returns `(run_service, repo_service, webhook_service)` already
/// wrapped in `Arc` for direct assignment to `AppState` fields.
#[allow(
    clippy::too_many_arguments,
    clippy::type_complexity,
    reason = "wiring helper threads named handles into the three services; factoring would obscure the 1:1 mapping with AppState fields"
)]
fn build_services(
    run_store: Arc<MsgpackFileStore<DomainEvent>>,
    repo_store: Arc<MsgpackFileStore<DomainEvent>>,
    webhook_store: Arc<MsgpackFileStore<DomainEvent>>,
    bus: Arc<InProcessEventBus<DomainEvent>>,
    run_index: Arc<Mutex<HashMap<String, AggregateId>>>,
    repo_index: Arc<Mutex<HashMap<String, AggregateId>>>,
    delivery_index: Arc<Mutex<HashMap<String, AggregateId>>>,
    sequence_tracker: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
) -> (
    Arc<RunServiceConcrete>,
    Arc<RepoServiceConcrete>,
    Arc<WebhookServiceConcrete>,
) {
    let run = Arc::new(RunService::with_stores(
        run_store,
        Arc::clone(&bus),
        run_index,
        Arc::clone(&sequence_tracker),
    ));
    let repo = Arc::new(RepoService::with_stores(
        repo_store,
        Arc::clone(&bus),
        repo_index,
        Arc::clone(&sequence_tracker),
    ));
    let webhook = Arc::new(WebhookService::with_stores(
        webhook_store,
        bus,
        delivery_index,
        sequence_tracker,
    ));
    (run, repo, webhook)
}

/// Construct three `Arc`-cloned handles to one shared
/// `MsgpackFileStore<DomainEvent>`.
///
/// Per CHE-0054 §"Open γ" the three services hold concrete
/// per-aggregate stores; per CHE-0036:R1 each aggregate writes its own
/// `<aggregate_id>.msgpack` file, so file-level isolation is preserved
/// regardless of how many handles point at the directory.
///
/// The handle is shared (not three independent constructions) because
/// `MsgpackFileStore` acquires a per-handle advisory `flock` on first
/// write; three independent handles to the same `events_dir` would race
/// for the lock and the second writer would panic with `StoreLocked`.
/// One shared handle holds a single `flock` lifetime-bounded to the
/// `AppState`, avoiding that contention.
#[allow(
    clippy::type_complexity,
    reason = "tuple-of-three mirrors the three per-aggregate services it feeds"
)]
fn build_three_stores(
    events_dir: &std::path::Path,
) -> (
    Arc<MsgpackFileStore<DomainEvent>>,
    Arc<MsgpackFileStore<DomainEvent>>,
    Arc<MsgpackFileStore<DomainEvent>>,
) {
    let store = Arc::new(MsgpackFileStore::<DomainEvent>::new(events_dir));
    (Arc::clone(&store), Arc::clone(&store), store)
}

/// Per-construction unique placeholder path. Used by `AppState::new()`
/// (test/no-store path) and other no-store constructors. Files leak into
/// OS temp dir; volume is bounded; OS reclaims.
///
/// History: previously a single shared path. B7'c migrations route
/// tests-of-production-paths through `MsgpackFileStore.create`, which
/// acquires a per-path lock; under parallel test execution that panicked
/// with `StoreLocked`. A unique suffix per call eliminates the contention.
fn noop_events_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("gh-report-noop-events-{}", uuid::Uuid::new_v4()))
}

/// Register the projection handler on the bus using a transient
/// (apply-only, never-written) `MsgpackFileStore`.
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
fn register_default_projection_handler(
    bus: &InProcessEventBus<DomainEvent>,
    projection_state: &Arc<Mutex<crate::projection::EvidenceProjection>>,
    checkpoint_seq: &Arc<AtomicU64>,
) {
    use crate::app::projection_runtime::register_projection_handler;
    use cherry_pit_projection::ProjectionDriver;

    let transient_store = MsgpackFileStore::<DomainEvent>::new(noop_events_dir());
    let driver = Arc::new(
        ProjectionDriver::<crate::projection::EvidenceProjection, _>::new(transient_store),
    );
    register_projection_handler(
        bus,
        driver,
        Arc::clone(projection_state),
        Arc::clone(checkpoint_seq),
    );
}

// ── Constructors ────────────────────────────────────────────────────

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
    #[must_use]
    pub fn new() -> Arc<Self> {
        let bus = Arc::new(InProcessEventBus::new());
        let run_index = Arc::new(Mutex::new(HashMap::new()));
        let repo_index = Arc::new(Mutex::new(HashMap::new()));
        let delivery_index = Arc::new(Mutex::new(HashMap::new()));
        let sequence_tracker = Arc::new(Mutex::new(HashMap::new()));
        let noop_dir = noop_events_dir();
        let (rs, ps, ws) = build_three_stores(&noop_dir);
        let (run_service, repo_service, webhook_service) = build_services(
            rs,
            ps,
            ws,
            Arc::clone(&bus),
            Arc::clone(&run_index),
            Arc::clone(&repo_index),
            Arc::clone(&delivery_index),
            Arc::clone(&sequence_tracker),
        );
        let projection_state =
            Arc::new(Mutex::new(crate::projection::EvidenceProjection::default()));
        let projection_checkpoint_seq = Arc::new(AtomicU64::new(0));
        // M2.cd: wire bus → projection so published envelopes materialise
        // into the read-model (CHE-0048:R2 sole-writer is `apply`).
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
            sequence_tracker,
            run_index,
            repo_index,
            delivery_index,
        })
    }

    /// Create a new `AppState` wired with both durable stores:
    /// a [`MsgpackFileStore`] event store at `<store_dir>/events/<org>/`
    /// and a [`FileProjectionStore`] projection snapshot store at
    /// `<store_dir>/projections/<org>/`.
    ///
    /// WU-6 v2 B3' + B4' composition root (charter
    /// `wu6v2-charter-1778415390`). Both stores are constructed
    /// lazily — the directories are not touched until the first
    /// write — so neither path needs to exist.
    ///
    /// Per the AdjustIntent option-2 file layout the per-org subtrees
    /// are siblings (BC-v2-13: events/ and projections/ disjoint):
    ///
    /// - `<store_dir>/events/<org>/1.msgpack` — single
    ///   [`MsgpackFileStore`] file (the singleton
    ///   [`crate::projection::ORG_GOVERNANCE_AGGREGATE_ID`] per
    ///   Tension-2). cherry-pit-gateway hard-codes the
    ///   `{aggregate_id}.msgpack` filename per CHE-0036:R1.
    /// - `<store_dir>/projections/<org>/1-evidence.snapshot.msgpack`
    ///   and `…1-evidence.checkpoint.msgpack` — paired snapshot +
    ///   checkpoint per CHE-0048:R1/R2.
    #[must_use]
    pub fn with_stores(events_dir: &Path, projections_dir: PathBuf) -> Arc<Self> {
        let event_store = Arc::new(MsgpackFileStore::<DomainEvent>::new(events_dir));
        let projection_store = Arc::new(
            FileProjectionStore::<crate::projection::EvidenceProjection>::new(
                projections_dir,
                "evidence",
            ),
        );
        let bus = Arc::new(InProcessEventBus::new());
        let run_index = Arc::new(Mutex::new(HashMap::new()));
        let repo_index = Arc::new(Mutex::new(HashMap::new()));
        let delivery_index = Arc::new(Mutex::new(HashMap::new()));
        let sequence_tracker = Arc::new(Mutex::new(HashMap::new()));
        let (rs, ps, ws) = build_three_stores(events_dir);
        let (run_service, repo_service, webhook_service) = build_services(
            rs,
            ps,
            ws,
            Arc::clone(&bus),
            Arc::clone(&run_index),
            Arc::clone(&repo_index),
            Arc::clone(&delivery_index),
            Arc::clone(&sequence_tracker),
        );
        Arc::new(Self {
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
            sequence_tracker,
            run_index,
            repo_index,
            delivery_index,
        })
    }
}

// ── Snapshot-fast-path projection runtime (B5') ─────────────────────

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
    /// `event_store_dir` is passed explicitly because
    /// [`MsgpackFileStore`] does not expose its underlying directory
    /// (no `dir()` accessor in cherry-pit-gateway as of this writing;
    /// see I2 follow-up bead). The caller already owns the
    /// `PathBuf` it used to construct the AppState-held event store
    /// (see [`crate::app::daemon::run`]) so threading it through is
    /// the most reversible interpretation.
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
        event_store_dir: &std::path::Path,
    ) -> Result<bool, cherry_pit_projection::ProjectionError> {
        use crate::app::projection_runtime::{
            register_projection_handler, snapshot_fast_path_startup,
        };
        use crate::projection::ORG_GOVERNANCE_AGGREGATE_ID;
        use cherry_pit_projection::ProjectionDriver;

        let (Some(event_store), Some(projection_store)) =
            (self.event_store.as_ref(), self.projection_store.as_ref())
        else {
            tracing::debug!(
                "snapshot_fast_path_init: no durable stores wired; skipping (test path)"
            );
            return Ok(false);
        };

        let startup = snapshot_fast_path_startup(
            event_store.as_ref(),
            event_store_dir,
            projection_store.as_ref(),
            ORG_GOVERNANCE_AGGREGATE_ID,
        )
        .await?;

        // Replace the projection state with the materialised one and
        // initialise the checkpoint atomic. No bus handler is yet
        // registered, so no concurrent writer can race this.
        {
            let mut guard = self
                .projection_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard = startup.projection;
        }
        self.projection_checkpoint_seq.store(
            startup.last_applied_sequence,
            std::sync::atomic::Ordering::Release,
        );

        // Register the bus handler that keeps the in-memory state
        // current as new envelopes are published. The driver holds a
        // transient MsgpackFileStore (apply_one never writes — see
        // ProjectionDriverExt::apply_one default impl).
        let transient_store = MsgpackFileStore::<DomainEvent>::new(event_store_dir);
        let driver = Arc::new(
            ProjectionDriver::<crate::projection::EvidenceProjection, _>::new(transient_store),
        );
        register_projection_handler(
            self.bus.as_ref(),
            driver,
            Arc::clone(&self.projection_state),
            Arc::clone(&self.projection_checkpoint_seq),
        );

        tracing::info!(
            used_snapshot_fast_path = startup.used_snapshot_fast_path,
            last_applied_sequence = startup.last_applied_sequence,
            "projection runtime initialised (B5')"
        );
        Ok(true)
    }
}

// ── Test builder ────────────────────────────────────────────────────

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
pub struct AppStateBuilder {
    cache_capacity: Option<u64>,
    webhook_secret: Option<secrecy::SecretString>,
}

impl Default for AppStateBuilder {
    fn default() -> Self {
        Self::new()
    }
}

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
    #[must_use]
    pub fn build(self) -> Arc<AppState> {
        let github = match self.cache_capacity {
            Some(cap) => GithubState::with_cache_capacity(cap),
            None => GithubState::new(),
        };
        let webhook = WebhookState::with_secret(self.webhook_secret);
        let bus = Arc::new(InProcessEventBus::new());
        let run_index = Arc::new(Mutex::new(HashMap::new()));
        let repo_index = Arc::new(Mutex::new(HashMap::new()));
        let delivery_index = Arc::new(Mutex::new(HashMap::new()));
        let sequence_tracker = Arc::new(Mutex::new(HashMap::new()));
        let noop_dir = noop_events_dir();
        let (rs, ps, ws) = build_three_stores(&noop_dir);
        let (run_service, repo_service, webhook_service) = build_services(
            rs,
            ps,
            ws,
            Arc::clone(&bus),
            Arc::clone(&run_index),
            Arc::clone(&repo_index),
            Arc::clone(&delivery_index),
            Arc::clone(&sequence_tracker),
        );
        let projection_state =
            Arc::new(Mutex::new(crate::projection::EvidenceProjection::default()));
        let projection_checkpoint_seq = Arc::new(AtomicU64::new(0));
        // M2.cd: wire bus → projection so published envelopes materialise
        // into the read-model (CHE-0048:R2 sole-writer is `apply`).
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
            sequence_tracker,
            run_index,
            repo_index,
            delivery_index,
        })
    }
}

/// Legacy convenience constructors (delegate to builder).
impl AppState {
    /// Create an `AppState` with a custom cache capacity (for testing).
    #[must_use]
    pub fn new_with_cache_capacity(capacity: u64) -> Arc<Self> {
        AppStateBuilder::new().cache_capacity(capacity).build()
    }

    /// Create an `AppState` with a known webhook secret (for testing).
    #[must_use]
    pub fn new_with_webhook_secret(secret: &str) -> Arc<Self> {
        AppStateBuilder::new().webhook_secret(secret).build()
    }
}

// ── Worker pool lifecycle ───────────────────────────────────────────

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

                // Spawn delivery task.
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
                (pool_handle, delivery_handle)
            })
            .await;

        started_now
    }
}

// ── Status endpoint payload ────────────────────────────────────────

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

// ── ServerState implementation ──────────────────────────────────────

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
        let state = AppState::new_with_cache_capacity(3);
        let cache = &state.github().repo_detail_cache;

        // Insert 4 entries into a cache with capacity 3.
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
        // Flush pending tasks so eviction is applied.
        cache.run_pending_tasks().await;

        assert!(
            cache.entry_count() <= 3,
            "cache should not exceed max_capacity; got {}",
            cache.entry_count()
        );
    }

    #[tokio::test]
    async fn cache_stores_and_retrieves_details() {
        let state = AppState::new();
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
        let state = AppState::new_with_cache_capacity(100);
        let cache = &state.github().repo_detail_cache;

        // Insert entries.
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

        // Export via iter() — same pattern as collect.rs.
        let exported: Vec<_> = cache
            .iter()
            .map(|(k, v)| ((*k).clone(), v.clone()))
            .collect();
        assert_eq!(exported.len(), 3);

        // Create a new cache and seed it.
        let new_cache = crate::app::github_infra::build_cache(100);
        for (k, v) in exported {
            new_cache.insert(k, v).await;
        }
        new_cache.run_pending_tasks().await;

        assert_eq!(new_cache.entry_count(), 3);
        let r1 = new_cache.get("repo-1").await.expect("should exist");
        assert_eq!(r1.default_branch, "branch-1");
    }

    #[test]
    fn html_cache_starts_empty() {
        let state = AppState::new();
        assert!(state.evidence().html_cache.load().is_none());
    }

    #[test]
    fn builder_default_produces_valid_state() {
        let state = AppStateBuilder::new().build();
        assert!(state.webhook().secret.is_none());
        assert!(state.evidence().html_cache.load().is_none());
    }

    #[test]
    fn builder_with_webhook_secret() {
        let state = AppStateBuilder::new().webhook_secret("test-secret").build();
        assert!(state.webhook().secret.is_some());
    }

    #[tokio::test]
    async fn builder_with_cache_capacity() {
        let state = AppStateBuilder::new().cache_capacity(5).build();
        let cache = &state.github().repo_detail_cache;
        // Insert 6 entries into a cache with capacity 5.
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

    #[test]
    fn sub_aggregate_accessors_return_correct_references() {
        let state = AppStateBuilder::new().webhook_secret("s").build();
        // Verify accessors compile and return the right types.
        let _wh: &WebhookState = state.webhook();
        let _gh: &GithubState = state.github();
        let _ev: &EvidenceState = state.evidence();
    }

    #[tokio::test]
    async fn builder_combined_cache_and_secret() {
        let state = AppStateBuilder::new()
            .cache_capacity(7)
            .webhook_secret("combo-secret")
            .build();
        // Webhook secret is set.
        assert!(state.webhook().secret.is_some());
        // Cache capacity is respected.
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

    #[test]
    fn is_ready_false_when_no_run_and_no_cache() {
        use crate::infra::server::state::ServerState;
        let state = AppStateBuilder::new().build();
        assert!(
            !state.is_ready(),
            "should not be ready with no run and no cache"
        );
    }

    #[test]
    fn is_ready_true_when_html_cache_populated() {
        use crate::infra::server::state::ServerState;
        let state = AppStateBuilder::new().build();
        // Populate the HTML cache.
        let mut pages = HashMap::new();
        pages.insert(
            "index.html".to_string(),
            CachedPage::new("index.html", b"<html>test</html>".to_vec()),
        );
        state.evidence().html_cache.store(Arc::new(Some(pages)));
        assert!(state.is_ready(), "should be ready when html_cache is Some");
    }
}
