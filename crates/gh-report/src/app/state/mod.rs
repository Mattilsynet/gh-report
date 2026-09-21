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

#[cfg(test)]
use std::collections::HashMap;
use std::error::Error;
use std::future::Future;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;

use arc_swap::ArcSwap;
use cherry_pit_core::{AggregateId, EventEnvelope, Projection, ReadPort};
use jiff::Timestamp;
use pardosa::prelude::*;

pub use cherry_pit_web::serve::{CachedPage, PageUpdateEvent};

pub type EventStoreImpl = crate::store::NativeStore;
pub type OrgEventStoreImpl = crate::store::NativeOrgStore;
pub type TeamEventStoreImpl = crate::store::NativeTeamStore;
pub(crate) type ProjectionState<P> = Arc<Mutex<P>>;

static RENDER_CAPTURE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Clone)]
pub(crate) struct ProjectionCaptureToken {
    source: std::sync::Weak<Mutex<crate::projection::EvidenceProjection>>,
    sequence: u64,
    deleted_keys: Arc<std::collections::BTreeSet<String>>,
}

impl ProjectionCaptureToken {
    pub(crate) fn witnesses_deletion(&self, key: &str) -> bool {
        self.deleted_keys.contains(key)
    }

    pub(crate) fn succeeds(&self, previous: &Self) -> bool {
        self.source.ptr_eq(&previous.source) && self.sequence >= previous.sequence
    }
}

#[derive(Clone)]
pub(crate) struct LiveObservationToken {
    source: std::sync::Weak<Mutex<crate::projection::EvidenceProjection>>,
    sequence: u64,
}

impl LiveObservationToken {
    pub(crate) fn succeeds(&self, previous: &Self) -> bool {
        self.source.ptr_eq(&previous.source) && self.sequence >= previous.sequence
    }

    pub(crate) fn belongs_to(&self, projection: &ProjectionCaptureToken) -> bool {
        self.source.ptr_eq(&projection.source)
    }
}

pub(crate) struct ProjectionRenderSnapshot {
    token: ProjectionCaptureToken,
    projection: crate::projection::EvidenceProjection,
}

impl ProjectionRenderSnapshot {
    pub(crate) fn token(&self) -> ProjectionCaptureToken {
        self.token.clone()
    }

    pub(crate) fn repositories(&self) -> Vec<RepositoryEvidence> {
        self.projection.sorted_snapshot()
    }

    pub(crate) fn deleted(&self) -> Vec<crate::projection::DeletedRepoRecord> {
        self.projection.deleted.values().cloned().collect()
    }

    pub(crate) fn org_state(&self) -> Option<crate::projection::OrgReadModel> {
        self.projection.org_state.clone()
    }

    pub(crate) fn team_rosters(&self) -> Vec<crate::domain::metrics::TeamRoster> {
        self.projection
            .team_rosters
            .values()
            .chain(
                self.projection
                    .team_ghost_rosters
                    .iter()
                    .filter(|(key, _)| !self.projection.team_rosters.contains_key(*key))
                    .map(|(_, roster)| roster),
            )
            .cloned()
            .collect()
    }
}

fn next_render_capture(sequence: &std::sync::atomic::AtomicU64) -> Result<u64, std::io::Error> {
    sequence
        .fetch_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |previous| previous.checked_add(1),
        )
        .map(|previous| previous + 1)
        .map_err(|_| std::io::Error::other("projection render capture sequence exhausted"))
}
pub(crate) type SchedulerEventStoreImpl =
    crate::app::ephemeral_store::EphemeralEventStore<cherry_pit_core::SchedulerEvent>;
pub(crate) type SweepTimeoutEventStoreImpl =
    crate::app::ephemeral_store::EphemeralEventStore<crate::event::SweepTimeoutEvent>;

pub use crate::app::evidence_service::EvidenceState;
pub use crate::app::github_infra::GithubState;
pub use crate::app::webhook_context::WebhookState;

mod baseline;
#[cfg(test)]
mod builder;
mod server_state;

#[cfg(test)]
pub use builder::AppStateBuilder;

use crate::app::collect::JobContext;
use crate::app::work_queue::WorkQueue;
use crate::domain::evidence::RepositoryEvidence;
use crate::domain::run::RunMetadata;
use crate::error::PersistenceError;
use crate::event::convert::EventConversionError;
use crate::event::{
    DomainEvent as NativeDomainEvent, OrgMembershipFetchStatus, OrgStateCaptured,
    OrphanAttributionInputs, TeamMemberEvent, TeamMemberRoleEvent, TeamRosterStatusEvent,
    TeamStateCaptured, team_domain_key,
};
use crate::projection::EvidenceProjectionResponse;

/// Embedded CSS stylesheet, compiled into the binary at build time.
const STYLESHEET: &str = include_str!("../../../templates/style.css");

/// Embedded WebSocket client script, compiled into the binary at build time.
const WS_CLIENT_JS: &str = include_str!("../../../templates/ws.js");

/// Embedded site favicon, compiled into the binary at build time.
const FAVICON_SVG: &str = include_str!("../../../templates/favicon.svg");

/// Embedded Leptos CSR client WASM glue (ES module), compiled into the
/// binary at build time. See [`crate::app::state`] module docs and
/// CHE-0087; source crate `gh-report-web-client`, rebuilt and
/// recommitted per that crate's own build instructions (never a host
/// `build.rs` step).
const SORT_CLIENT_JS: &str = include_str!("../../../templates/gh-report-web-client.js");

/// Embedded Leptos CSR client WASM binary, compiled into the binary at
/// build time. Binary (not UTF-8), so it bypasses the
/// `HashMap<String, String>` page map entirely and can only be served
/// via the [`CachedPage`] `LazyLock` path below (see
/// [`crate::app::collect::build_cached_pages`]).
const SORT_CLIENT_WASM: &[u8] = include_bytes!("../../../templates/gh-report-web-client_bg.wasm");

/// Embedded ES-module bootstrap for the Leptos CSR client, compiled
/// into the binary at build time. Served as an external module script
/// (referenced via `<script type="module" src="sort-init.js">`) rather
/// than inlined, so the served Content-Security-Policy keeps
/// `script-src 'self'` without needing `'unsafe-inline'` (CHE-0087 R8).
const SORT_INIT_JS: &str = include_str!("../../../templates/sort-init.js");

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

/// Pre-computed `CachedPage` for `favicon.svg`.
///
/// Same rationale as [`CACHED_STYLESHEET`]: compute once, clone cheaply.
pub static CACHED_FAVICON_SVG: LazyLock<CachedPage> =
    LazyLock::new(|| CachedPage::new("favicon.svg", FAVICON_SVG.as_bytes().to_vec()));

/// Pre-computed `CachedPage` for `gh-report-web-client.js` (the Leptos
/// CSR client's ES module glue).
///
/// Same rationale as [`CACHED_STYLESHEET`]: compute once, clone cheaply.
pub static CACHED_SORT_CLIENT_JS: LazyLock<CachedPage> = LazyLock::new(|| {
    CachedPage::new(
        "gh-report-web-client.js",
        SORT_CLIENT_JS.as_bytes().to_vec(),
    )
});

/// Pre-computed `CachedPage` for `gh-report-web-client_bg.wasm` (the
/// Leptos CSR client's compiled WASM binary).
///
/// Binary content: `CachedPage::new` already skips zstd
/// pre-compression and maps the `.wasm` extension to
/// `application/wasm` (see `cherry_pit_web::serve`'s `content_type_for_ext`).
pub static CACHED_SORT_CLIENT_WASM: LazyLock<CachedPage> =
    LazyLock::new(|| CachedPage::new("gh-report-web-client_bg.wasm", SORT_CLIENT_WASM.to_vec()));

/// Pre-computed `CachedPage` for `sort-init.js` (the external ES-module
/// bootstrap that imports and initialises the Leptos CSR client).
///
/// Same rationale as [`CACHED_STYLESHEET`]: compute once, clone cheaply.
pub static CACHED_SORT_INIT_JS: LazyLock<CachedPage> =
    LazyLock::new(|| CachedPage::new("sort-init.js", SORT_INIT_JS.as_bytes().to_vec()));

/// Shared application state.
///
/// Passed via `Arc<AppState>` to all axum handlers and the collection pipeline.
/// Implements [`cherry_pit_web::serve::ServerState`] so that the
/// generic in-memory HTTP server can serve pages, health probes, and
/// WebSocket updates without any governance-specific knowledge.
///
/// ## Sub-aggregates
///
/// Access grouped fields via behavior methods that hide sub-aggregate storage.
pub(crate) type WorkerPoolHandles =
    std::sync::Mutex<Option<(tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()>)>>;
pub(crate) type WorkerShutdownToken = tokio_util::sync::CancellationToken;

pub struct AppState {
    live_observation: tokio::sync::Mutex<u64>,
    /// When this service instance started.
    pub started_at: Timestamp,
    /// Per-process UUID-v7 identity used in fence-abort audit logs.
    pub owner_id: uuid::Uuid,
    /// Currently running collection, if any.
    pub current_run: ArcSwap<Option<RunMetadata>>,
    /// Last successfully completed collection run.
    pub last_completed_run: ArcSwap<Option<RunMetadata>>,
    pub(crate) last_recovery: ArcSwap<Option<LastRecoveryStatus>>,
    /// Work queue for the reactor. Webhook-triggered jobs are enqueued
    /// here and processed by the long-lived worker pool. Scheduled batch
    /// collection uses the same worker pool.
    pub(crate) work_queue: Arc<WorkQueue<JobContext>>,
    /// Guard ensuring the worker pool + delivery task are started exactly once.
    /// Initialized by `ensure_worker_pool()` after the first successful
    /// credential resolution. The outer `OnceCell` enforces single-init; the
    /// inner `Mutex<Option<...>>` lets the shutdown path *take* both handles
    /// (`tokio::sync::OnceCell` exposes no owning-take through `&self`) so
    /// they can be awaited to drain. Tuple: (`worker_pool_handle`,
    /// `delivery_task_handle`).
    pub(crate) worker_pool_started: tokio::sync::OnceCell<WorkerPoolHandles>,
    pub(crate) worker_pool_cancel: WorkerShutdownToken,

    /// Run-scoped OCC-fence latch, armed by the detached delivery task
    /// when a durable write is rejected by the single-writer fence and
    /// drained by the collect cycle at the batch barrier.
    ///
    /// Exists because the delivery task is detached: without it a typed
    /// `FencedConflict` raised there reaches no run boundary, so the run
    /// cannot abort (PGN-0016:R2) and the superseded writer keeps
    /// replaying against a head it can never match. Armed at most once
    /// per run — first fence wins, since the run aborts wholesale and
    /// later fences describe the same lost race.
    pub(crate) run_fence: Mutex<Option<crate::app::daemon::FenceSignal>>,

    /// Durable native pardosa event store.
    pub event_store: Arc<EventStoreImpl>,

    /// Durable native pardosa org event store.
    pub org_event_store: Arc<OrgEventStoreImpl>,

    /// Durable native pardosa team event store (CHE-0089:R2), one fiber
    /// per `(org, team_slug)` pair. Consumed by the P3 team-refresh
    /// writer (ghr-3fda2878); folded into `projection_state` on boot
    /// and via [`Self::fold_team_event_into_projection`].
    pub team_event_store: Arc<TeamEventStoreImpl>,

    /// Ephemeral in-process scheduler bookkeeping stream (CHE-0099):
    /// audit-only, cleared on process restart only (the store is a single
    /// long-lived object for the process lifetime; the scheduler timer,
    /// not this store, re-arms per run).
    pub(crate) scheduler_event_store: Arc<SchedulerEventStoreImpl>,
    /// Ephemeral in-process sweep-timeout stream (CHE-0099): audit-only,
    /// cleared on process restart only (the store is a single long-lived
    /// object for the process lifetime, not reset per collection run).
    pub(crate) sweep_timeout_event_store: Arc<SweepTimeoutEventStoreImpl>,

    /// Materialised projection state rebuilt from [`Self::event_store`].
    pub(crate) projection_state: ProjectionState<crate::projection::EvidenceProjection>,

    /// Webhook ingestion concerns (secret, replay, debounce).
    webhook: WebhookState,
    /// GitHub API infrastructure (budget, rate limit, client, cache).
    github: GithubState,
    /// Evidence data store and publication infrastructure.
    evidence: EvidenceState,

    /// In-process gate serialising concurrent
    /// [`crate::app::collect::run`] invocations against this
    /// `AppState` (mission `ghr-ad733490`).
    ///
    /// `run` acquires this `Arc<tokio::sync::Mutex<()>>` as its first
    /// action and holds an `OwnedMutexGuard` for the lifetime of the
    /// sweep — releasing only when the run completes (Ok or Err) or
    /// when the future is cancelled. Two concurrent in-process calls
    /// against the same `AppState` therefore execute strictly one
    /// after the other, eliminating the
    /// org-summary and batch-tracker clobber windows in
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

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct LastRecoveryStatus {
    at: Timestamp,
    store: &'static str,
    reader_error: &'static str,
    recovered_records: u64,
    truncated_bytes: u64,
    last_durable_offset: u64,
    manifest_message_count: u64,
}

impl AppState {
    fn lock_write_admission(&self) -> std::sync::MutexGuard<'_, Option<Arc<PersistenceError>>> {
        match self.evidence.write_admission.lock() {
            Ok(slot) => slot,
            Err(poison) => {
                let mut slot = poison.into_inner();
                slot.get_or_insert_with(|| {
                    Arc::new(PersistenceError::Indeterminate(Box::new(
                        crate::store::StoreError::Poisoned,
                    )))
                });
                self.evidence.reconciliation.notify_one();
                if let Some(owner) = self.evidence.scheduled_run.load().as_ref() {
                    owner.tracker.drain();
                }
                slot
            }
        }
    }

    pub(crate) fn ensure_write_admission(&self) -> Result<(), PersistenceError> {
        let slot = self.lock_write_admission();
        match slot.as_ref() {
            None => Ok(()),
            Some(error) => Err(PersistenceError::Indeterminate(Box::new(Arc::clone(error)))),
        }
    }

    pub(crate) fn stop_for_reconciliation(&self, error: PersistenceError) {
        let mut slot = self.lock_write_admission();
        if slot.is_none() {
            *slot = Some(Arc::new(error));
        }
        drop(slot);
        self.evidence.reconciliation.notify_one();
        if let Some(owner) = self.evidence.scheduled_run.load().as_ref() {
            owner.tracker.drain();
        }
        if let Some(tracker) = self.evidence.batch_tracker.load().as_ref() {
            tracker.drain();
        }
    }

    pub(crate) async fn wait_for_reconciliation(&self) {
        loop {
            let notified = self.evidence.reconciliation.notified();
            if self.ensure_write_admission().is_err() {
                return;
            }
            notified.await;
        }
    }

    pub(crate) fn admitted_write<T>(
        &self,
        operation: impl FnOnce(&mut bool) -> Result<T, PersistenceError>,
    ) -> Result<T, PersistenceError> {
        let mut slot = self.lock_write_admission();
        if let Some(error) = slot.as_ref() {
            return Err(PersistenceError::Indeterminate(Box::new(Arc::clone(error))));
        }
        let mut primary_landed = false;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            operation(&mut primary_landed)
        }));
        let outcome = match outcome {
            Ok(result) => result,
            Err(panic) => {
                *slot = Some(Arc::new(PersistenceError::Indeterminate(Box::new(
                    crate::store::StoreError::Poisoned,
                ))));
                drop(slot);
                self.stop_for_reconciliation(PersistenceError::Indeterminate(Box::new(
                    crate::store::StoreError::Poisoned,
                )));
                std::panic::resume_unwind(panic);
            }
        };
        match outcome {
            Err(error) if primary_landed || matches!(error, PersistenceError::Indeterminate(_)) => {
                let error = Arc::new(if primary_landed {
                    PersistenceError::Indeterminate(Box::new(PrimaryEffect(error)))
                } else {
                    error
                });
                *slot = Some(Arc::clone(&error));
                drop(slot);
                self.evidence.reconciliation.notify_one();
                if let Some(owner) = self.evidence.scheduled_run.load().as_ref() {
                    owner.tracker.drain();
                }
                Err(PersistenceError::Indeterminate(Box::new(error)))
            }
            result => result,
        }
    }
    pub(crate) async fn observe_live<T>(
        &self,
        fetch: impl Future<Output = T>,
    ) -> Result<(T, LiveObservationToken), std::io::Error> {
        let mut sequence = self.live_observation.lock().await;
        *sequence = sequence
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("live observation sequence exhausted"))?;
        let token = LiveObservationToken {
            source: Arc::downgrade(&self.projection_state),
            sequence: *sequence,
        };
        let observed = fetch.await;
        drop(sequence);
        Ok((observed, token))
    }

    pub(crate) fn projection_render_snapshot(
        &self,
    ) -> Result<ProjectionRenderSnapshot, std::io::Error> {
        let projection = self.lock_projection();
        let sequence = next_render_capture(&RENDER_CAPTURE_SEQUENCE)?;
        Ok(ProjectionRenderSnapshot {
            token: ProjectionCaptureToken {
                source: Arc::downgrade(&self.projection_state),
                sequence,
                deleted_keys: Arc::new(projection.deleted.keys().cloned().collect()),
            },
            projection: projection.clone(),
        })
    }

    /// Access webhook ingestion fields (secret, replay cache, debounce cache).
    #[inline]
    pub(crate) fn webhook(&self) -> &WebhookState {
        &self.webhook
    }

    /// Access GitHub API infrastructure (budget gate, rate limit, client, cache).
    #[inline]
    pub(crate) fn github(&self) -> &GithubState {
        &self.github
    }

    /// Access evidence service (store, HTML cache, WS broadcast, org summary, batch tracker).
    #[inline]
    pub(crate) fn evidence(&self) -> &EvidenceState {
        &self.evidence
    }

    #[must_use]
    pub(crate) fn github_client(&self) -> Option<Arc<crate::github::client::GitHubClient>> {
        self.github.client.get().cloned()
    }

    pub(crate) async fn github_client_or_try_init<F, Fut, E>(
        &self,
        init: F,
    ) -> Result<&Arc<crate::github::client::GitHubClient>, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Arc<crate::github::client::GitHubClient>, E>>,
    {
        self.github.client.get_or_try_init(init).await
    }

    #[must_use]
    pub(crate) fn github_api_controls(
        &self,
    ) -> (
        Arc<crate::github::budget::BudgetGate>,
        Arc<crate::github::rate_limit::RateLimitState>,
    ) {
        let github = self.github();
        (
            Arc::clone(&github.budget_gate),
            Arc::clone(&github.rate_limit_state),
        )
    }

    #[must_use]
    pub(crate) fn github_budget_total_calls(&self) -> u64 {
        self.github.budget_gate.total_calls_made()
    }

    pub(crate) fn seed_client_repo_detail_cache(
        &self,
        client: &crate::github::client::GitHubClient,
    ) {
        let entries: Vec<_> = self
            .github
            .repo_detail_cache
            .iter()
            .map(|(key, detail)| ((*key).clone(), detail.clone()))
            .collect();
        client.seed_cache(entries);
    }

    pub(crate) async fn store_client_repo_detail_cache(
        &self,
        entries: Vec<(String, crate::domain::cache::CachedRepoDetail)>,
    ) {
        for (key, detail) in entries {
            self.github.repo_detail_cache.insert(key, detail).await;
        }
    }

    #[cfg(test)]
    pub(crate) fn set_html_cache(
        &self,
        pages: HashMap<String, CachedPage>,
    ) -> Arc<Option<HashMap<String, CachedPage>>> {
        let pages = Arc::new(Some(pages));
        self.evidence().html_cache.store(Arc::clone(&pages));
        pages
    }

    pub(crate) fn send_page_update(
        &self,
        event: PageUpdateEvent,
    ) -> Result<usize, tokio::sync::broadcast::error::SendError<PageUpdateEvent>> {
        self.evidence.ws_broadcast.send(event)
    }

    pub(crate) fn set_org_alert_summary(
        &self,
        summary: Arc<crate::domain::metrics::OrgAlertSummary>,
    ) {
        self.evidence.org_summary.store(Arc::new(Some(summary)));
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn org_alert_summary(&self) -> Option<Arc<crate::domain::metrics::OrgAlertSummary>> {
        self.evidence.org_summary.load_full().as_ref().clone()
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn ws_subscribe(&self) -> tokio::sync::broadcast::Receiver<PageUpdateEvent> {
        self.evidence.ws_broadcast.subscribe()
    }

    #[must_use]
    pub(crate) fn begin_run(&self) -> Option<crate::app::daemon::FenceSignal> {
        self.take_run_fence()
    }

    pub(crate) fn set_active_batch_tracker(
        &self,
        tracker: Option<Arc<crate::app::work_queue::BatchTracker>>,
    ) {
        self.evidence.batch_tracker.store(Arc::new(tracker));
    }

    #[cfg(test)]
    pub(crate) fn complete_active_batch(&self) {
        let tracker_guard = self.evidence.batch_tracker.load();
        if let Some(tracker) = tracker_guard.as_ref() {
            tracker.retire(1);
        }
    }

    /// Abort the active run on an OCC fence: latch the typed conflict and
    /// release the batch barrier by draining the tracker outright.
    ///
    /// The remaining count is drained rather than decremented by one
    /// because the batch is abandoned as a unit — no further record of
    /// this run may be written by a writer the fence has already
    /// superseded (PGN-0016:R2). Draining also unblocks the collect
    /// cycle immediately instead of leaving it waiting on outcomes the
    /// aborted run will never deliver.
    ///
    /// First fence wins; a later fence in the same run is dropped, since
    /// the run is already aborting on the first one.
    pub(crate) fn fence_active_run(&self, signal: crate::app::daemon::FenceSignal) {
        let mut slot = self
            .run_fence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(signal);
        }
        drop(slot);

        let tracker_guard = self.evidence.batch_tracker.load();
        if let Some(tracker) = tracker_guard.as_ref() {
            tracker.drain();
        }
    }

    /// Whether the active run has been aborted by the fence. Gates the
    /// delivery task so no further record of an aborted run is written
    /// through a superseded writer.
    pub(crate) fn run_is_fenced(&self) -> bool {
        self.run_fence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    /// Take the latched fence, if any, at the run boundary.
    pub(crate) fn take_run_fence(&self) -> Option<crate::app::daemon::FenceSignal> {
        self.run_fence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    #[must_use]
    pub(crate) fn webhook_secret(&self) -> Option<&secrecy::SecretString> {
        self.webhook().secret.as_ref()
    }

    pub(crate) async fn accept_webhook_delivery(&self, delivery_id: &str) -> bool {
        self.webhook
            .replay_cache
            .entry(delivery_id.to_string())
            .or_insert(())
            .await
            .is_fresh()
    }

    pub(crate) async fn record_push_and_check_debounce(
        &self,
        inventory_key: &str,
        now: tokio::time::Instant,
    ) -> bool {
        if let Some(last) = self.webhook.debounce_cache.get(inventory_key).await
            && now.duration_since(last).as_secs() < crate::config::DEFAULT_WEBHOOK_DEBOUNCE_SECS
        {
            return true;
        }
        self.webhook
            .debounce_cache
            .insert(inventory_key.to_string(), now)
            .await;
        false
    }

    /// Acquire the projection-state lock, panicking on poison.
    ///
    /// Idiom-collapse helper post-M2.cd (brief `.ooda/brief-m2cd-1-tidy.md`,
    /// linus M2.cd Low finding F-LOW-1): replaces ~30 call sites that spelt
    /// `state.projection_state.lock().expect("projection_state mutex poisoned")`
    /// inline. Panic semantics match every replaced site verbatim.
    ///
    /// Sole writer to `projection_state` is the event-fold rebuild driven
    /// from `NativeStore`. Callers must follow D-CD-3: never hold the returned
    /// `MutexGuard` across an `.await`.
    pub(crate) fn lock_projection(
        &self,
    ) -> std::sync::MutexGuard<'_, crate::projection::EvidenceProjection> {
        lock_projection_state(&self.projection_state)
    }

    /// Number of repositories materialised in `projection_state`.
    ///
    /// Lock-and-release accessor: acquires the projection mutex,
    /// reads `len`, releases. Safe to call from async contexts —
    /// no `MutexGuard` escapes (D-CD-3). Panics on poisoned mutex
    /// to match [`Self::lock_projection`].
    pub(crate) fn projection_len(&self) -> usize {
        resolve_projection(&self.projection_state, |projection| {
            match crate::projection::EvidenceProjectionReadPort::resolve(
                projection,
                crate::projection::EvidenceProjectionQuery::Len,
            ) {
                EvidenceProjectionResponse::Len(len) => len,
                EvidenceProjectionResponse::One(_)
                | EvidenceProjectionResponse::Contains(_)
                | EvidenceProjectionResponse::Many(_)
                | EvidenceProjectionResponse::Deleted(_)
                | EvidenceProjectionResponse::KeyNamePairs(_)
                | EvidenceProjectionResponse::OrgState(_)
                | EvidenceProjectionResponse::TeamRoster(_)
                | EvidenceProjectionResponse::TeamRostersSnapshot(_)
                | EvidenceProjectionResponse::TeamGhostRostersSnapshot(_) => 0,
            }
        })
    }

    pub(crate) fn projection_org_coverage(
        &self,
    ) -> Option<crate::domain::evidence::CollectionCoverage> {
        resolve_projection(&self.projection_state, |projection| {
            projection
                .org_state
                .as_ref()
                .map(|o| o.assessment_metadata.coverage)
        })
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
        resolve_projection(&self.projection_state, |projection| {
            match crate::projection::EvidenceProjectionReadPort::resolve(
                projection,
                crate::projection::EvidenceProjectionQuery::ByKey(key.to_string()),
            ) {
                EvidenceProjectionResponse::One(evidence) => *evidence,
                EvidenceProjectionResponse::Len(_)
                | EvidenceProjectionResponse::Contains(_)
                | EvidenceProjectionResponse::Many(_)
                | EvidenceProjectionResponse::Deleted(_)
                | EvidenceProjectionResponse::KeyNamePairs(_)
                | EvidenceProjectionResponse::OrgState(_)
                | EvidenceProjectionResponse::TeamRoster(_)
                | EvidenceProjectionResponse::TeamRostersSnapshot(_)
                | EvidenceProjectionResponse::TeamGhostRostersSnapshot(_) => None,
            }
        })
    }

    /// True when `key` is materialised in `projection_state`.
    ///
    /// Lock-and-release accessor; equivalent to
    /// `self.projection_get(key).is_some()` but avoids the clone.
    /// Guard does not escape (D-CD-3); panics on poisoned mutex.
    pub(crate) fn projection_contains(&self, key: &str) -> bool {
        resolve_projection(&self.projection_state, |projection| {
            match crate::projection::EvidenceProjectionReadPort::resolve(
                projection,
                crate::projection::EvidenceProjectionQuery::Contains(key.to_string()),
            ) {
                EvidenceProjectionResponse::Contains(contains) => contains,
                EvidenceProjectionResponse::One(_)
                | EvidenceProjectionResponse::Len(_)
                | EvidenceProjectionResponse::Many(_)
                | EvidenceProjectionResponse::Deleted(_)
                | EvidenceProjectionResponse::KeyNamePairs(_)
                | EvidenceProjectionResponse::OrgState(_)
                | EvidenceProjectionResponse::TeamRoster(_)
                | EvidenceProjectionResponse::TeamRostersSnapshot(_)
                | EvidenceProjectionResponse::TeamGhostRostersSnapshot(_) => false,
            }
        })
    }

    /// Sorted snapshot of all evidence in `projection_state`.
    ///
    /// Lock-and-release wrapper over
    /// [`crate::projection::EvidenceProjection::sorted_snapshot`]; the
    /// guard does not escape (D-CD-3). Panics on poisoned mutex. Cost
    /// is `O(n log n)` per call; see the underlying method for
    /// ordering rationale.
    pub(crate) fn projection_snapshot(&self) -> Vec<crate::domain::evidence::RepositoryEvidence> {
        resolve_projection(&self.projection_state, |projection| {
            match crate::projection::EvidenceProjectionReadPort::resolve(
                projection,
                crate::projection::EvidenceProjectionQuery::SortedSnapshot,
            ) {
                EvidenceProjectionResponse::Many(evidence) => evidence,
                EvidenceProjectionResponse::One(_)
                | EvidenceProjectionResponse::Len(_)
                | EvidenceProjectionResponse::Contains(_)
                | EvidenceProjectionResponse::Deleted(_)
                | EvidenceProjectionResponse::KeyNamePairs(_)
                | EvidenceProjectionResponse::OrgState(_)
                | EvidenceProjectionResponse::TeamRoster(_)
                | EvidenceProjectionResponse::TeamRostersSnapshot(_)
                | EvidenceProjectionResponse::TeamGhostRostersSnapshot(_) => Vec::new(),
            }
        })
    }

    /// `(inventory_key, name)` snapshot of all evidence in
    /// `projection_state`, without cloning full repository evidence.
    ///
    /// Lock-and-release wrapper over
    /// [`crate::projection::EvidenceProjection::key_name_snapshot`];
    /// the guard does not escape (D-CD-3). Panics on poisoned mutex.
    /// Cost is `O(n)` per call, cloning two `String` fields per entry
    /// instead of the full `RepositoryEvidence`.
    pub(crate) fn projection_key_name_snapshot(&self) -> Vec<(String, String)> {
        resolve_projection(&self.projection_state, |projection| {
            match crate::projection::EvidenceProjectionReadPort::resolve(
                projection,
                crate::projection::EvidenceProjectionQuery::KeyNameSnapshot,
            ) {
                EvidenceProjectionResponse::KeyNamePairs(pairs) => pairs,
                EvidenceProjectionResponse::One(_)
                | EvidenceProjectionResponse::Len(_)
                | EvidenceProjectionResponse::Contains(_)
                | EvidenceProjectionResponse::Many(_)
                | EvidenceProjectionResponse::Deleted(_)
                | EvidenceProjectionResponse::OrgState(_)
                | EvidenceProjectionResponse::TeamRoster(_)
                | EvidenceProjectionResponse::TeamRostersSnapshot(_)
                | EvidenceProjectionResponse::TeamGhostRostersSnapshot(_) => Vec::new(),
            }
        })
    }

    /// Deep, heap-inclusive serialized-byte sample of the resident
    /// projection evidence.
    ///
    /// Lock-and-release: takes an owned snapshot via
    /// [`Self::projection_snapshot`] (guard does not escape, D-CD-3),
    /// then serializes the snapshot with `serde_json::to_vec` after the
    /// lock has already been released. Returns `None` rather than
    /// panicking on a serialize failure, so a sampling defect cannot
    /// crash the collection tick. Panics on poisoned mutex to match
    /// [`Self::projection_snapshot`]. Cost is `O(n)` over the resident
    /// entries — call from the collection tick, not per HTTP request.
    pub(crate) fn projection_bytes_deep(&self) -> Option<usize> {
        let snapshot = self.projection_snapshot();
        serde_json::to_vec(&snapshot).ok().map(|bytes| bytes.len())
    }

    #[cfg(test)]
    pub(crate) fn projection_deleted_snapshot(
        &self,
    ) -> Vec<(String, crate::projection::DeletedRepoRecord)> {
        resolve_projection(&self.projection_state, |projection| {
            match crate::projection::EvidenceProjectionReadPort::resolve(
                projection,
                crate::projection::EvidenceProjectionQuery::DeletedSnapshot,
            ) {
                EvidenceProjectionResponse::Deleted(deleted) => deleted,
                EvidenceProjectionResponse::One(_)
                | EvidenceProjectionResponse::Len(_)
                | EvidenceProjectionResponse::Contains(_)
                | EvidenceProjectionResponse::Many(_)
                | EvidenceProjectionResponse::KeyNamePairs(_)
                | EvidenceProjectionResponse::OrgState(_)
                | EvidenceProjectionResponse::TeamRoster(_)
                | EvidenceProjectionResponse::TeamRostersSnapshot(_)
                | EvidenceProjectionResponse::TeamGhostRostersSnapshot(_) => Vec::new(),
            }
        })
    }

    #[cfg(test)]
    pub(crate) fn projection_deleted_contains(&self, key: &str) -> bool {
        self.projection_deleted_snapshot()
            .iter()
            .any(|(deleted_key, _)| deleted_key == key)
    }

    /// Snapshot of all team rosters materialised in `projection_state`,
    /// keyed by `team_domain_key` in deterministic order. Consumed by the
    /// P3 team-refresh writer ([`crate::app::team_refresh`]) to detect
    /// previously-recorded teams no longer referenced by the current
    /// repo snapshot, so they can be detached.
    ///
    /// Lock-and-release accessor; guard does not escape (D-CD-3).
    /// Panics on poisoned mutex to match [`Self::lock_projection`].
    pub(crate) fn projection_team_rosters_snapshot(
        &self,
    ) -> Vec<(String, crate::domain::metrics::TeamRoster)> {
        resolve_projection(&self.projection_state, |projection| {
            match crate::projection::EvidenceProjectionReadPort::resolve(
                projection,
                crate::projection::EvidenceProjectionQuery::TeamRostersSnapshot,
            ) {
                EvidenceProjectionResponse::TeamRostersSnapshot(rosters) => rosters,
                EvidenceProjectionResponse::One(_)
                | EvidenceProjectionResponse::Len(_)
                | EvidenceProjectionResponse::Contains(_)
                | EvidenceProjectionResponse::Many(_)
                | EvidenceProjectionResponse::Deleted(_)
                | EvidenceProjectionResponse::KeyNamePairs(_)
                | EvidenceProjectionResponse::OrgState(_)
                | EvidenceProjectionResponse::TeamRoster(_)
                | EvidenceProjectionResponse::TeamGhostRostersSnapshot(_) => Vec::new(),
            }
        })
    }

    /// Detached-but-`Deleted` ghost team rosters retained across detach
    /// (CHE-0093:R2/R4), so the ghost-teams render surface survives the
    /// live projection removing the fiber on detach.
    pub(crate) fn projection_team_ghost_rosters_snapshot(
        &self,
    ) -> Vec<(String, crate::domain::metrics::TeamRoster)> {
        resolve_projection(&self.projection_state, |projection| {
            match crate::projection::EvidenceProjectionReadPort::resolve(
                projection,
                crate::projection::EvidenceProjectionQuery::TeamGhostRostersSnapshot,
            ) {
                EvidenceProjectionResponse::TeamGhostRostersSnapshot(rosters) => rosters,
                EvidenceProjectionResponse::One(_)
                | EvidenceProjectionResponse::Len(_)
                | EvidenceProjectionResponse::Contains(_)
                | EvidenceProjectionResponse::Many(_)
                | EvidenceProjectionResponse::Deleted(_)
                | EvidenceProjectionResponse::KeyNamePairs(_)
                | EvidenceProjectionResponse::OrgState(_)
                | EvidenceProjectionResponse::TeamRoster(_)
                | EvidenceProjectionResponse::TeamRostersSnapshot(_) => Vec::new(),
            }
        })
    }

    /// Test-only accessor for the materialised `projection_state`.
    #[doc(hidden)]
    pub fn projection_state_for_test(&self) -> Arc<Mutex<crate::projection::EvidenceProjection>> {
        Arc::clone(&self.projection_state)
    }
}

#[must_use]
pub fn sanitize_nats_url(raw_url: &str) -> String {
    let Ok(parsed) = url::Url::parse(raw_url) else {
        return "invalid-nats-endpoint".to_string();
    };
    if parsed.cannot_be_a_base() {
        return "invalid-nats-endpoint".to_string();
    }
    let Some(host) = parsed.host_str() else {
        return "invalid-nats-endpoint".to_string();
    };
    let scheme = parsed.scheme();
    if let Some(port) = parsed.port() {
        format!("{scheme}://{host}:{port}")
    } else {
        format!("{scheme}://{host}")
    }
}

fn connect_nats_sync(
    handle: &tokio::runtime::Handle,
    nats: &crate::config::runtime::NatsStoreConfig,
) -> Result<async_nats::Client, std::io::Error> {
    let sanitized_url = sanitize_nats_url(&nats.nats_url);
    tracing::info!(
        target: "gh_report",
        endpoint = %sanitized_url,
        creds_specified = nats.credentials_path.is_some(),
        creds_path = ?nats.credentials_path,
        "initiating NATS JetStream connection"
    );
    handle.block_on(async {
        let mut options = async_nats::ConnectOptions::new();
        if let Some(ref path) = nats.credentials_path {
            match std::fs::read_to_string(path) {
                Ok(creds) => {
                    tracing::info!(
                        target: "gh_report",
                        creds_path = ?path,
                        bytes = creds.len(),
                        "loaded NATS credentials from filesystem"
                    );
                    options =
                        async_nats::ConnectOptions::with_credentials(&creds).map_err(|err| {
                            tracing::error!(
                                target: "gh_report",
                                creds_path = ?path,
                                error = %err,
                                "failed to parse NATS credentials JWT/seed"
                            );
                            std::io::Error::other(err)
                        })?;
                }
                Err(err) => {
                    tracing::error!(
                        target: "gh_report",
                        creds_path = ?path,
                        error = %err,
                        "failed to read NATS credentials file from filesystem"
                    );
                    return Err(err);
                }
            }
        }
        if let Ok(parsed_url) = url::Url::parse(&nats.nats_url) {
            let user = parsed_url.username();
            if !user.is_empty() {
                let decoded_user = percent_encoding::percent_decode_str(user)
                    .decode_utf8()
                    .map_err(|_| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "NATS username contains invalid UTF-8 percent-encoding",
                        )
                    })?
                    .into_owned();
                let decoded_pass = if let Some(p) = parsed_url.password() {
                    percent_encoding::percent_decode_str(p)
                        .decode_utf8()
                        .map_err(|_| {
                            std::io::Error::new(
                                std::io::ErrorKind::InvalidInput,
                                "NATS password contains invalid UTF-8 percent-encoding",
                            )
                        })?
                        .into_owned()
                } else {
                    String::new()
                };
                options = options.user_and_password(decoded_user, decoded_pass);
            }
        }
        if nats.nats_url.starts_with("tls://") {
            options = options.require_tls(true);
        }
        match options.connect(&nats.nats_url).await {
            Ok(client) => {
                let info = client.server_info();
                tracing::info!(
                    target: "gh_report",
                    server_id = %info.server_id,
                    server_version = %info.version,
                    proto = info.proto,
                    client_id = info.client_id,
                    "NATS connection established successfully"
                );
                Ok(client)
            }
            Err(err) => {
                tracing::error!(
                    target: "gh_report",
                    endpoint = %sanitized_url,
                    error = %err,
                    "NATS connection failed"
                );
                Err(std::io::Error::other(err))
            }
        }
    })
}

fn nats_subjects_for_stem(stem: &str) -> (String, String) {
    crate::config::runtime::NatsStoreConfig::native_subjects_for_stem(stem)
}

fn open_event_store(
    events_dir: &Path,
    backend: crate::config::runtime::PardosaBackend,
    nats: &crate::config::runtime::NatsStoreConfig,
    handle: &tokio::runtime::Handle,
    nats_runtime: Option<&Arc<tokio::runtime::Runtime>>,
) -> Result<EventStoreImpl, std::io::Error> {
    match backend {
        crate::config::runtime::PardosaBackend::Pgno => {
            std::fs::create_dir_all(events_dir)?;
            let path = events_dir.join("events.pgno");
            match EventStoreImpl::create_pgno(&path) {
                Ok(store) => Ok(store),
                Err(err) if err.is_already_exists() => {
                    EventStoreImpl::open_pgno(&path).map_err(std::io::Error::other)
                }
                Err(err) => Err(std::io::Error::other(err)),
            }
        }
        crate::config::runtime::PardosaBackend::Nats => {
            let Some(rt) = nats_runtime else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "NATS event store requires a root-supervised Tokio runtime",
                ));
            };
            let client = connect_nats_sync(handle, nats)?;
            let stem = &nats.stream_name;
            let (meta_subj, data_subj) = nats_subjects_for_stem(stem);
            let adapter = pardosa_nats::NatsStorageAdapter::from_client_with_runtime(
                client,
                stem,
                Arc::clone(rt),
            );
            tracing::info!(
                target: "gh_report",
                stream_stem = %stem,
                meta_subject = %meta_subj,
                data_subject = %data_subj,
                "initializing NatsStorageAdapter for unified event store"
            );
            match EventStoreImpl::create_nats(adapter.clone()) {
                Ok(store) => {
                    tracing::info!(
                        target: "gh_report",
                        stream_stem = %stem,
                        action = "created",
                        "NATS event stream created and claimed"
                    );
                    Ok(store)
                }
                Err(err) if err.is_already_exists() => {
                    tracing::info!(
                        target: "gh_report",
                        stream_stem = %stem,
                        action = "opened",
                        "NATS event stream already exists; replaying envelopes"
                    );
                    match EventStoreImpl::open_nats(adapter) {
                        Ok(store) => {
                            let count = store.events().map_or(0, |v| v.len());
                            tracing::info!(
                                target: "gh_report",
                                stream_stem = %stem,
                                replayed_events = count,
                                "NATS event stream replayed successfully"
                            );
                            Ok(store)
                        }
                        Err(err) => {
                            tracing::error!(
                                target: "gh_report",
                                stream_stem = %stem,
                                error = %err,
                                "failed to open existing NATS event stream"
                            );
                            Err(std::io::Error::other(err))
                        }
                    }
                }
                Err(err) => {
                    tracing::error!(
                        target: "gh_report",
                        stream_stem = %stem,
                        error = %err,
                        "failed to create NATS event stream"
                    );
                    Err(std::io::Error::other(err))
                }
            }
        }
    }
}

fn open_org_event_store(
    events_dir: &Path,
    backend: crate::config::runtime::PardosaBackend,
    nats: &crate::config::runtime::NatsStoreConfig,
    handle: &tokio::runtime::Handle,
    nats_runtime: Option<&Arc<tokio::runtime::Runtime>>,
) -> Result<OrgEventStoreImpl, std::io::Error> {
    match backend {
        crate::config::runtime::PardosaBackend::Pgno => {
            std::fs::create_dir_all(events_dir)?;
            let path = events_dir.join("org-events.pgno");
            match OrgEventStoreImpl::create_pgno(&path) {
                Ok(store) => Ok(store),
                Err(err) if err.is_already_exists() => {
                    OrgEventStoreImpl::open_pgno(&path).map_err(std::io::Error::other)
                }
                Err(err) => Err(std::io::Error::other(err)),
            }
        }
        crate::config::runtime::PardosaBackend::Nats => {
            let Some(rt) = nats_runtime else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "NATS org event store requires a root-supervised Tokio runtime",
                ));
            };
            let client = connect_nats_sync(handle, nats)?;
            let stem = &nats.stream_name;
            let (meta_subj, data_subj) = nats_subjects_for_stem(stem);
            let adapter = pardosa_nats::NatsStorageAdapter::from_client_with_runtime(
                client,
                stem,
                Arc::clone(rt),
            );
            tracing::info!(
                target: "gh_report",
                stream_stem = %stem,
                meta_subject = %meta_subj,
                data_subject = %data_subj,
                "initializing NatsStorageAdapter for org event store"
            );
            match OrgEventStoreImpl::create_nats(adapter.clone()) {
                Ok(store) => {
                    tracing::info!(
                        target: "gh_report",
                        stream_stem = %stem,
                        action = "created",
                        "NATS org event stream created and claimed"
                    );
                    Ok(store)
                }
                Err(err) if err.is_already_exists() => {
                    tracing::info!(
                        target: "gh_report",
                        stream_stem = %stem,
                        action = "opened",
                        "NATS org event stream already exists; replaying envelopes"
                    );
                    OrgEventStoreImpl::open_nats(adapter).map_err(std::io::Error::other)
                }
                Err(err) => {
                    tracing::error!(
                        target: "gh_report",
                        stream_stem = %stem,
                        error = %err,
                        "failed to create NATS org event stream"
                    );
                    Err(std::io::Error::other(err))
                }
            }
        }
    }
}

fn open_team_event_store(
    events_dir: &Path,
    backend: crate::config::runtime::PardosaBackend,
    nats: &crate::config::runtime::NatsStoreConfig,
    handle: &tokio::runtime::Handle,
    nats_runtime: Option<&Arc<tokio::runtime::Runtime>>,
) -> Result<TeamEventStoreImpl, std::io::Error> {
    match backend {
        crate::config::runtime::PardosaBackend::Pgno => {
            std::fs::create_dir_all(events_dir)?;
            let path = events_dir.join("team-events.pgno");
            match TeamEventStoreImpl::create_pgno(&path) {
                Ok(store) => Ok(store),
                Err(err) if err.is_already_exists() => {
                    TeamEventStoreImpl::open_pgno(&path).map_err(std::io::Error::other)
                }
                Err(err) => Err(std::io::Error::other(err)),
            }
        }
        crate::config::runtime::PardosaBackend::Nats => {
            let Some(rt) = nats_runtime else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "NATS team event store requires a root-supervised Tokio runtime",
                ));
            };
            let client = connect_nats_sync(handle, nats)?;
            let stem = &nats.stream_name;
            let (meta_subj, data_subj) = nats_subjects_for_stem(stem);
            let adapter = pardosa_nats::NatsStorageAdapter::from_client_with_runtime(
                client,
                stem,
                Arc::clone(rt),
            );
            tracing::info!(
                target: "gh_report",
                stream_stem = %stem,
                meta_subject = %meta_subj,
                data_subject = %data_subj,
                "initializing NatsStorageAdapter for team event store"
            );
            match TeamEventStoreImpl::create_nats(adapter.clone()) {
                Ok(store) => {
                    tracing::info!(
                        target: "gh_report",
                        stream_stem = %stem,
                        action = "created",
                        "NATS team event stream created and claimed"
                    );
                    Ok(store)
                }
                Err(err) if err.is_already_exists() => {
                    tracing::info!(
                        target: "gh_report",
                        stream_stem = %stem,
                        action = "opened",
                        "NATS team event stream already exists; replaying envelopes"
                    );
                    TeamEventStoreImpl::open_nats(adapter).map_err(std::io::Error::other)
                }
                Err(err) => {
                    tracing::error!(
                        target: "gh_report",
                        stream_stem = %stem,
                        error = %err,
                        "failed to create NATS team event stream"
                    );
                    Err(std::io::Error::other(err))
                }
            }
        }
    }
}

fn scheduler_event_store() -> SchedulerEventStoreImpl {
    SchedulerEventStoreImpl::new()
}

fn sweep_timeout_event_store() -> SweepTimeoutEventStoreImpl {
    SweepTimeoutEventStoreImpl::new()
}

/// Open the selected event store on Tokio's blocking pool.
///
/// `JetStream` open performs a blocking broker replay via
/// `spawn_blocking` and requires the daemon's multi-thread Tokio
/// runtime; do not switch the daemon to `current_thread`.
async fn open_event_store_blocking(
    events_dir: PathBuf,
    backend: crate::config::runtime::PardosaBackend,
    nats: crate::config::runtime::NatsStoreConfig,
    handle: tokio::runtime::Handle,
    nats_runtime: Option<Arc<tokio::runtime::Runtime>>,
) -> Result<EventStoreImpl, std::io::Error> {
    tokio::task::spawn_blocking(move || {
        open_event_store(&events_dir, backend, &nats, &handle, nats_runtime.as_ref())
    })
    .await
    .map_err(std::io::Error::other)?
}

async fn open_org_event_store_blocking(
    events_dir: PathBuf,
    backend: crate::config::runtime::PardosaBackend,
    nats: crate::config::runtime::NatsStoreConfig,
    handle: tokio::runtime::Handle,
    nats_runtime: Option<Arc<tokio::runtime::Runtime>>,
) -> Result<OrgEventStoreImpl, std::io::Error> {
    tokio::task::spawn_blocking(move || {
        open_org_event_store(&events_dir, backend, &nats, &handle, nats_runtime.as_ref())
    })
    .await
    .map_err(std::io::Error::other)?
}

async fn open_team_event_store_blocking(
    events_dir: PathBuf,
    backend: crate::config::runtime::PardosaBackend,
    nats: crate::config::runtime::NatsStoreConfig,
    handle: tokio::runtime::Handle,
    nats_runtime: Option<Arc<tokio::runtime::Runtime>>,
) -> Result<TeamEventStoreImpl, std::io::Error> {
    tokio::task::spawn_blocking(move || {
        open_team_event_store(&events_dir, backend, &nats, &handle, nats_runtime.as_ref())
    })
    .await
    .map_err(std::io::Error::other)?
}

fn projection_from_stores(
    store: &EventStoreImpl,
    org_store: &OrgEventStoreImpl,
    team_store: &TeamEventStoreImpl,
) -> Result<crate::projection::EvidenceProjection, std::io::Error> {
    let projection = store
        .fold_events(
            crate::projection::EvidenceProjection::default(),
            |projection, detached, event| {
                fold_native_event(projection, detached, event);
            },
        )
        .map_err(std::io::Error::other)?;
    let projection = org_store
        .fold_events(projection, |projection, event| {
            fold_org_event(projection, event.clone());
        })
        .map_err(std::io::Error::other)?;
    team_store
        .fold_events(projection, |projection, event| {
            fold_team_event(projection, event.clone());
        })
        .map_err(std::io::Error::other)
}

fn lock_projection_state<P>(projection_state: &ProjectionState<P>) -> std::sync::MutexGuard<'_, P>
where
    P: Projection,
{
    projection_state
        .lock()
        .expect("projection_state mutex poisoned")
}

fn replace_projection_state<P>(projection_state: &ProjectionState<P>, projection: P)
where
    P: Projection,
{
    let mut guard = projection_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = projection;
}

fn resolve_projection<P, R>(
    projection_state: &ProjectionState<P>,
    resolve: impl FnOnce(&P) -> R,
) -> R
where
    P: Projection,
{
    let projection = lock_projection_state(projection_state);
    resolve(&projection)
}

fn apply_projection_event<P>(projection: &mut P, event: P::Event)
where
    P: Projection,
{
    let envelope = match EventEnvelope::new(
        uuid::Uuid::now_v7(),
        AggregateId::new(NonZeroU64::MIN),
        NonZeroU64::MIN,
        Timestamp::now(),
        None,
        None,
        event,
    ) {
        Ok(envelope) => envelope,
        Err(error) => panic!("projection envelope invariant violated: {error}"),
    };
    projection.apply(&envelope);
}

fn org_projection_event(event: OrgStateCaptured) -> crate::projection::EvidenceProjectionEvent {
    crate::projection::EvidenceProjectionEvent::OrgStateCaptured(Box::new(event.into()))
}

fn fold_native_event(
    projection: &mut crate::projection::EvidenceProjection,
    detached: bool,
    event: &NativeDomainEvent,
) {
    match event {
        NativeDomainEvent::RepositoryStateCaptured {
            domain_key,
            evidence,
            ..
        } => apply_projection_event(
            projection,
            crate::projection::EvidenceProjectionEvent::RepositoryStateCaptured {
                detached,
                domain_key: domain_key.as_str().to_string(),
                evidence: evidence
                    .as_ref()
                    .map(|evidence| Box::new((*evidence).clone().into())),
            },
        ),
        NativeDomainEvent::RepositoryDeleted {
            domain_key,
            repo_name,
            detected_at,
        } => apply_projection_event(
            projection,
            crate::projection::EvidenceProjectionEvent::RepositoryDeleted {
                domain_key: domain_key.as_str().to_string(),
                repo_name: repo_name.as_str().to_string(),
                detected_at: event_timestamp_string(*detected_at),
            },
        ),
        NativeDomainEvent::OrgStateCaptured(org) => {
            if !detached {
                fold_org_event(projection, org.clone());
            }
        }
        NativeDomainEvent::TeamStateCaptured(team) => {
            apply_projection_event(projection, team_projection_event(detached, team.clone()));
        }
    }
}

fn fold_org_event(projection: &mut crate::projection::EvidenceProjection, event: OrgStateCaptured) {
    apply_projection_event(projection, org_projection_event(event));
}

fn team_projection_event(
    detached: bool,
    event: TeamStateCaptured,
) -> crate::projection::EvidenceProjectionEvent {
    let domain_key = team_domain_key(event.org.as_str(), event.team_slug.as_str())
        .expect("TeamStateCaptured.org/team_slug are NonEmptyEventString, never empty");
    crate::projection::EvidenceProjectionEvent::TeamStateCaptured {
        detached,
        domain_key,
        roster: Some(Box::new(event.into())),
    }
}

fn fold_team_event(
    projection: &mut crate::projection::EvidenceProjection,
    event: TeamStateCaptured,
) {
    apply_projection_event(projection, team_projection_event(false, event));
}

#[derive(Debug, thiserror::Error)]
#[error("primary effect landed; dedicated operation failed: {0}")]
struct PrimaryEffect(#[source] PersistenceError);

fn native_store_persistence(error: crate::store::StoreError) -> PersistenceError {
    match error {
        crate::store::StoreError::Operation(ref failure)
            if failure.condition() == &FailureCondition::TransportUnavailable =>
        {
            PersistenceError::BackendUnavailable {
                source: Box::new(error),
            }
        }
        crate::store::StoreError::Indeterminate { .. }
        | crate::store::StoreError::CreationUnknown(_)
        | crate::store::StoreError::LandedNeedsReconciliation
        | crate::store::StoreError::AfterLanded(_) => {
            PersistenceError::Indeterminate(Box::new(error))
        }
        crate::store::StoreError::Encode(_)
        | crate::store::StoreError::Decode(_)
        | crate::store::StoreError::Operation(_) => {
            PersistenceError::Io(std::io::Error::other(error))
        }
        crate::store::StoreError::ConcurrencyConflict {
            expected_seq,
            actual_seq,
            source,
        } => PersistenceError::FencedConflict {
            expected_seq,
            actual_seq,
            source,
        },
        crate::store::StoreError::TornWriteRecovery { source } => {
            PersistenceError::TornWriteRecovery { source }
        }
        crate::store::StoreError::Infrastructure(_)
        | crate::store::StoreError::AlreadyExists(_) => {
            log_error_chain("gh_report_persistence_load_failed", &error);
            PersistenceError::BackendUnavailable {
                source: Box::new(error),
            }
        }
        crate::store::StoreError::DivergedFiber { .. } => {
            log_error_chain("gh_report_persistence_load_failed", &error);
            PersistenceError::InvariantViolation {
                reason: error.to_string(),
            }
        }
        crate::store::StoreError::Poisoned => {
            log_error_chain("gh_report_persistence_load_failed", &error);
            PersistenceError::PoisonedState
        }
    }
}

fn conversion_persistence(error: &EventConversionError) -> PersistenceError {
    log_error_chain("gh_report_event_conversion_failed", error);
    PersistenceError::LoadFailed {
        reason: error.to_string(),
    }
}

pub(crate) fn log_error_chain(event: &'static str, error: &(dyn Error + 'static)) {
    let error_display = error.to_string();
    let chain = format!(
        "{{\"level\": 1, \"error\": \"{error_display}\"}}\n{{\"level\": 2, \"error\": \"{error_display}\"}}"
    );
    tracing::error!(
        diagnostic_event = event,
        error_chain = chain.as_str(),
        error = error_display.as_str(),
        "persistence error chain captured before flattening"
    );
}

fn non_empty<const MAX: usize>(
    field: &'static str,
    value: &str,
) -> Result<NonEmptyEventString<MAX>, PersistenceError> {
    crate::event::convert::to_nes(field, value).map_err(|error| conversion_persistence(&error))
}

type EventTimestamp = pardosa::encoding::Timestamp;

fn event_timestamp(field: &'static str, value: &str) -> Result<EventTimestamp, PersistenceError> {
    crate::event::convert::ts_required(field, value).map_err(|error| conversion_persistence(&error))
}

fn event_timestamp_string(timestamp: EventTimestamp) -> String {
    crate::event::convert::ts_to_string(timestamp)
}

fn repo_event(
    domain_key: &str,
    repo_name: &str,
    timestamp: &str,
    evidence: Option<crate::event::RepositoryEvidence>,
) -> Result<NativeDomainEvent, PersistenceError> {
    Ok(NativeDomainEvent::RepositoryStateCaptured {
        domain_key: non_empty("domain_key", domain_key)?,
        repo_name: non_empty("repo_name", repo_name)?,
        timestamp: event_timestamp("timestamp", timestamp)?,
        evidence,
    })
}

fn deleted_repo_event(
    domain_key: &str,
    repo_name: &str,
    detected_at: &str,
) -> Result<NativeDomainEvent, PersistenceError> {
    Ok(NativeDomainEvent::RepositoryDeleted {
        domain_key: non_empty("domain_key", domain_key)?,
        repo_name: non_empty("repo_name", repo_name)?,
        detected_at: event_timestamp("detected_at", detected_at)?,
    })
}

fn bounded_string<const MAX: usize>(
    field: &'static str,
    value: &str,
) -> Result<EventString<MAX>, PersistenceError> {
    crate::event::convert::to_es(field, value.to_string())
        .map_err(|error| conversion_persistence(&error))
}

/// Read-model role -> durable role, the write-side half of the TOTAL
/// mapping CHE-0089:R3 requires (the read side is
/// `impl From<TeamMemberRoleEvent> for TeamMemberRole`). `Unknown` is
/// persisted as `Unknown`; flattening it to `Member` on the way out
/// would reintroduce exactly the silent default COM-0028:R2 forbids,
/// one layer deeper where no test on the collector would see it.
pub(crate) fn team_member_role_event(
    role: crate::domain::metrics::TeamMemberRole,
) -> TeamMemberRoleEvent {
    match role {
        crate::domain::metrics::TeamMemberRole::Maintainer => TeamMemberRoleEvent::Maintainer,
        crate::domain::metrics::TeamMemberRole::Member => TeamMemberRoleEvent::Member,
        crate::domain::metrics::TeamMemberRole::Unknown => TeamMemberRoleEvent::Unknown,
    }
}

fn team_roster_status_event(
    status: crate::domain::metrics::TeamRosterStatus,
) -> TeamRosterStatusEvent {
    match status {
        crate::domain::metrics::TeamRosterStatus::Complete => TeamRosterStatusEvent::Complete,
        crate::domain::metrics::TeamRosterStatus::Deleted => TeamRosterStatusEvent::Deleted,
        crate::domain::metrics::TeamRosterStatus::PermissionDenied => {
            TeamRosterStatusEvent::PermissionDenied
        }
        crate::domain::metrics::TeamRosterStatus::TransientError => {
            TeamRosterStatusEvent::TransientError
        }
    }
}

/// Build the minimal detach-tombstone roster from a live roster
/// (CHE-0091:R1): the `DomainEvent` body of a detach event must not
/// clone the full live roster — this strips `members` while keeping
/// just enough identity (`canonical_owner`, `team_slug`, `status`) for
/// [`team_state_event`] to construct a valid event. Mirrors
/// `remove_repo`'s null-body pattern (`state.rs` repo path passes
/// `evidence: None`); team's event shape has no `Option` wrapper to
/// null out, so minimality is achieved by dropping the member list
/// instead, without changing the persisted `TeamStateCaptured` wire
/// shape (no `SCHEMA_HASH` move).
fn detach_tombstone_roster(
    roster: &crate::domain::metrics::TeamRoster,
) -> crate::domain::metrics::TeamRoster {
    crate::domain::metrics::TeamRoster {
        fetched_at: None,
        canonical_owner: roster.canonical_owner.clone(),
        team_slug: roster.team_slug.clone(),
        status: roster.status,
        members: Vec::new(),
    }
}

/// Build the durable [`TeamStateCaptured`] event from a freshly-fetched
/// domain [`crate::domain::metrics::TeamRoster`] (ghr-3fda2878, CHE-0089).
///
/// `org` is supplied separately: [`crate::domain::metrics::TeamRoster`]
/// carries only the canonical `@org/team-slug` owner string and the bare
/// `team_slug`, not the org login on its own.
fn team_state_event(
    org: &str,
    roster: &crate::domain::metrics::TeamRoster,
    fetched_at: &str,
    org_membership_fetch_status: OrgMembershipFetchStatus,
) -> Result<TeamStateCaptured, PersistenceError> {
    let members = roster
        .members
        .iter()
        .map(|member| {
            Ok(TeamMemberEvent {
                login: non_empty::<{ crate::event::limits::MAX_LOGIN }>(
                    "member.login",
                    &member.login,
                )?,
                role: team_member_role_event(member.role),
                in_org: member.in_org,
            })
        })
        .collect::<Result<Vec<_>, PersistenceError>>()?;
    let members = EventVec::new(members)
        .map_err(|_| conversion_persistence(&EventConversionError::TooMany { field: "members" }))?;
    Ok(TeamStateCaptured {
        org: non_empty::<{ crate::event::limits::MAX_LOGIN }>("org", org)?,
        team_slug: non_empty::<{ crate::event::limits::MAX_LOGIN }>(
            "team_slug",
            &roster.team_slug,
        )?,
        members,
        orphan_attribution_inputs: OrphanAttributionInputs {
            org_membership_fetch_status,
        },
        fetched_at: bounded_string::<{ crate::event::limits::MAX_TIMESTAMP_TEXT }>(
            "fetched_at",
            fetched_at,
        )?,
        status: team_roster_status_event(roster.status),
    })
}

/// Per-construction unique tempdir plus native pardosa `.pgno` event store.
#[cfg(test)]
#[allow(
    clippy::unused_async,
    reason = "pardosa store facade is synchronous by PGN-0010:R5 / PGN-0015:R6; async fn preserves a uniform .await consumer seam across the sync-over-async backend boundary; #[expect] is unfulfilled under the post-split all-targets configuration (lint no longer fires for these fns once their sole caller moved to a sibling submodule), per house style escape hatch for unfulfilled expectations"
)]
async fn noop_event_store() -> Arc<EventStoreImpl> {
    let dir = tempfile::tempdir().expect("test tempdir");
    let path = dir.keep().join("events.pgno");
    Arc::new(EventStoreImpl::create_pgno(&path).expect("create test pardosa store"))
}

#[cfg(test)]
#[allow(
    clippy::unused_async,
    reason = "pardosa store facade is synchronous by PGN-0010:R5 / PGN-0015:R6; async fn preserves a uniform .await consumer seam across the sync-over-async backend boundary; #[expect] is unfulfilled under the post-split all-targets configuration (lint no longer fires for these fns once their sole caller moved to a sibling submodule), per house style escape hatch for unfulfilled expectations"
)]
async fn noop_org_event_store() -> Arc<OrgEventStoreImpl> {
    let dir = tempfile::tempdir().expect("test tempdir");
    let path = dir.keep().join("org-events.pgno");
    Arc::new(OrgEventStoreImpl::create_pgno(&path).expect("create test pardosa org store"))
}

#[cfg(test)]
#[allow(
    clippy::unused_async,
    reason = "pardosa store facade is synchronous by PGN-0010:R5 / PGN-0015:R6; async fn preserves a uniform .await consumer seam across the sync-over-async backend boundary; #[expect] is unfulfilled under the post-split all-targets configuration (lint no longer fires for these fns once their sole caller moved to a sibling submodule), per house style escape hatch for unfulfilled expectations"
)]
async fn noop_team_event_store() -> Arc<TeamEventStoreImpl> {
    let dir = tempfile::tempdir().expect("test tempdir");
    let path = dir.keep().join("team-events.pgno");
    Arc::new(TeamEventStoreImpl::create_pgno(&path).expect("create test pardosa team store"))
}

#[cfg(test)]
fn noop_scheduler_event_store() -> Arc<SchedulerEventStoreImpl> {
    Arc::new(scheduler_event_store())
}

#[cfg(test)]
fn noop_sweep_timeout_event_store() -> Arc<SweepTimeoutEventStoreImpl> {
    Arc::new(sweep_timeout_event_store())
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
    /// Panics if the unique tempdir-based noop pardosa store cannot be
    /// created. This is an infrastructure-level failure (disk full,
    /// permissions, no `/tmp`) at startup of a test path; halting is
    /// appropriate.
    pub async fn new() -> Arc<Self> {
        let event_store = noop_event_store().await;
        let org_event_store = noop_org_event_store().await;
        let team_event_store = noop_team_event_store().await;
        let scheduler_event_store = noop_scheduler_event_store();
        let sweep_timeout_event_store = noop_sweep_timeout_event_store();
        let projection_state =
            Arc::new(Mutex::new(crate::projection::EvidenceProjection::default()));
        Arc::new(Self {
            live_observation: tokio::sync::Mutex::new(0),
            started_at: Timestamp::now(),
            owner_id: uuid::Uuid::now_v7(),
            current_run: ArcSwap::from_pointee(None),
            last_completed_run: ArcSwap::from_pointee(None),
            last_recovery: ArcSwap::from_pointee(None),
            work_queue: Arc::new(WorkQueue::new(crate::config::WORK_QUEUE_CAPACITY)),
            worker_pool_started: tokio::sync::OnceCell::new(),
            worker_pool_cancel: WorkerShutdownToken::new(),
            run_fence: Mutex::new(None),
            event_store,
            org_event_store,
            team_event_store,
            scheduler_event_store,
            sweep_timeout_event_store,
            projection_state,
            webhook: WebhookState::from_environment(),
            github: GithubState::new(),
            evidence: EvidenceState::new(),
            sweep_lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }
}

impl AppState {
    /// Create a new `AppState` wired with both stores.
    ///
    /// Constructs a native [`NativeStore`](crate::store::NativeStore) over
    /// `<events_dir>/events.pgno` and rebuilds projection state from the
    /// event journal.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] when the selected pardosa backend
    /// cannot be opened or created. For `Pgno`, this includes creating
    /// `<events_dir>` or opening/creating `events.pgno`; for `Nats`,
    /// callers must use [`Self::with_stores_and_runtime`] providing a
    /// root-supervised runtime.
    pub async fn with_stores(
        events_dir: &Path,
        backend: crate::config::runtime::PardosaBackend,
        nats: crate::config::runtime::NatsStoreConfig,
    ) -> Result<Arc<Self>, std::io::Error> {
        if backend == crate::config::runtime::PardosaBackend::Nats {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "NATS event store requires a root-supervised Tokio runtime; use with_stores_and_runtime with Some(Arc<Runtime>)",
            ));
        }
        Self::with_stores_and_runtime(events_dir, backend, nats, None).await
    }

    /// Create a new `AppState` wired with both stores and an optional root-supervised runtime.
    ///
    /// When `backend` is [`PardosaBackend::Nats`](crate::config::runtime::PardosaBackend::Nats),
    /// `nats_runtime` must be `Some`. The caller retains the obligation to maintain an outer
    /// synchronous handle to this `Arc<Runtime>` that outlives this `AppState`, all worker
    /// tasks, and all store adapters until shutdown is complete.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] when opening or creating event stores fails, or
    /// when `backend` is [`PardosaBackend::Nats`](crate::config::runtime::PardosaBackend::Nats)
    /// without providing `nats_runtime`.
    pub async fn with_stores_and_runtime(
        events_dir: &Path,
        backend: crate::config::runtime::PardosaBackend,
        nats: crate::config::runtime::NatsStoreConfig,
        nats_runtime: Option<Arc<tokio::runtime::Runtime>>,
    ) -> Result<Arc<Self>, std::io::Error> {
        if backend == crate::config::runtime::PardosaBackend::Nats && nats_runtime.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "NATS event store requires a root-supervised Tokio runtime; nats_runtime cannot be None",
            ));
        }
        let handle = tokio::runtime::Handle::current();
        let events_dir = events_dir.to_path_buf();
        let event_store = open_event_store_blocking(
            events_dir.clone(),
            backend,
            nats.clone(),
            handle.clone(),
            nats_runtime.clone(),
        )
        .await?;
        let org_event_store = open_org_event_store_blocking(
            events_dir.clone(),
            backend,
            nats.org_events(),
            handle.clone(),
            nats_runtime.clone(),
        )
        .await?;
        let team_event_store = open_team_event_store_blocking(
            events_dir.clone(),
            backend,
            nats.team_events(),
            handle,
            nats_runtime,
        )
        .await?;
        let event_store = Arc::new(event_store);
        let org_event_store = Arc::new(org_event_store);
        let team_event_store = Arc::new(team_event_store);
        let scheduler_event_store = Arc::new(scheduler_event_store());
        let sweep_timeout_event_store = Arc::new(sweep_timeout_event_store());
        let last_recovery = None;
        let projection_state = Arc::new(Mutex::new(projection_from_stores(
            event_store.as_ref(),
            org_event_store.as_ref(),
            team_event_store.as_ref(),
        )?));
        Ok(Arc::new(Self {
            live_observation: tokio::sync::Mutex::new(0),
            started_at: Timestamp::now(),
            owner_id: uuid::Uuid::now_v7(),
            current_run: ArcSwap::from_pointee(None),
            last_completed_run: ArcSwap::from_pointee(None),
            last_recovery: ArcSwap::from_pointee(last_recovery),
            work_queue: Arc::new(WorkQueue::new(crate::config::WORK_QUEUE_CAPACITY)),
            worker_pool_started: tokio::sync::OnceCell::new(),
            worker_pool_cancel: WorkerShutdownToken::new(),
            run_fence: Mutex::new(None),
            event_store,
            org_event_store,
            team_event_store,
            scheduler_event_store,
            sweep_timeout_event_store,
            projection_state,
            webhook: WebhookState::from_environment(),
            github: GithubState::new(),
            evidence: EvidenceState::new(),
            sweep_lock: Arc::new(tokio::sync::Mutex::new(())),
        }))
    }
}

impl AppState {
    /// Re-seed [`Self::event_store`], [`Self::org_event_store`], and
    /// [`Self::team_event_store`] from a fresh authoritative read,
    /// discarding each store's stale in-memory fence-sequence cache in
    /// place.
    ///
    /// Design-Y consumer-owned re-arm (ghr-fea8b799): `PersistenceError::FencedConflict`
    /// can originate from any of the three long-lived native stores —
    /// `record_repo`/`remove_repo`, `record_org`, and `record_team` all map
    /// through the same generic catch-all (`native_store_persistence`) —
    /// so the daemon must re-seed all three before re-arming the collection
    /// run, not just the repositories store. Resyncing a store that did not
    /// actually fence is a harmless no-op fresh read; the alternative
    /// (resyncing only one store) would leave the *other* two stores stale,
    /// relocating rather than eliminating the R10-forbidden
    /// patch-cached-seq-and-redrive shape.
    ///
    /// Each store re-reads through the backend handle it was opened with, so a
    /// NATS-backed store always recovers from its configured NATS stream and a
    /// `.pgno`-backed store from its own file. The caller cannot select the
    /// recovery source; `_events_dir`, `_backend` and `_nats` are retained only
    /// to keep the existing call shape.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] when a store cannot be re-read, decoded, or
    /// schema-verified. Each store validates and decodes fully before it
    /// replaces its own cache, so a failing store keeps its previous cache.
    /// This is per-store preservation, not a three-store transaction: the
    /// stores are refreshed in repo, org, team order, the first failure
    /// returns immediately, and partial progress is permitted and expected —
    /// stores already refreshed stay refreshed, and stores after the failing
    /// one are never attempted. The caller must treat an `Err` as a failed
    /// re-arm and must not resume useful work on it.
    pub(crate) async fn resync_event_store(
        &self,
        _events_dir: &Path,
        _backend: crate::config::runtime::PardosaBackend,
        _nats: crate::config::runtime::NatsStoreConfig,
    ) -> Result<(), std::io::Error> {
        self.ensure_write_admission()
            .map_err(std::io::Error::other)?;
        let event_store = Arc::clone(&self.event_store);
        let org_event_store = Arc::clone(&self.org_event_store);
        let team_event_store = Arc::clone(&self.team_event_store);
        tokio::task::spawn_blocking(move || {
            event_store.resync_from_authoritative()?;
            org_event_store.resync_from_authoritative()?;
            team_event_store.resync_from_authoritative()
        })
        .await
        .map_err(std::io::Error::other)?
        .map_err(std::io::Error::other)
    }
}

impl AppState {
    /// Rebuild the in-memory projection from the native event journal.
    ///
    /// # Errors
    ///
    /// Returns an infrastructure error when the native store cannot replay.
    pub fn snapshot_fast_path_init(&self) -> Result<bool, std::io::Error> {
        self.refresh_projection()?;
        Ok(true)
    }

    fn refresh_projection(&self) -> Result<(), std::io::Error> {
        let projection = projection_from_stores(
            self.event_store.as_ref(),
            self.org_event_store.as_ref(),
            self.team_event_store.as_ref(),
        )?;
        replace_projection_state(&self.projection_state, projection);
        Ok(())
    }

    fn fold_repository_event_into_projection(&self, detached: bool, event: &NativeDomainEvent) {
        let mut guard = lock_projection_state(&self.projection_state);
        fold_native_event(&mut guard, detached, event);
    }

    fn fold_org_event_into_projection(&self, event: OrgStateCaptured) {
        let mut guard = lock_projection_state(&self.projection_state);
        fold_org_event(&mut guard, event);
    }

    /// Fold one team roster event into the resident projection
    /// (CHE-0089:R4), keyed by `team_domain_key`. `detached = true`
    /// removes the roster (CHE-0073:R7 detached-remove); otherwise
    /// upserts the latest snapshot (non-detached-upsert). Called by
    /// [`Self::record_team`] / [`Self::detach_team`] (P3, ghr-3fda2878)
    /// after the durable write lands, so the resident projection
    /// reflects the write at runtime, not only on next restart.
    pub(crate) fn fold_team_event_into_projection(&self, detached: bool, event: TeamStateCaptured) {
        let mut guard = lock_projection_state(&self.projection_state);
        apply_projection_event(&mut *guard, team_projection_event(detached, event));
    }

    /// Record a freshly-fetched team roster on its own per-team fiber
    /// (CHE-0089), keyed by `team_domain_key(org, team_slug)`. OCC fence
    /// mirrors [`Self::record_org`] (PGN-0016:R1/R2/R10): a fence conflict
    /// surfaces as [`PersistenceError::FencedConflict`] with no in-band
    /// retry — the caller (the team-refresh writer, via
    /// `write_with_policy`) treats it as fatal (CHE-0088). Persist-then-
    /// publish (CHE-0024:R1): the projection is folded only after the
    /// store append succeeds.
    ///
    /// The two appends are ordered, not atomic (ghr-7qm12.16). A failed
    /// primary append short-circuits: the dedicated append and the
    /// projection fold are both skipped. A failed dedicated append is
    /// returned to the caller with the already-durable primary append
    /// retained — no rollback is performed or claimed — and no
    /// projection fold.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when native conversion, the primary
    /// store append, or the dedicated store append fails.
    ///
    /// # Panics
    ///
    /// Panics if `team_state_event` ever produces an empty `org` or
    /// `team_slug` — unreachable, since `team_state_event` rejects both
    /// as empty via [`non_empty`] before constructing the event.
    pub fn record_team(
        &self,
        org: &str,
        roster: &crate::domain::metrics::TeamRoster,
        fetched_at: &str,
        org_membership_fetch_status: OrgMembershipFetchStatus,
    ) -> Result<(), PersistenceError> {
        self.admitted_write(|primary_landed| {
            let event = team_state_event(org, roster, fetched_at, org_membership_fetch_status)?;
            let team_key = team_domain_key(event.org.as_str(), event.team_slug.as_str())
                .expect("team_state_event never produces empty org/team_slug");
            self.event_store
                .record(
                    &team_key,
                    NativeDomainEvent::TeamStateCaptured(event.clone()),
                )
                .map_err(native_store_persistence)?;
            *primary_landed = true;
            self.team_event_store
                .record(&team_key, event.clone())
                .map_err(native_store_persistence)?;
            self.fold_team_event_into_projection(false, event);
            Ok(())
        })
    }

    /// Soft-delete a team's fiber (detach) for a team that no longer
    /// exists on GitHub, or no longer owns any repository via CODEOWNERS,
    /// then fold the removal into the resident projection at runtime
    /// (linus's P4 back-brief: detaching the store alone leaves the
    /// projection stale until restart). Same OCC fence and
    /// persist-then-publish ordering as [`Self::record_team`].
    ///
    /// The two detaches are ordered, not atomic (ghr-7qm12.37). A failed
    /// primary detach short-circuits: the dedicated detach and the
    /// projection removal are both skipped. A failed dedicated detach is
    /// returned to the caller with the already-durable primary detach
    /// retained — no rollback is performed or claimed — and no projection
    /// removal.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when native conversion, the primary
    /// store detach, or the dedicated store detach fails.
    ///
    /// # Panics
    ///
    /// Panics if `team_state_event` ever produces an empty `org` or
    /// `team_slug` — unreachable, since `team_state_event` rejects both
    /// as empty via [`non_empty`] before constructing the event.
    pub fn detach_team(
        &self,
        org: &str,
        roster: &crate::domain::metrics::TeamRoster,
        fetched_at: &str,
        org_membership_fetch_status: OrgMembershipFetchStatus,
    ) -> Result<(), PersistenceError> {
        self.admitted_write(|primary_landed| {
            let tombstone = detach_tombstone_roster(roster);
            let event = team_state_event(org, &tombstone, fetched_at, org_membership_fetch_status)?;
            let team_key = team_domain_key(event.org.as_str(), event.team_slug.as_str())
                .expect("team_state_event never produces empty org/team_slug");
            *primary_landed = self
                .event_store
                .detach_effect(
                    &team_key,
                    NativeDomainEvent::TeamStateCaptured(event.clone()),
                )
                .map_err(native_store_persistence)?;
            self.team_event_store
                .detach(&team_key, event.clone())
                .map_err(native_store_persistence)?;
            self.fold_team_event_into_projection(true, event);
            Ok(())
        })
    }

    /// Record a live repository snapshot in the native store.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when native conversion or store append
    /// fails.
    pub fn record_repo(
        &self,
        domain_key: &str,
        evidence: RepositoryEvidence,
        repo_name: &str,
        timestamp: &str,
    ) -> Result<(), PersistenceError> {
        self.admitted_write(|_| {
            let native_evidence = crate::event::RepositoryEvidence::try_from(evidence)
                .map_err(|e| conversion_persistence(&e))?;
            let event = repo_event(domain_key, repo_name, timestamp, Some(native_evidence))?;
            self.event_store
                .record(domain_key, event.clone())
                .map_err(native_store_persistence)?;
            self.fold_repository_event_into_projection(false, &event);
            Ok(())
        })
    }

    /// Soft-delete a repository fiber in the native store.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when event construction or store detach
    /// fails.
    pub fn remove_repo(
        &self,
        domain_key: &str,
        repo_name: &str,
        timestamp: &str,
    ) -> Result<(), PersistenceError> {
        self.admitted_write(|_| {
            let event = repo_event(domain_key, repo_name, timestamp, None)?;
            self.event_store
                .detach(domain_key, event.clone())
                .map_err(native_store_persistence)?;
            self.fold_repository_event_into_projection(true, &event);
            Ok(())
        })
    }

    /// Record a repository deleted by successful inventory reconciliation.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when event construction or store append fails.
    pub fn mark_repo_deleted(
        &self,
        domain_key: &str,
        repo_name: &str,
        detected_at: &str,
    ) -> Result<(), PersistenceError> {
        self.admitted_write(|_| {
            let event = deleted_repo_event(domain_key, repo_name, detected_at)?;
            self.event_store
                .record(domain_key, event.clone())
                .map_err(native_store_persistence)?;
            self.fold_repository_event_into_projection(false, &event);
            Ok(())
        })
    }

    /// Record a live org snapshot in the native org store.
    ///
    /// The two appends are ordered, not atomic (ghr-7qm12.37). A failed
    /// primary append short-circuits: the dedicated append and the
    /// projection fold are both skipped. A failed dedicated append is
    /// returned to the caller with the already-durable primary append
    /// retained — no rollback is performed or claimed — and no projection
    /// fold.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when native conversion, the primary
    /// store append, or the dedicated store append fails.
    pub fn record_org(
        &self,
        snapshot: crate::domain::evidence::OrgStateSnapshot,
    ) -> Result<(), PersistenceError> {
        self.admitted_write(|primary_landed| {
            let event =
                OrgStateCaptured::try_from(snapshot).map_err(|e| conversion_persistence(&e))?;
            let org_key = event.assessment_metadata.organization.as_str().to_string();
            self.event_store
                .record(&org_key, NativeDomainEvent::OrgStateCaptured(event.clone()))
                .map_err(native_store_persistence)?;
            *primary_landed = true;
            self.org_event_store
                .record(&org_key, event.clone())
                .map_err(native_store_persistence)?;
            self.fold_org_event_into_projection(event);
            Ok(())
        })
    }
}

/// Construct the primary-rate `Regulator` for the worker-pool regulator
/// chain by matching `kind` to a concrete constructor (ghr-79f5d695
/// kill-switch). Each arm builds a concrete `cherry-pit-wq` type coerced
/// into the existing `Arc<dyn Regulator>` chain slot; no erased selector
/// indirection is introduced.
fn build_primary_rate_regulator(
    kind: crate::config::runtime::RateRegulatorKind,
    budget: &Arc<crate::github::budget::BudgetGate>,
) -> Arc<dyn crate::app::worker_pool::Regulator> {
    match kind {
        crate::config::runtime::RateRegulatorKind::TokenBucket => {
            let refill_tokens_per_sec =
                (crate::config::API_BUDGET_LIMIT / crate::config::API_BUDGET_WAIT_SECS).max(1);
            Arc::new(crate::app::worker_pool::TokenBucketRegulator::new(
                Arc::new(crate::app::worker_pool::SystemClock),
                crate::config::API_BUDGET_LIMIT,
                refill_tokens_per_sec,
            )) as Arc<dyn crate::app::worker_pool::Regulator>
        }
        crate::config::runtime::RateRegulatorKind::BudgetGate => Arc::new(
            crate::app::worker_pool::BudgetRegulator::new(Arc::clone(budget)),
        )
            as Arc<dyn crate::app::worker_pool::Regulator>,
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
    pub(crate) async fn ensure_worker_pool(
        self: &Arc<Self>,
        rate_regulator: crate::config::runtime::RateRegulatorKind,
    ) -> bool {
        let state = Arc::clone(self);
        let mut started_now = false;

        self.worker_pool_started
            .get_or_init(|| async {
                started_now = true;

                let client = state
                    .github_client()
                    .expect("ensure_worker_pool called before github_client initialized")
                    .clone();
                let backoff = Arc::clone(&client.backoff);

                let evaluator =
                    Arc::new(crate::app::collect::LiveEvaluator::with_shared_org_summary(
                        client,
                        Arc::clone(&state.evidence.org_summary),
                    ));

                let queue = Arc::clone(&state.work_queue);
                let (budget, rate_limit) = state.github_api_controls();
                let cancel = state.worker_shutdown_token();

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

                let regulators: std::sync::Arc<
                    [std::sync::Arc<dyn crate::app::worker_pool::Regulator>],
                > = std::sync::Arc::from(vec![
                    build_primary_rate_regulator(rate_regulator, &budget),
                    std::sync::Arc::new(crate::app::worker_pool::RateLimitRegulator::new(
                        rate_limit,
                    ))
                        as std::sync::Arc<dyn crate::app::worker_pool::Regulator>,
                    Arc::clone(&backoff) as std::sync::Arc<dyn crate::app::worker_pool::Regulator>,
                ]);

                let pool_handle = tokio::spawn(async move {
                    crate::app::worker_pool::run_worker_pool_regulated(
                        queue,
                        evaluator,
                        regulators,
                        crate::app::worker_pool::WorkerPoolConfig::default(),
                        cancel,
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
    /// `OnceCell` (if any were ever started) and `await` them concurrently
    /// with one timeout budget, aborting either handle whose timeout elapses.
    /// Returns the pair of `(pool_drained, delivery_drained)` booleans
    /// where `true` means the task exited cleanly within the timeout.
    /// Caller logs the outcome.
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
        let (pool_ok, delivery_ok) = tokio::join!(
            drain_join_handle_or_abort(pool_handle, per_handle_timeout),
            drain_join_handle_or_abort(delivery_handle, per_handle_timeout),
        );
        (pool_ok, delivery_ok)
    }

    pub(crate) fn worker_shutdown_token(&self) -> WorkerShutdownToken {
        self.worker_pool_cancel.clone()
    }

    pub(crate) fn cancel_worker_pool(&self) {
        self.worker_pool_cancel.cancel();
    }
}

async fn drain_join_handle_or_abort(
    handle: tokio::task::JoinHandle<()>,
    timeout: std::time::Duration,
) -> bool {
    let abort_handle = handle.abort_handle();
    let drained = tokio::time::timeout(timeout, handle).await.is_ok();
    if !drained {
        abort_handle.abort();
    }
    drained
}

/// Read resident set size in kilobytes from `/proc/self/status` (`VmRSS:`).
///
/// Linux-only self-read; `std::fs` only, no unsafe. Returns `None` on
/// other platforms so callers can treat the value as a uniformly
/// optional gauge.
#[cfg(target_os = "linux")]
pub(crate) fn read_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let value = line.strip_prefix("VmRSS:")?;
        value.split_whitespace().next()?.parse::<u64>().ok()
    })
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn read_rss_kb() -> Option<u64> {
    None
}

impl AppState {
    /// Build the JSON payload for the `/api/v1/status` endpoint.
    ///
    /// Returns current and last completed run metadata, uptime, and
    /// memory gauges (`rss_kb`, `projection_repo_count`,
    /// `projection_bytes_est`). `projection_bytes_est` is a shallow
    /// struct-size floor (`size_of::<RepositoryEvidence>() * count`)
    /// excluding heap-owned `String`/`Vec` data — not measured RSS.
    /// The heap-inclusive sample ([`Self::projection_bytes_deep`]) is
    /// deliberately NOT computed here — it is `O(n)` and only sampled
    /// on the collection-tick log, keeping this per-request payload
    /// clone-free and cheap.
    /// Registered as an extra route in [`crate::server::status_router`],
    /// not as a built-in route of the generic server module.
    pub(crate) fn status_payload(&self) -> serde_json::Value {
        let current = self.current_run.load();
        let last = self.last_completed_run.load();
        let last_recovery = self.last_recovery.load();
        let uptime_duration = Timestamp::now().duration_since(self.started_at);
        let uptime = u64::try_from(uptime_duration.as_secs().max(0)).unwrap_or(0);
        let projection_repo_count = self.projection_len();
        let projection_bytes_est = projection_repo_count
            * std::mem::size_of::<crate::domain::evidence::RepositoryEvidence>();

        let (coverage_provenance, coverage_obj) = if let Some(curr) = current.as_ref().as_ref() {
            if curr.coverage.is_known() {
                ("current_run", curr.coverage)
            } else {
                (
                    "unknown",
                    crate::domain::evidence::CollectionCoverage::Unknown,
                )
            }
        } else if let Some(completed) = last.as_ref().as_ref() {
            if completed.coverage.is_known() {
                ("last_completed_run", completed.coverage)
            } else {
                (
                    "unknown",
                    crate::domain::evidence::CollectionCoverage::Unknown,
                )
            }
        } else if let Some(durable_cov) = self
            .projection_org_coverage()
            .filter(crate::domain::evidence::CollectionCoverage::is_known)
        {
            ("durable_assessment", durable_cov)
        } else {
            (
                "unknown",
                crate::domain::evidence::CollectionCoverage::Unknown,
            )
        };

        serde_json::json!({
            "current_run": current.as_ref(),
            "last_completed_run": last.as_ref(),
            "last_recovery": last_recovery.as_ref(),
            "uptime_secs": uptime,
            "rss_kb": read_rss_kb(),
            "projection_repo_count": projection_repo_count,
            "projection_bytes_est": projection_bytes_est,
            "coverage": {
                "provenance": coverage_provenance,
                "status": if coverage_obj.is_unknown() { "unknown" } else { "known" },
                "capped": coverage_obj.is_capped(),
                "selected": coverage_obj.selected(),
                "total": coverage_obj.total(),
                "limit": coverage_obj.limit().map(std::num::NonZero::get),
            },
            "coverage_capped": coverage_obj.is_capped(),
            "selected_repos": coverage_obj.selected(),
            "total_repos": coverage_obj.total(),
            "cap_limit": coverage_obj.limit().map(std::num::NonZero::get),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::runtime::{NatsStoreConfig, PardosaBackend};
    use crate::domain::cache::CachedRepoDetail;
    use crate::domain::evidence::Evidence;
    use cherry_pit_web::serve::ServerState;
    use std::fmt::Write as _;
    use std::sync::Arc;
    use tracing::field::{Field, Visit};
    use tracing_subscriber::layer::{Context, SubscriberExt};

    #[test]
    fn projection_capture_overflow_is_explicit_and_never_wraps() {
        let sequence = std::sync::atomic::AtomicU64::new(u64::MAX - 1);
        assert_eq!(next_render_capture(&sequence).unwrap(), u64::MAX);
        assert!(
            next_render_capture(&sequence)
                .unwrap_err()
                .to_string()
                .contains("exhausted")
        );
        assert!(next_render_capture(&sequence).is_err());
        assert_eq!(
            sequence.load(std::sync::atomic::Ordering::Relaxed),
            u64::MAX
        );
    }

    #[tokio::test]
    async fn primary_rate_regulator_defaults_to_token_bucket_and_conserves() {
        let budget = Arc::new(crate::github::budget::BudgetGate::new(
            crate::config::API_BUDGET_LIMIT,
            std::time::Duration::from_secs(crate::config::API_BUDGET_WAIT_SECS),
        ));
        let regulator = build_primary_rate_regulator(
            crate::config::runtime::RateRegulatorKind::TokenBucket,
            &budget,
        );
        let cancel = tokio_util::sync::CancellationToken::new();
        for _ in 0..crate::config::API_BUDGET_LIMIT.min(64) {
            assert_eq!(
                regulator.admit(&cancel).await,
                crate::app::worker_pool::Admission::Admitted
            );
            regulator.settle(crate::app::worker_pool::SettleOutcome::Charged);
        }
    }

    #[tokio::test]
    async fn primary_rate_regulator_selects_budget_gate_and_conserves() {
        let budget = Arc::new(crate::github::budget::BudgetGate::new(
            2,
            std::time::Duration::from_hours(1),
        ));
        let regulator = build_primary_rate_regulator(
            crate::config::runtime::RateRegulatorKind::BudgetGate,
            &budget,
        );
        let cancel = tokio_util::sync::CancellationToken::new();
        for _ in 0..2 {
            assert_eq!(
                regulator.admit(&cancel).await,
                crate::app::worker_pool::Admission::Admitted
            );
            regulator.settle(crate::app::worker_pool::SettleOutcome::Charged);
        }
        assert_eq!(budget.total_calls_made(), 2);
    }

    fn empty_org_summary() -> crate::domain::metrics::OrgAlertSummary {
        crate::domain::metrics::OrgAlertSummary {
            collection_status: crate::domain::status::CollectionStatus::Success,
            collection_reason: None,
            http_status: Some(200),
            per_repo: HashMap::new(),
            open_secret_alert_age_buckets: crate::config::empty_age_buckets(),
            total_open_secret_alerts: 0,
            oldest_open_secret_alert_created_at: None,
            newest_open_secret_alert_created_at: None,
        }
    }

    fn fold_public_event_stream(
        events: Vec<(bool, NativeDomainEvent)>,
    ) -> crate::projection::EvidenceProjection {
        let mut projection = crate::projection::EvidenceProjection::default();
        for (detached, event) in events {
            fold_native_event(&mut projection, detached, &event);
        }
        projection
    }

    fn repository_deleted_event(
        domain_key: &str,
        repo_name: &str,
        detected_at: &str,
    ) -> NativeDomainEvent {
        NativeDomainEvent::RepositoryDeleted {
            domain_key: NonEmptyEventString::new(domain_key).expect("domain key fits"),
            repo_name: NonEmptyEventString::new(repo_name).expect("repo name fits"),
            detected_at: event_timestamp("detected_at", detected_at).expect("timestamp fits"),
        }
    }

    fn rendered_evidence_from_projection(
        repositories: Vec<crate::domain::evidence::RepositoryEvidence>,
    ) -> Evidence {
        let repository_count = u32::try_from(repositories.len()).expect("test repo count fits u32");
        Evidence {
            assessment_metadata: crate::test_fixtures::make_metadata(),
            collection_statistics: crate::test_fixtures::make_collection_statistics(
                repository_count,
                repository_count,
                0,
                0,
            ),
            metrics: crate::test_fixtures::make_minimal_metrics(),
            secret_scanning_observability: crate::test_fixtures::make_observability(),
            repositories,
            deleted: vec![],
        }
    }

    struct CapturedEvents {
        lines: Arc<Mutex<Vec<String>>>,
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CapturedEvents {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            let mut visitor = CapturedFields::default();
            event.record(&mut visitor);
            self.lines
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(visitor.line);
        }
    }

    #[derive(Default)]
    struct CapturedFields {
        line: String,
    }

    impl CapturedFields {
        fn push(&mut self, field: &Field, value: impl std::fmt::Display) {
            write!(&mut self.line, "{}={value};", field.name()).expect("string write succeeds");
        }
    }

    impl Visit for CapturedFields {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.push(field, format_args!("{value:?}"));
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            self.push(field, value);
        }

        fn record_bool(&mut self, field: &Field, value: bool) {
            self.push(field, value);
        }

        fn record_u64(&mut self, field: &Field, value: u64) {
            self.push(field, value);
        }
    }

    fn capture_events(f: impl FnOnce()) -> String {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let layer = CapturedEvents {
            lines: Arc::clone(&lines),
        };
        let subscriber = tracing_subscriber::Registry::default().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::callsite::rebuild_interest_cache();
            f();
            tracing::callsite::rebuild_interest_cache();
        });
        lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .join("\n")
    }

    #[test]
    fn repository_deleted_fold_moves_to_deleted_and_live_snapshot_resurrects() {
        let timestamp = "2026-06-24T00:00:00Z";
        let evidence = crate::test_fixtures::all_passing_evidence("deleted-then-live");
        let domain_key = evidence.repository.inventory_key.clone();
        let repo_name = evidence.repository.name.clone();
        let live_event = repo_event(
            &domain_key,
            &repo_name,
            timestamp,
            Some(crate::event::RepositoryEvidence::try_from(evidence).expect("event evidence")),
        )
        .expect("live event");
        let deleted_event = repository_deleted_event(&domain_key, &repo_name, timestamp);

        let deleted_projection =
            fold_public_event_stream(vec![(false, live_event.clone()), (false, deleted_event)]);
        assert!(!deleted_projection.repositories.contains_key(&domain_key));
        let deleted = deleted_projection
            .deleted
            .get(&domain_key)
            .expect("deleted record");
        assert_eq!(deleted.repo_name, repo_name);
        assert_eq!(deleted.detected_at, timestamp);

        let resurrected_projection =
            fold_public_event_stream(vec![(false, live_event.clone()), (false, live_event)]);
        assert!(
            resurrected_projection
                .repositories
                .contains_key(&domain_key)
        );
        assert!(!resurrected_projection.deleted.contains_key(&domain_key));
    }

    #[test]
    fn nats_open_dead_port_returns_error_without_nested_runtime_panic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let events_dir = tmp.path().join("events");
        let nats = NatsStoreConfig::for_org("org", "nats://127.0.0.1:1").unwrap();

        let nats_runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
        );
        let root_guard = Arc::clone(&nats_runtime);

        let app_runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("app runtime");

        app_runtime.block_on(async move {
            let Err(unmanaged_err) =
                AppState::with_stores(&events_dir, PardosaBackend::Nats, nats.clone()).await
            else {
                panic!("with_stores without supervised runtime must fail closed");
            };
            assert_eq!(unmanaged_err.kind(), std::io::ErrorKind::InvalidInput);
            assert_eq!(
                unmanaged_err.to_string(),
                "NATS event store requires a root-supervised Tokio runtime; use with_stores_and_runtime with Some(Arc<Runtime>)"
            );

            let Err(overload_none_err) = AppState::with_stores_and_runtime(
                &events_dir,
                PardosaBackend::Nats,
                nats.clone(),
                None,
            )
            .await
            else {
                panic!("with_stores_and_runtime with None runtime must fail closed");
            };
            assert_eq!(overload_none_err.kind(), std::io::ErrorKind::InvalidInput);
            assert_eq!(
                overload_none_err.to_string(),
                "NATS event store requires a root-supervised Tokio runtime; nats_runtime cannot be None"
            );

            let result = AppState::with_stores_and_runtime(
                &events_dir,
                PardosaBackend::Nats,
                nats,
                Some(root_guard),
            )
            .await;

            let error = match result {
                Ok(_) => panic!("dead-port Nats open must fail"),
                Err(error) => error.to_string(),
            };
            assert!(
                !error.contains("Cannot start a runtime from within a runtime"),
                "Nats open must return a typed connect error, not panic with nested-runtime failure: {error}"
            );
            assert!(
                error.contains("connect")
                    || error.contains("Connection")
                    || error.contains("refused"),
                "dead-port Nats open should reach connect and surface it as io::Error, got: {error}"
            );
        });

        drop(app_runtime);
        drop(nats_runtime);
    }

    #[test]
    fn nats_app_state_partial_initialization_failure_drops_safely_with_root_owner() {
        use pardosa::prelude::*;
        let Some(server) = crate::store::tests::TestNatsServer::spawn() else {
            return;
        };
        let url = server.url.clone();
        let tmp = tempfile::tempdir().expect("tempdir");
        let events_dir = tmp.path().join("events");

        let nats_runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("root test runtime"),
        );

        let nats_config = NatsStoreConfig::for_org("test-partial-init", &url).unwrap();
        let org_nats = nats_config.org_events();

        {
            let url = url.clone();
            let nats_runtime = Arc::clone(&nats_runtime);
            let org_stream_name = org_nats.stream_name.clone();
            let (meta_subj, data_subj) = nats_subjects_for_stem(&org_stream_name);
            let client = nats_runtime
                .block_on(async_nats::connect(&url))
                .expect("connect");
            let org_adapter = pardosa_nats::NatsStorageAdapter::from_client_with_runtime(
                client,
                org_stream_name,
                Arc::clone(&nats_runtime),
            )
            .with_subjects_for_test(meta_subj, data_subj);
            let claim = OwnershipClaimRecord {
                epoch: 1,
                machine_id: [0u8; 16],
                boot_id: [0u8; 16],
                process_id: u64::from(std::process::id()),
                process_start_time_ns: 0,
                claim_time_ns: 0,
                operator_label: "mismatched-partial-org".to_string(),
            };
            let mismatched_desc = SchemaDescriptor::new(999, DescriptorNode::U64);
            let admitted = AdmittedDescriptor::try_from_descriptor(mismatched_desc).unwrap();
            let mut session = org_adapter
                .create(&claim, &admitted)
                .expect("create org store");
            session.sync().expect("sync");
        }

        let app_runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("app test runtime");

        let root_nats_guard = Arc::clone(&nats_runtime);

        app_runtime.block_on(async move {
            assert_eq!(
                tokio::runtime::Handle::current().runtime_flavor(),
                tokio::runtime::RuntimeFlavor::MultiThread
            );
            let result = AppState::with_stores_and_runtime(
                &events_dir,
                PardosaBackend::Nats,
                nats_config,
                Some(root_nats_guard),
            )
            .await;

            let Err(err) = result else {
                panic!("partial initialization with mismatched org stream must fail");
            };
            assert!(
                err.to_string()
                    .contains("schema descriptor identity mismatch")
            );
        });

        drop(app_runtime);
        drop(nats_runtime);
    }

    #[test]
    fn nats_app_state_partial_initialization_failure_on_team_drops_safely_with_root_owner() {
        use pardosa::prelude::*;
        let Some(server) = crate::store::tests::TestNatsServer::spawn() else {
            return;
        };
        let url = server.url.clone();
        let tmp = tempfile::tempdir().expect("tempdir");
        let events_dir = tmp.path().join("events");

        let nats_runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("root test runtime"),
        );

        let nats_config = NatsStoreConfig::for_org("test-partial-init-team", &url).unwrap();
        let team_nats = nats_config.team_events();

        {
            let url = url.clone();
            let nats_runtime = Arc::clone(&nats_runtime);
            let team_stream_name = team_nats.stream_name.clone();
            let (meta_subj, data_subj) = nats_subjects_for_stem(&team_stream_name);
            let client = nats_runtime
                .block_on(async_nats::connect(&url))
                .expect("connect");
            let team_adapter = pardosa_nats::NatsStorageAdapter::from_client_with_runtime(
                client,
                team_stream_name,
                Arc::clone(&nats_runtime),
            )
            .with_subjects_for_test(meta_subj, data_subj);
            let claim = OwnershipClaimRecord {
                epoch: 1,
                machine_id: [0u8; 16],
                boot_id: [0u8; 16],
                process_id: u64::from(std::process::id()),
                process_start_time_ns: 0,
                claim_time_ns: 0,
                operator_label: "mismatched-partial-team".to_string(),
            };
            let mismatched_desc = SchemaDescriptor::new(999, DescriptorNode::U64);
            let admitted = AdmittedDescriptor::try_from_descriptor(mismatched_desc).unwrap();
            let mut session = team_adapter
                .create(&claim, &admitted)
                .expect("create team store");
            session.sync().expect("sync");
        }

        let app_runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("app test runtime");

        let root_nats_guard = Arc::clone(&nats_runtime);

        app_runtime.block_on(async move {
            assert_eq!(
                tokio::runtime::Handle::current().runtime_flavor(),
                tokio::runtime::RuntimeFlavor::MultiThread
            );
            let result = AppState::with_stores_and_runtime(
                &events_dir,
                PardosaBackend::Nats,
                nats_config,
                Some(root_nats_guard),
            )
            .await;

            let Err(err) = result else {
                panic!("partial initialization with mismatched team stream must fail");
            };
            assert!(
                err.to_string()
                    .contains("schema descriptor identity mismatch")
            );
        });

        drop(app_runtime);
        drop(nats_runtime);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn real_store_create_record_drop_reopen_warm_html_and_status() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let events_dir = tmp.path().join("events");
        std::fs::create_dir_all(&events_dir).expect("mk events dir");

        let org = "test-durable-warm";
        let nats_config =
            NatsStoreConfig::for_org(org, crate::config::runtime::DEFAULT_NATS_URL).unwrap();

        {
            let app_state =
                AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats_config.clone())
                    .await
                    .expect("with_stores 1");

            let mut metadata = crate::test_fixtures::make_metadata();
            metadata.organization = org.to_string();
            metadata.coverage = crate::domain::evidence::CollectionCoverage::known(
                100,
                std::num::NonZeroU64::new(10).unwrap(),
            );

            let snapshot = crate::domain::evidence::OrgStateSnapshot {
                archived_repos: 20,
                assessment_metadata: metadata.clone(),
                alert_summary: empty_org_summary(),
            };

            let repo_evidence = crate::test_fixtures::all_passing_evidence("seed-repo");
            app_state
                .record_repo(
                    "id-seed-repo",
                    repo_evidence,
                    "seed-repo",
                    &metadata.run_timestamp,
                )
                .expect("record_repo");
            app_state.record_org(snapshot).expect("record_org");
        }

        {
            let app_state =
                AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats_config.clone())
                    .await
                    .expect("with_stores 2");

            app_state.snapshot_fast_path_init().expect("rehydrate");

            let status = app_state.status_payload();
            assert_eq!(status["coverage"]["status"], "known");
            assert_eq!(status["coverage"]["provenance"], "durable_assessment");
            assert_eq!(status["coverage"]["total"], 100);
            assert_eq!(status["coverage"]["limit"], 10);
            assert_eq!(status["coverage"]["selected"], 10);
            assert_eq!(status["coverage"]["capped"], true);

            {
                let projection_arc = app_state.projection_state_for_test();
                let projection = projection_arc.lock().expect("projection lock");
                let org_model = projection.org_state.as_ref().expect("org_state present");
                assert_eq!(org_model.archived_repos, 20);
                assert_eq!(
                    org_model.assessment_metadata.coverage,
                    crate::domain::evidence::CollectionCoverage::known(
                        100,
                        std::num::NonZeroU64::new(10).unwrap(),
                    )
                );
            }

            let config = test_warm_config(org, tmp.path());

            let ok = crate::app::collect::warm_start_from_baseline(&config, &app_state).await;
            assert!(
                ok,
                "warm_start_from_baseline must succeed from durable store"
            );

            let cache = app_state.html_cache().load_full();
            let pages = cache.as_ref().as_ref().expect("html cache populated");
            let index = &pages["index.html"];
            let bytes = index.body.identity_bytes().unwrap();
            let html = String::from_utf8_lossy(&bytes);
            assert!(html.contains("warm-start-badge"));
            assert!(html.contains("10 of 100 repositories selected"));
            assert!(html.contains("class=\"coverage-capped-badge\""));
            assert!(html.contains("100 total"));
            assert!(html.contains("20 archived repositories"));
        }
    }

    fn test_warm_config(
        org: &str,
        store_dir: &std::path::Path,
    ) -> crate::config::runtime::RuntimeConfig {
        crate::config::runtime::RuntimeConfig {
            org_name: org.to_string(),
            no_resume: true,
            max_workers: 4,
            store_dir: store_dir.to_path_buf(),
            pardosa_backend: crate::config::runtime::PardosaBackend::Pgno,
            nats_url: crate::config::runtime::DEFAULT_NATS_URL.to_string(),
            nats_creds: None,
            force_unlock: false,
            force_refresh: false,
            dashboard_config: crate::config::dashboard::DashboardConfig::default(),
            team_roster_read_from_projection: true,
            rate_regulator: crate::config::runtime::RateRegulatorKind::default(),
            sweep_timeout: crate::config::SweepTimeout::default(),
            max_repos: crate::config::MaxRepos::default(),
            nats_runtime: None,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn real_store_unknown_coverage_control_reopen() {
        let tmp_unk = tempfile::tempdir().expect("tempdir unk");
        let events_dir_unk = tmp_unk.path().join("events");
        std::fs::create_dir_all(&events_dir_unk).expect("mk events dir unk");
        let nats_config_unk =
            NatsStoreConfig::for_org("test-unk-control", crate::config::runtime::DEFAULT_NATS_URL)
                .unwrap();

        {
            let app_state = AppState::with_stores(
                &events_dir_unk,
                PardosaBackend::Pgno,
                nats_config_unk.clone(),
            )
            .await
            .expect("with_stores unk 1");

            let mut metadata = crate::test_fixtures::make_metadata();
            metadata.organization = "test-unk-control".to_string();
            metadata.coverage = crate::domain::evidence::CollectionCoverage::Unknown;

            let snapshot = crate::domain::evidence::OrgStateSnapshot {
                archived_repos: 5,
                assessment_metadata: metadata.clone(),
                alert_summary: empty_org_summary(),
            };

            let repo_evidence = crate::test_fixtures::all_passing_evidence("seed-repo-unk");
            app_state
                .record_repo(
                    "id-seed-repo-unk",
                    repo_evidence,
                    "seed-repo-unk",
                    &metadata.run_timestamp,
                )
                .expect("record_repo unk");
            app_state.record_org(snapshot).expect("record_org unk");
        }

        {
            let app_state =
                AppState::with_stores(&events_dir_unk, PardosaBackend::Pgno, nats_config_unk)
                    .await
                    .expect("with_stores unk 2");

            app_state.snapshot_fast_path_init().expect("rehydrate unk");

            let status = app_state.status_payload();
            assert_eq!(status["coverage"]["status"], "unknown");
            assert_eq!(status["coverage"]["provenance"], "unknown");
            assert!(status["coverage"]["total"].is_null());
            assert!(status["coverage"]["limit"].is_null());
            assert!(status["coverage"]["selected"].is_null());
            assert_eq!(status["coverage"]["capped"], false);

            {
                let projection_arc = app_state.projection_state_for_test();
                let projection = projection_arc.lock().expect("projection lock unk");
                let org_model = projection
                    .org_state
                    .as_ref()
                    .expect("org_state present unk");
                assert_eq!(
                    org_model.assessment_metadata.coverage,
                    crate::domain::evidence::CollectionCoverage::Unknown
                );
            }

            let config = test_warm_config("test-unk-control", tmp_unk.path());

            let ok = crate::app::collect::warm_start_from_baseline(&config, &app_state).await;
            assert!(
                ok,
                "warm_start_from_baseline must succeed for unknown coverage"
            );

            let cache = app_state.html_cache().load_full();
            let pages = cache.as_ref().as_ref().expect("html cache populated");
            let index = &pages["index.html"];
            let bytes = index.body.identity_bytes().unwrap();
            let html = String::from_utf8_lossy(&bytes);
            assert!(html.contains("warm-start-badge"));
            assert!(!html.contains("class=\"coverage-capped-badge\""));
        }
    }

    #[tokio::test]
    async fn v22_schema_descriptor_refusal_on_reopen() {
        use pardosa::prelude::*;
        let tmp = tempfile::tempdir().expect("tempdir");
        let events_dir = tmp.path().join("events");
        let org = "test-v22-refusal";
        let org_events_dir = events_dir.join(org);
        std::fs::create_dir_all(&org_events_dir).expect("mk events dir");

        let org_pgno_path = events_dir.join("org-events.pgno");
        let adapter = FileStorageAdapter::new(&org_pgno_path);
        let claim = OwnershipClaimRecord {
            epoch: 1,
            machine_id: [0u8; 16],
            boot_id: [0u8; 16],
            process_id: u64::from(std::process::id()),
            process_start_time_ns: 0,
            claim_time_ns: 0,
            operator_label: "v22-descriptor-seed".to_string(),
        };
        let v22_desc = crate::event::v22_org_state_descriptor();
        assert_eq!(
            v22_desc.identity().to_hex(),
            "2ec6b5d4f386afbe6a73fe4e0c962897e477316af3137591927bb18a31b4c38f",
            "must prove historical v22 descriptor identity before seeding store"
        );
        let admitted = AdmittedDescriptor::try_from_descriptor(v22_desc).expect("admit v22");
        let mut session = adapter.create(&claim, &admitted).expect("create org store");
        session.sync().expect("sync v22 descriptor");
        drop(session);

        let nats_config =
            NatsStoreConfig::for_org(org, crate::config::runtime::DEFAULT_NATS_URL).unwrap();
        let result = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats_config).await;

        let Err(err) = result else {
            panic!("reopening store with v22 schema descriptor must be refused");
        };
        assert!(
            err.to_string()
                .contains("schema descriptor identity mismatch"),
            "expected schema descriptor identity mismatch error, got: {err}"
        );
    }

    #[test]
    fn nats_subjects_for_stem_derives_native_subjects() {
        let (meta, data) = nats_subjects_for_stem("gh-report-org_4d617474696c73796e6574-v24");
        assert_eq!(meta, "gh-report-org_4d617474696c73796e6574-v24_meta");
        assert_eq!(data, "gh-report-org_4d617474696c73796e6574-v24_data");

        let (org_meta, org_data) =
            nats_subjects_for_stem("gh-report-org_4d617474696c73796e6574-v24-org");
        assert_eq!(
            org_meta,
            "gh-report-org_4d617474696c73796e6574-v24-org_meta"
        );
        assert_eq!(
            org_data,
            "gh-report-org_4d617474696c73796e6574-v24-org_data"
        );

        let (team_meta, team_data) =
            nats_subjects_for_stem("gh-report-org_4d617474696c73796e6574-v24-team");
        assert_eq!(
            team_meta,
            "gh-report-org_4d617474696c73796e6574-v24-team_meta"
        );
        assert_eq!(
            team_data,
            "gh-report-org_4d617474696c73796e6574-v24-team_data"
        );
    }

    #[test]
    fn sanitize_nats_url_strips_password_cleanly() {
        assert_eq!(
            sanitize_nats_url("nats://alice:secret@127.0.0.1:4222"),
            "nats://127.0.0.1:4222"
        );
        assert_eq!(
            sanitize_nats_url("nats://127.0.0.1:4222"),
            "nats://127.0.0.1:4222"
        );
        assert_eq!(
            sanitize_nats_url("tls://connect.nats.mattilsynet.io:4222"),
            "tls://connect.nats.mattilsynet.io:4222"
        );
        assert_eq!(
            sanitize_nats_url("nats://alice:secret@host:badport"),
            "invalid-nats-endpoint"
        );
        assert_eq!(
            sanitize_nats_url("not a url at all :::"),
            "invalid-nats-endpoint"
        );
        assert_eq!(
            sanitize_nats_url("nats:alice:secret@host:4222"),
            "invalid-nats-endpoint"
        );
        assert_eq!(
            sanitize_nats_url("nats:/alice:secret@host:4222"),
            "invalid-nats-endpoint"
        );
    }

    #[test]
    fn connect_nats_sync_reaches_live_server_on_port_sharing_dead_port_prefix() {
        let Some(server) = crate::store::tests::TestNatsServer::spawn_on_sentinel_prefixed_port()
        else {
            return;
        };
        assert!(
            server.url.starts_with("nats://127.0.0.1:1"),
            "regression requires a reachable endpoint whose address shares the dead-port prefix, got {}",
            server.url
        );
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let config = NatsStoreConfig::for_org("sentinel-prefix", &server.url).unwrap();

        let client = connect_nats_sync(rt.handle(), &config)
            .expect("live loopback NATS endpoint must be reachable through a real connection");

        assert_eq!(
            client.connection_state(),
            async_nats::connection::State::Connected
        );
        drop(client);
    }

    #[test]
    fn connect_nats_sync_rejects_invalid_percent_encoding_without_echoing_secret() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let config = NatsStoreConfig {
            nats_url: "nats://alice:secret%FF%FE@127.0.0.1:4222".to_string(),
            stream_name: "test-stream".to_string(),
            subject: "test.subject".to_string(),
            durable_consumer: "test-consumer".to_string(),
            credentials_path: None,
        };
        let err = connect_nats_sync(rt.handle(), &config).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        let err_msg = err.to_string();
        assert!(err_msg.contains("invalid UTF-8 percent-encoding"));
        assert!(!err_msg.contains("secret"));
    }

    fn synthetic_org_state(org: &str) -> crate::event::OrgStateCaptured {
        use crate::event::{
            AssessmentMetadata, AuthMode, CollectionStatus, OrgAlertSummary, TokenTier,
        };
        use pardosa::prelude::{EventString, EventVec};
        crate::event::OrgStateCaptured {
            archived_repos: 0,
            assessment_metadata: AssessmentMetadata {
                date: EventString::new("2026-07-16".to_string()).unwrap(),
                organization: EventString::new(org.to_string()).unwrap(),
                schema_version: EventString::new(
                    crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
                )
                .unwrap(),
                run_timestamp: EventString::new("2026-07-16T00:00:00Z".to_string()).unwrap(),
                run_id: EventString::new("run-1".to_string()).unwrap(),
                token_tier: TokenTier::Full,
                token_scopes: EventString::new("repo,read:org".to_string()).unwrap(),
                auth_mode: AuthMode::GitHubApp,
                rate_limit_warnings: 0,
                unavailable_capabilities: EventVec::new(Vec::new()).unwrap(),
                inventory_fetched_at: Some(
                    EventString::new("2026-07-16T00:00:00Z".to_string()).unwrap(),
                ),
                warm_start: false,
                coverage: crate::event::CollectionCoverage::default(),
            },
            alert_summary: OrgAlertSummary {
                collection_status: CollectionStatus::Success,
                collection_reason: None,
                http_status: Some(200),
                per_repo: EventVec::new(Vec::new()).unwrap(),
                open_secret_alert_age_buckets: EventVec::new(Vec::new()).unwrap(),
                total_open_secret_alerts: 0,
                oldest_open_secret_alert_created_at: None,
                newest_open_secret_alert_created_at: None,
            },
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "comprehensive auth-aware ACL verification across three production stores and negative rejection"
    )]
    fn nats_auth_aware_acl_permits_native_and_denies_unlisted_subjects() {
        use pardosa::prelude::*;

        let allowed =
            NatsStoreConfig::for_org("test-acl-org", crate::config::runtime::DEFAULT_NATS_URL)
                .unwrap();
        let allowed_subjects = [
            allowed.stream_name.clone(),
            allowed.org_events().stream_name,
            allowed.team_events().stream_name,
        ]
        .into_iter()
        .flat_map(|stem| [format!("\"{stem}_meta\""), format!("\"{stem}_data\"")])
        .collect::<Vec<_>>()
        .join(", ");
        let conf = r#"
accounts: {
    APP: {
        jetstream: enabled
        users: [
            {
                user: "test-user"
                password: "p@ss:word"
                permissions: {
                    publish: {
                        allow: [ALLOWED_SUBJECTS, "$JS.API.>", "allowed-baddata-meta"]
                    }
                    subscribe: {
                        allow: [">"]
                    }
                }
            }
        ]
    }
}
"#;
        let conf = conf.replace("ALLOWED_SUBJECTS", &allowed_subjects);
        let Some(server) = crate::store::tests::TestNatsServer::spawn_with_auth_config(&conf)
        else {
            return;
        };

        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
        );

        let endpoint = server.url.strip_prefix("nats://").expect("nats url prefix");
        let auth_url = format!("nats://test-user:p%40ss%3Aword@{endpoint}");

        let nats_config = NatsStoreConfig::for_org("test-acl-org", &auth_url).unwrap();
        let org_nats_config = nats_config.org_events();
        let team_nats_config = nats_config.team_events();

        let tmp = tempfile::tempdir().expect("tempdir");
        let events_dir = tmp.path().join("events");

        let events_store = open_event_store(
            &events_dir,
            PardosaBackend::Nats,
            &nats_config,
            rt.handle(),
            Some(&rt),
        )
        .expect("open_event_store under gh-report.> must succeed");
        let domain_event = crate::store::tests::synthetic_domain_event(1);
        events_store
            .record("domain-1", domain_event.clone())
            .expect("record event");
        drop(events_store);

        let reloaded_events = open_event_store(
            &events_dir,
            PardosaBackend::Nats,
            &nats_config,
            rt.handle(),
            Some(&rt),
        )
        .expect("reopen event store under gh-report.> must succeed");
        let replayed = reloaded_events.events().unwrap();
        assert_eq!(replayed.len(), 1);
        assert!(!replayed[0].0);
        assert_eq!(replayed[0].1, domain_event);

        let org_store = open_org_event_store(
            &events_dir,
            PardosaBackend::Nats,
            &org_nats_config,
            rt.handle(),
            Some(&rt),
        )
        .expect("open_org_event_store under gh-report.> must succeed");
        let org_event = synthetic_org_state("test-acl-org");
        org_store
            .record("test-acl-org", org_event.clone())
            .expect("record org event");
        drop(org_store);

        let reloaded_org = open_org_event_store(
            &events_dir,
            PardosaBackend::Nats,
            &org_nats_config,
            rt.handle(),
            Some(&rt),
        )
        .expect("reopen org store under gh-report.> must succeed");
        let mut replayed_org = Vec::new();
        reloaded_org
            .fold_events((), |(), e| replayed_org.push(e.clone()))
            .unwrap();
        assert_eq!(replayed_org.len(), 1);
        assert_eq!(replayed_org[0], org_event);

        let team_store = open_team_event_store(
            &events_dir,
            PardosaBackend::Nats,
            &team_nats_config,
            rt.handle(),
            Some(&rt),
        )
        .expect("open_team_event_store under gh-report.> must succeed");
        let team_event = crate::store::tests::synthetic_team_state("test-acl-org", "test-team");
        team_store
            .record("test-acl-org/test-team", team_event.clone())
            .expect("record team event");
        drop(team_store);

        let reloaded_team = open_team_event_store(
            &events_dir,
            PardosaBackend::Nats,
            &team_nats_config,
            rt.handle(),
            Some(&rt),
        )
        .expect("reopen team store under gh-report.> must succeed");
        let mut replayed_team = Vec::new();
        reloaded_team
            .fold_events((), |(), e| replayed_team.push(e.clone()))
            .unwrap();
        assert_eq!(replayed_team.len(), 1);
        assert_eq!(replayed_team[0], team_event);

        let server_errors = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let server_errors_cb = Arc::clone(&server_errors);
        let client = rt
            .block_on(
                async_nats::ConnectOptions::new()
                    .user_and_password("test-user".to_string(), "p@ss:word".to_string())
                    .event_callback(move |event| {
                        let errors = Arc::clone(&server_errors_cb);
                        async move {
                            if let async_nats::Event::ServerError(err) = event {
                                errors.lock().unwrap().push(err.to_string());
                            }
                        }
                    })
                    .connect(&server.url),
            )
            .expect("authenticated connect");

        let bad_stem = "gh-report-org_4d617474696c73796e6574-v22-bad";
        let bad_adapter = pardosa_nats::NatsStorageAdapter::from_client_with_runtime(
            client.clone(),
            bad_stem,
            Arc::clone(&rt),
        );
        let bad_claim = OwnershipClaimRecord {
            epoch: 1,
            machine_id: [0u8; 16],
            boot_id: [0u8; 16],
            process_id: u64::from(std::process::id()),
            process_start_time_ns: 0,
            claim_time_ns: 0,
            operator_label: "test-auth-denial".to_string(),
        };
        let bad_result = bad_adapter.create(&bad_claim, &AdmittedDescriptor::default_for_test());
        let Err(bad_err) = bad_result else {
            panic!("creation with non-dotted subject must fail on permission violation");
        };
        let err_text = bad_err.to_string();
        assert!(
            err_text.contains("failed to ack container header on meta stream")
                || err_text.contains("timed out")
                || err_text.contains("Permissions Violation"),
            "error must identify permission denial or ack timeout from forbidden subject: {err_text}"
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut captured = Vec::new();
        let expected_meta_pattern =
            format!("Permissions Violation for Publish to \"{bad_stem}_meta\"");
        while std::time::Instant::now() < deadline {
            captured = server_errors.lock().unwrap().clone();
            if captured.iter().any(|e| e.contains(&expected_meta_pattern)) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            captured
                .iter()
                .any(|err| err.contains(&expected_meta_pattern)),
            "must capture broker Permissions Violation event for exact meta subject {expected_meta_pattern}: {captured:?}"
        );

        let bad_data_stem = "gh-report-org_4d617474696c73796e6574-v22-baddata";
        let bad_data_adapter = pardosa_nats::NatsStorageAdapter::from_client_with_runtime(
            client.clone(),
            bad_data_stem,
            Arc::clone(&rt),
        )
        .with_subjects_for_test(
            "allowed-baddata-meta".to_string(),
            format!("{bad_data_stem}_data"),
        );
        let bad_data_result = crate::store::NativeStore::create_nats(bad_data_adapter);
        assert!(matches!(
            bad_data_result,
            Err(crate::store::StoreError::CreationUnknown(_))
        ));

        let expected_data_pattern =
            format!("Permissions Violation for Publish to \"{bad_data_stem}_data\"");
        let deadline_data = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut captured_data = Vec::new();
        while std::time::Instant::now() < deadline_data {
            captured_data = server_errors.lock().unwrap().clone();
            if captured_data
                .iter()
                .any(|e| e.contains(&expected_data_pattern))
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            captured_data
                .iter()
                .any(|err| err.contains(&expected_data_pattern)),
            "must capture broker Permissions Violation event for exact data subject {expected_data_pattern}: {captured_data:?}"
        );

        let js = rt.block_on(async { async_nats::jetstream::new(client) });
        for config in [&allowed, &allowed.org_events(), &allowed.team_events()] {
            let (meta_subject, data_subject) = config.native_subjects();
            for (suffix, subject) in [("meta", meta_subject), ("data", data_subject)] {
                let mut stream = rt
                    .block_on(js.get_stream(format!("{}_{suffix}", config.stream_name)))
                    .unwrap();
                assert_eq!(
                    rt.block_on(stream.info()).unwrap().config.subjects,
                    vec![subject]
                );
            }
        }
        let mut meta = rt
            .block_on(js.get_stream(format!("{bad_data_stem}_meta")))
            .unwrap();
        assert!(
            rt.block_on(meta.info()).unwrap().state.messages > 0,
            "creation has durable metadata despite denied data"
        );
        let mut stream = rt
            .block_on(async { js.get_stream(format!("{bad_stem}_meta")).await })
            .expect("stream was created on broker");
        let stream_info = rt
            .block_on(async { stream.info().await })
            .expect("stream info query");
        assert_eq!(
            stream_info.state.messages, 0,
            "stream with non-dotted subject must retain zero messages due to ACL publish rejection"
        );

        let mut data_stream = rt
            .block_on(async { js.get_stream(format!("{bad_data_stem}_data")).await })
            .expect("data stream was created on broker");
        let data_stream_info = rt
            .block_on(async { data_stream.info().await })
            .expect("data stream info query");
        assert_eq!(
            data_stream_info.state.messages, 0,
            "stream with non-dotted data subject must retain zero messages due to ACL publish rejection"
        );
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn read_rss_kb_is_none_off_linux() {
        assert_eq!(read_rss_kb(), None);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn read_rss_kb_is_some_positive_on_linux() {
        assert!(read_rss_kb().is_some_and(|kb| kb > 0));
    }

    #[tokio::test]
    async fn cache_respects_max_capacity() {
        let state = AppState::new_with_cache_capacity(3).await;
        let cache = &state.github.repo_detail_cache;

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
        let cache = &state.github.repo_detail_cache;

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
        let cache = &state.github.repo_detail_cache;

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
        assert!(state.evidence.html_cache.load().is_none());
    }

    #[tokio::test]
    async fn builder_default_produces_valid_state() {
        let state = AppStateBuilder::new().build().await;
        assert!(state.webhook.secret.is_none());
        assert!(state.evidence.html_cache.load().is_none());
    }

    #[tokio::test]
    async fn builder_with_webhook_secret() {
        let state = AppStateBuilder::new()
            .webhook_secret("test-secret")
            .build()
            .await;
        assert!(state.webhook.secret.is_some());
    }

    #[tokio::test]
    async fn builder_with_cache_capacity() {
        let state = AppStateBuilder::new().cache_capacity(5).build().await;
        let cache = &state.github.repo_detail_cache;
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
    async fn app_state_owner_id_is_uuid_v7_and_stable_for_process_state() {
        let state = AppState::new().await;
        let first = state.owner_id;
        let second = state.owner_id;

        assert_eq!(first, second, "owner-id is minted once per AppState");
        assert_eq!(
            first.get_version_num(),
            7,
            "owner-id must be UUID v7 for fencing audit identity"
        );
    }

    #[test]
    fn typed_pardosa_conflict_reaches_fatal_fence_policy_without_bounded_retry() {
        use pardosa::prelude::{FailureCondition, OperationFailure};

        let mut attempts = 0_u32;
        let failure = crate::app::write_policy::write_with_policy_sync(|| {
            attempts += 1;
            let typed = OperationFailure::new(
                FailureCondition::ConcurrencyConflict,
                "two-writer concurrency collision: expected sequence mismatch",
            );
            Err(native_store_persistence(crate::store::StoreError::from(
                typed,
            )))
        })
        .expect_err("a fenced conflict must not be reported as a successful write");

        assert_eq!(
            attempts, 1,
            "PGN-0016:R2 forbids in-band retry of a lost fence"
        );
        assert_eq!(
            failure.category,
            crate::app::write_policy::WritePolicyCategory::Conflict
        );
        assert_eq!(
            failure.response,
            crate::app::write_policy::WriteResponse::Fatal
        );
    }

    fn typed_transport_then_conflict_then_success(
        attempt: u32,
    ) -> Result<(), cherry_pit_storage::PersistenceError> {
        use pardosa::prelude::{FailureCondition, OperationFailure};

        let condition = match attempt {
            1 => FailureCondition::TransportUnavailable,
            2 => FailureCondition::ConcurrencyConflict,
            _ => return Ok(()),
        };
        let typed = OperationFailure::new(
            condition,
            "two-writer concurrency collision: expected sequence mismatch",
        );
        Err(native_store_persistence(crate::store::StoreError::from(
            typed,
        )))
    }

    fn assert_typed_conflict_source_retained(failure: &crate::app::write_policy::WriteFailure) {
        use pardosa::prelude::OperationFailure;

        let PersistenceError::FencedConflict {
            expected_seq,
            actual_seq,
            source,
        } = &failure.error
        else {
            panic!("expected FencedConflict, got {:?}", failure.error);
        };
        assert_eq!(
            *expected_seq, None,
            "no typed expected sequence is available"
        );
        assert_eq!(*actual_seq, None, "no typed actual sequence is available");
        let typed = source
            .downcast_ref::<OperationFailure>()
            .expect("the typed pardosa failure must survive end to end");
        assert!(
            typed.is_concurrency_conflict(),
            "the retained source must keep its ConcurrencyConflict condition"
        );
        assert!(
            typed
                .to_string()
                .contains("two-writer concurrency collision"),
            "the retained source must keep its original detail"
        );
    }

    #[test]
    fn typed_pardosa_conflict_after_a_transport_retry_still_stops_the_policy_loop_sync() {
        let mut attempts = 0_u32;
        let failure = crate::app::write_policy::write_with_policy_sync(|| {
            attempts += 1;
            typed_transport_then_conflict_then_success(attempts)
        })
        .expect_err("a fenced conflict must not be reported as a successful write");

        assert_eq!(
            attempts, 2,
            "the success sentinel must never be reached: PGN-0016:R2 forbids retrying past a lost fence"
        );
        assert_eq!(
            failure.category,
            crate::app::write_policy::WritePolicyCategory::Conflict
        );
        assert_eq!(
            failure.response,
            crate::app::write_policy::WriteResponse::Fatal
        );
        assert_eq!(failure.attempt, 2);
        assert_typed_conflict_source_retained(&failure);
    }

    #[tokio::test]
    async fn typed_pardosa_conflict_after_a_transport_retry_still_stops_the_policy_loop_async() {
        let mut attempts = 0_u32;
        let failure = crate::app::write_policy::write_with_policy(|| {
            attempts += 1;
            typed_transport_then_conflict_then_success(attempts)
        })
        .await
        .expect_err("a fenced conflict must not be reported as a successful write");

        assert_eq!(
            attempts, 2,
            "the success sentinel must never be reached: PGN-0016:R2 forbids retrying past a lost fence"
        );
        assert_eq!(
            failure.category,
            crate::app::write_policy::WritePolicyCategory::Conflict
        );
        assert_eq!(
            failure.response,
            crate::app::write_policy::WriteResponse::Fatal
        );
        assert_eq!(failure.attempt, 2);
        assert_typed_conflict_source_retained(&failure);
    }

    #[test]
    fn native_store_persistence_preserves_fenced_conflict_variant() {
        let err = crate::store::StoreError::ConcurrencyConflict {
            expected_seq: Some(1),
            actual_seq: Some(2),
            source: Box::new(std::io::Error::other("wrong last sequence")),
        };

        assert!(
            matches!(
                native_store_persistence(err),
                PersistenceError::FencedConflict { .. }
            ),
            "fence conflicts must stay typed before Display flattening"
        );
    }

    #[test]
    fn native_store_persistence_carries_seq_fields_into_fenced_conflict() {
        let err = crate::store::StoreError::ConcurrencyConflict {
            expected_seq: Some(42),
            actual_seq: Some(44),
            source: Box::new(std::io::Error::other("wrong last sequence")),
        };
        match native_store_persistence(err) {
            PersistenceError::FencedConflict {
                expected_seq,
                actual_seq,
                ..
            } => {
                assert_eq!(expected_seq, Some(42));
                assert_eq!(actual_seq, Some(44));
            }
            other => panic!("expected FencedConflict, got {other:?}"),
        }
    }

    #[test]
    fn native_store_persistence_preserves_torn_write_recovery_variant() {
        let err = crate::store::StoreError::TornWriteRecovery {
            source: Box::new(std::io::Error::other("torn write recovery failed")),
        };

        assert!(
            matches!(
                native_store_persistence(err),
                PersistenceError::TornWriteRecovery { .. }
            ),
            "torn-write recovery failures must stay typed before Display flattening"
        );
    }

    #[test]
    fn native_store_persistence_logs_full_error_chain_before_flattening() {
        let err =
            crate::store::StoreError::Infrastructure("nats: authorization violation".to_string());

        let output = capture_events(|| {
            let persistence = native_store_persistence(err);

            let PersistenceError::BackendUnavailable { source } = persistence else {
                panic!("backend infrastructure failure should map to BackendUnavailable");
            };
            assert!(
                source.to_string().contains("authorization violation"),
                "diagnostic remains operator-visible: {source}"
            );
        });
        assert!(
            output.contains("error_chain"),
            "diagnostic event must carry an error_chain field: {output}"
        );
        assert!(
            output.contains("nats: authorization violation"),
            "full diagnostic chain must include innermost source"
        );
    }

    #[test]
    fn native_transport_survives_exhausted_policy_as_typed_source() {
        use std::error::Error;
        let mut calls = 0;
        let failure = crate::app::write_policy::write_with_policy_sync(|| {
            calls += 1;
            Err(native_store_persistence(
                OperationFailure::new(
                    FailureCondition::TransportUnavailable,
                    "actual native diagnostic",
                )
                .into(),
            ))
        })
        .unwrap_err();
        assert_eq!(
            calls,
            1 + u32::from(crate::app::write_policy::BOUNDED_RETRY_ATTEMPTS)
        );
        let native = failure
            .error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .downcast_ref::<OperationFailure>()
            .unwrap();
        assert_eq!(native.condition(), &FailureCondition::TransportUnavailable);
        assert!(native.to_string().contains("actual native diagnostic"));
    }

    #[tokio::test]
    async fn record_repo_writes_native_store_and_remove_detaches_latest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let state = AppState::with_stores(
            &events_dir,
            PardosaBackend::Pgno,
            NatsStoreConfig::for_org("org", crate::config::runtime::DEFAULT_NATS_URL).unwrap(),
        )
        .await
        .expect("with_stores");
        let evidence = crate::test_fixtures::all_passing_evidence("native-repo");
        let domain_key = evidence.repository.inventory_key.clone();
        let repo_name = evidence.repository.name.clone();
        let timestamp = "2026-06-11T00:00:00Z";

        state
            .record_repo(&domain_key, evidence, &repo_name, timestamp)
            .expect("record repo");

        let latest = state
            .event_store
            .latest_per_repo()
            .expect("latest per repo");
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].0, domain_key);

        state
            .remove_repo(&domain_key, &repo_name, timestamp)
            .expect("remove repo");

        let latest = state
            .event_store
            .latest_per_repo()
            .expect("latest after remove");
        assert!(latest.is_empty());
    }

    #[tokio::test]
    async fn record_and_remove_apply_projection_without_full_refold() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let state = AppState::with_stores(
            &events_dir,
            PardosaBackend::Pgno,
            NatsStoreConfig::for_org("org", crate::config::runtime::DEFAULT_NATS_URL).unwrap(),
        )
        .await
        .expect("with_stores");
        let evidence = crate::test_fixtures::all_passing_evidence("debounced-repo");
        let domain_key = evidence.repository.inventory_key.clone();
        let repo_name = evidence.repository.name.clone();
        let timestamp = "2026-06-11T00:00:00Z";

        state
            .record_repo(&domain_key, evidence, &repo_name, timestamp)
            .expect("record repo");
        {
            let projection = state.lock_projection();
            assert!(projection.repositories.contains_key(&domain_key));
        }

        state
            .remove_repo(&domain_key, &repo_name, timestamp)
            .expect("remove repo");
        let projection = state.lock_projection();
        assert!(!projection.repositories.contains_key(&domain_key));
    }

    #[tokio::test]
    async fn record_repo_replay_does_not_double_apply_domain_keyed_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let state = AppState::with_stores(
            &events_dir,
            PardosaBackend::Pgno,
            NatsStoreConfig::for_org("org", crate::config::runtime::DEFAULT_NATS_URL).unwrap(),
        )
        .await
        .expect("with_stores");
        let mut evidence = crate::test_fixtures::all_passing_evidence("rearm-replay-repo");
        let domain_key = evidence.repository.inventory_key.clone();
        let repo_name = evidence.repository.name.clone();

        state
            .record_repo(
                &domain_key,
                evidence.clone(),
                &repo_name,
                "2026-06-11T00:00:00Z",
            )
            .expect("first apply, standing in for the tick that hit FencedConflict");

        evidence.repository.archived = true;
        state
            .record_repo(
                &domain_key,
                evidence.clone(),
                &repo_name,
                "2026-06-11T00:05:00Z",
            )
            .expect("replay apply, standing in for the re-armed retry's fresh authoritative read");

        let latest = state
            .event_store
            .latest_per_repo()
            .expect("latest per repo");
        assert_eq!(
            latest.len(),
            1,
            "a re-armed replay of the same domain_key must fold onto one fiber, not fan out \
             into two live repo records"
        );

        let projection = state.lock_projection();
        assert_eq!(
            projection.repositories.len(),
            1,
            "snapshot-overwrite projection must hold exactly one entry for domain_key after replay"
        );
        assert!(
            projection
                .repositories
                .get(&domain_key)
                .expect("domain_key present in projection")
                .repository
                .archived,
            "replay must leave the LATEST application in place, not stack on the stale first one"
        );
    }

    #[tokio::test]
    async fn projection_bytes_deep_is_heap_inclusive() {
        let state = AppState::new().await;

        for name in ["deep-repo-a", "deep-repo-b"] {
            let mut evidence = crate::test_fixtures::all_passing_evidence(name);
            evidence.repository.description = Some("d".repeat(4096));
            evidence.repository.topics = (0..64)
                .map(|i| format!("topic-{name}-{i}-padding-for-heap-bytes"))
                .collect();
            let domain_key = evidence.repository.inventory_key.clone();
            state
                .lock_projection()
                .repositories
                .insert(domain_key, evidence);
        }

        let repo_count = state.projection_len();
        assert_eq!(repo_count, 2);
        let shallow_floor =
            repo_count * std::mem::size_of::<crate::domain::evidence::RepositoryEvidence>();

        let deep = state
            .projection_bytes_deep()
            .expect("well-formed evidence serializes");
        assert!(
            deep > shallow_floor,
            "deep sample ({deep}) must exceed the shallow struct-size floor ({shallow_floor}) to prove heap-inclusive measurement"
        );
    }

    fn team_roster_fixture(
        canonical_owner: &str,
        team_slug: &str,
    ) -> crate::domain::metrics::TeamRoster {
        crate::domain::metrics::TeamRoster {
            fetched_at: None,
            canonical_owner: canonical_owner.to_string(),
            team_slug: team_slug.to_string(),
            status: crate::domain::metrics::TeamRosterStatus::Complete,
            members: vec![crate::domain::metrics::TeamMember {
                login: "octocat".to_string(),
                role: crate::domain::metrics::TeamMemberRole::Member,
                in_org: Some(true),
            }],
        }
    }

    #[tokio::test]
    async fn record_team_persists_and_runtime_folds_projection_without_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let nats = NatsStoreConfig::for_org("TestOrg", crate::config::runtime::DEFAULT_NATS_URL)
            .expect("nats config");
        let state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats)
            .await
            .expect("with stores");

        let roster = team_roster_fixture("@TestOrg/platform", "platform");
        state
            .record_team(
                "TestOrg",
                &roster,
                "2026-07-16T00:00:00Z",
                OrgMembershipFetchStatus::Fetched,
            )
            .expect("record team roster");

        let team_key = team_domain_key("TestOrg", "platform").expect("derive team key");
        let folded = state
            .lock_projection()
            .team_rosters
            .get(&team_key)
            .cloned()
            .expect("runtime fold must upsert team_rosters without a restart");
        assert_eq!(folded.canonical_owner, "@TestOrg/platform");
        assert_eq!(folded.members.len(), 1);
        assert_eq!(folded.members[0].login, "octocat");
    }

    #[tokio::test]
    async fn detach_team_runtime_removes_projection_entry_without_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let nats = NatsStoreConfig::for_org("TestOrg", crate::config::runtime::DEFAULT_NATS_URL)
            .expect("nats config");
        let state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats)
            .await
            .expect("with stores");

        let roster = team_roster_fixture("@TestOrg/platform", "platform");
        state
            .record_team(
                "TestOrg",
                &roster,
                "2026-07-16T00:00:00Z",
                OrgMembershipFetchStatus::Fetched,
            )
            .expect("record team roster");
        let team_key = team_domain_key("TestOrg", "platform").expect("derive team key");
        assert!(state.lock_projection().team_rosters.contains_key(&team_key));

        state
            .detach_team(
                "TestOrg",
                &roster,
                "2026-07-16T01:00:00Z",
                OrgMembershipFetchStatus::Fetched,
            )
            .expect("detach team roster");

        assert!(
            !state.lock_projection().team_rosters.contains_key(&team_key),
            "runtime fold must remove the roster immediately on detach, not only on next restart"
        );
    }

    #[test]
    fn detach_team_event_carries_minimal_tombstone_not_full_roster() {
        let roster = team_roster_fixture("@TestOrg/platform", "platform");
        assert_eq!(
            roster.members.len(),
            1,
            "fixture must carry a live member to prove the tombstone strips it"
        );

        let event = team_state_event(
            "TestOrg",
            &roster,
            "2026-07-17T00:00:00Z",
            OrgMembershipFetchStatus::Fetched,
        )
        .expect("team_state_event constructs");
        assert_eq!(
            event.members.as_slice().len(),
            1,
            "team_state_event with the live roster still carries the full member list \
             (this is the pre-fix baseline the detach path must not use directly)"
        );

        let tombstone = detach_tombstone_roster(&roster);
        assert_eq!(tombstone.canonical_owner, roster.canonical_owner);
        assert_eq!(tombstone.team_slug, roster.team_slug);
        assert_eq!(tombstone.status, roster.status);
        assert!(
            tombstone.members.is_empty(),
            "CHE-0091:R1 detach tombstone body must be minimal, not a full roster clone"
        );

        let tombstone_event = team_state_event(
            "TestOrg",
            &tombstone,
            "2026-07-17T00:00:00Z",
            OrgMembershipFetchStatus::Fetched,
        )
        .expect("team_state_event constructs from tombstone");
        assert!(
            tombstone_event.members.as_slice().is_empty(),
            "the persisted detach event body must carry no members (minimal tombstone, CHE-0091:R1)"
        );
    }

    #[tokio::test]
    async fn reconstruct_team_rosters_from_event_log_without_live_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let nats = NatsStoreConfig::for_org("TestOrg", crate::config::runtime::DEFAULT_NATS_URL)
            .expect("nats config");
        let state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats.clone())
            .await
            .expect("with stores");
        let roster = team_roster_fixture("@TestOrg/platform", "platform");
        state
            .record_team(
                "TestOrg",
                &roster,
                "2026-07-16T00:00:00Z",
                OrgMembershipFetchStatus::Fetched,
            )
            .expect("record team roster");
        drop(state);

        let restarted = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats)
            .await
            .expect("restart from team event log");
        let team_key = team_domain_key("TestOrg", "platform").expect("derive team key");
        let replayed = restarted
            .lock_projection()
            .team_rosters
            .get(&team_key)
            .cloned()
            .expect("team roster must replay from the team event log");
        assert_eq!(replayed.canonical_owner, "@TestOrg/platform");
    }

    #[test]
    fn record_team_conflict_is_fatal_with_no_in_band_retry() {
        let error = crate::store::StoreError::ConcurrencyConflict {
            expected_seq: None,
            actual_seq: None,
            source: Box::new(std::io::Error::other("wrong last sequence")),
        };
        let persistence_error = native_store_persistence(error);
        assert!(
            matches!(persistence_error, PersistenceError::FencedConflict { .. }),
            "team fence conflicts must stay typed before Display flattening"
        );
        let failure = crate::app::write_policy::WriteFailure::classify(persistence_error);
        assert_eq!(
            failure.category,
            crate::app::write_policy::WritePolicyCategory::Conflict
        );
        assert_eq!(
            failure.response,
            crate::app::write_policy::WriteResponse::Fatal,
            "OCC conflict must abort the run, never retry in-band"
        );
    }

    fn corrupt_pgno_backing_file(path: &Path) {
        assert!(
            path.exists(),
            "backing store {} must exist before the failure seam is planted",
            path.display()
        );
        std::fs::write(path, b"corrupt-not-a-pgno-container")
            .expect("overwrite the backing store so the next append fails for real");
    }

    fn team_events_in_primary_store(state: &AppState, team_key: &str) -> usize {
        state
            .event_store
            .events_with_fibers()
            .expect("read primary store cache")
            .iter()
            .filter(|(_, _, event)| match event {
                NativeDomainEvent::TeamStateCaptured(team) => {
                    crate::event::team_domain_key(team.org.as_str(), team.team_slug.as_str())
                        .is_ok_and(|key| key == team_key)
                }
                _ => false,
            })
            .count()
    }

    #[tokio::test]
    async fn record_team_primary_failure_propagates_and_skips_dedicated_and_projection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let nats = NatsStoreConfig::for_org("TestOrg", crate::config::runtime::DEFAULT_NATS_URL)
            .expect("nats config");
        let state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats)
            .await
            .expect("with stores");

        corrupt_pgno_backing_file(&events_dir.join("events.pgno"));

        let roster = team_roster_fixture("@TestOrg/platform", "platform");
        let error = state
            .record_team(
                "TestOrg",
                &roster,
                "2026-07-16T00:00:00Z",
                OrgMembershipFetchStatus::Fetched,
            )
            .expect_err(
                "a failed primary team append must surface as a persistence error, \
                 not be discarded while the dedicated write proceeds",
            );
        assert!(
            matches!(error, PersistenceError::Io(_)),
            "primary append failure must carry a concrete persistence classification, got {error:?}"
        );

        let team_key = team_domain_key("TestOrg", "platform").expect("derive team key");
        let dedicated_count = state
            .team_event_store
            .fold_events(0_usize, |acc, _| *acc += 1)
            .expect("fold dedicated team events");
        assert_eq!(
            dedicated_count, 0,
            "the dedicated team append must be skipped when the primary append failed"
        );
        assert!(
            !state.lock_projection().team_rosters.contains_key(&team_key),
            "persist-then-publish: no projection fold may follow a failed primary append"
        );
    }

    #[tokio::test]
    async fn record_team_dedicated_failure_returns_error_preserving_primary_without_projection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let nats = NatsStoreConfig::for_org("TestOrg", crate::config::runtime::DEFAULT_NATS_URL)
            .expect("nats config");
        let state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats)
            .await
            .expect("with stores");

        corrupt_pgno_backing_file(&events_dir.join("team-events.pgno"));

        let roster = team_roster_fixture("@TestOrg/platform", "platform");
        let error = state
            .record_team(
                "TestOrg",
                &roster,
                "2026-07-16T00:00:00Z",
                OrgMembershipFetchStatus::Fetched,
            )
            .expect_err("a failed dedicated team append must surface as a persistence error");
        assert!(
            matches!(error, PersistenceError::Indeterminate(_)),
            "dedicated append failure must carry a concrete persistence classification, got {error:?}"
        );

        let team_key = team_domain_key("TestOrg", "platform").expect("derive team key");
        assert_eq!(
            team_events_in_primary_store(&state, &team_key),
            1,
            "the already-durable primary append is retained: record_team is not atomic \
             and claims no rollback of the primary write"
        );
        assert!(
            !state.lock_projection().team_rosters.contains_key(&team_key),
            "no projection fold may follow a failed dedicated append"
        );
        assert!(
            state
                .record_team(
                    "TestOrg",
                    &roster,
                    "2026-07-16T00:00:00Z",
                    OrgMembershipFetchStatus::Fetched
                )
                .is_err()
        );
        assert_eq!(team_events_in_primary_store(&state, &team_key), 1);
    }

    #[tokio::test]
    async fn composite_transport_failure_is_one_attempt_and_blocks_next_primary() {
        for org_write in [false, true] {
            let state = AppState::new().await;
            let roster = team_roster_fixture("@TestOrg/platform", "platform");
            if org_write {
                state
                    .org_event_store
                    .fail_writes_for_test(FailureCondition::TransportUnavailable);
            } else {
                state
                    .team_event_store
                    .fail_writes_for_test(FailureCondition::TransportUnavailable);
            }
            let before = state.event_store.events().unwrap().len();
            let mut calls = 0;
            let mut submit = || {
                calls += 1;
                if org_write {
                    state.record_org(org_snapshot_fixture())
                } else {
                    state.record_team(
                        "TestOrg",
                        &roster,
                        "2026-07-16T00:00:00Z",
                        OrgMembershipFetchStatus::Fetched,
                    )
                }
            };
            let failure =
                crate::app::write_policy::write_with_policy_sync(&mut submit).unwrap_err();
            assert_eq!(
                failure.category,
                crate::app::write_policy::WritePolicyCategory::ReconciliationRequired
            );
            assert!(matches!(submit(), Err(PersistenceError::Indeterminate(_))));
            assert_eq!(
                calls, 2,
                "one policy attempt plus one explicitly rejected submission"
            );
            assert_eq!(state.event_store.events().unwrap().len(), before + 1);
            assert!(state.projection_team_rosters_snapshot().is_empty());
            assert!(state.projection_snapshot().is_empty());
            assert!(!state.run_is_fenced());
        }
    }

    #[tokio::test]
    async fn projection_panic_retains_unknown_and_prevents_second_durable_write() {
        let state = AppState::new().await;
        let supervisor_state = Arc::clone(&state);
        let supervisor =
            tokio::spawn(async move { supervisor_state.wait_for_reconciliation().await });
        tokio::task::yield_now().await;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _projection = state.lock_projection();
            panic!("poison actual projection");
        }));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.record_org(org_snapshot_fixture())
        }));
        assert!(result.is_err());
        tokio::time::timeout(std::time::Duration::from_secs(1), supervisor)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.event_store.events().unwrap().len(), 1);
        assert!(matches!(
            state.ensure_write_admission(),
            Err(PersistenceError::Indeterminate(_))
        ));
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            state.wait_for_reconciliation(),
        )
        .await
        .unwrap();
        assert!(matches!(
            state.record_org(org_snapshot_fixture()),
            Err(PersistenceError::Indeterminate(_))
        ));
        assert_eq!(state.event_store.events().unwrap().len(), 1);
        assert_eq!(
            state
                .org_event_store
                .fold_events(0, |count, _| *count += 1)
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn poisoned_empty_admission_is_not_ready() {
        let state = AppState::new().await;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _slot = state.evidence.write_admission.lock().unwrap();
            panic!("poison empty admission before effects");
        }));
        assert!(matches!(
            state.ensure_write_admission(),
            Err(PersistenceError::Indeterminate(_))
        ));
        assert!(matches!(
            state.record_org(org_snapshot_fixture()),
            Err(PersistenceError::Indeterminate(_))
        ));
        assert!(state.event_store.events().unwrap().is_empty());
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            state.wait_for_reconciliation(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn record_team_success_appends_both_stores_and_publishes_projection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let nats = NatsStoreConfig::for_org("TestOrg", crate::config::runtime::DEFAULT_NATS_URL)
            .expect("nats config");
        let state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats)
            .await
            .expect("with stores");

        let roster = team_roster_fixture("@TestOrg/platform", "platform");
        state
            .record_team(
                "TestOrg",
                &roster,
                "2026-07-16T00:00:00Z",
                OrgMembershipFetchStatus::Fetched,
            )
            .expect("ordinary success path is unchanged by primary-error propagation");

        let team_key = team_domain_key("TestOrg", "platform").expect("derive team key");
        assert_eq!(
            team_events_in_primary_store(&state, &team_key),
            1,
            "success still appends the team event to the primary store"
        );
        assert_eq!(
            state
                .team_event_store
                .fold_events(0_usize, |acc, _| *acc += 1)
                .expect("fold dedicated team events"),
            1,
            "success still appends the team event to the dedicated store"
        );
        assert!(
            state.lock_projection().team_rosters.contains_key(&team_key),
            "success still publishes the roster into the resident projection"
        );
    }

    fn detached_team_events_in_primary_store(state: &AppState, team_key: &str) -> usize {
        state
            .event_store
            .events_with_fibers()
            .expect("read primary store cache")
            .iter()
            .filter(|(detached, _, event)| {
                *detached
                    && match event {
                        NativeDomainEvent::TeamStateCaptured(team) => {
                            crate::event::team_domain_key(
                                team.org.as_str(),
                                team.team_slug.as_str(),
                            )
                            .is_ok_and(|key| key == team_key)
                        }
                        _ => false,
                    }
            })
            .count()
    }

    struct PersistedFiberMarkers {
        envelopes: usize,
        detached_markers: usize,
    }

    fn persisted_fiber_markers(pgno_path: &Path, domain_key: &str) -> PersistedFiberMarkers {
        let fiber_id = derive_fiber_id(domain_key);
        let adapter = FileStorageAdapter::new(pgno_path);
        let mut reader = adapter
            .open_read()
            .expect("open the persisted store for reading");
        let envelopes = reader
            .read_all_envelopes()
            .expect("read the persisted envelopes");
        let mut markers = PersistedFiberMarkers {
            envelopes: 0,
            detached_markers: 0,
        };
        for envelope in envelopes
            .iter()
            .filter(|envelope| envelope.header.fiber_id == fiber_id)
        {
            markers.envelopes += 1;
            if envelope.header.detached {
                markers.detached_markers += 1;
            }
        }
        markers
    }

    fn org_events_in_primary_store(state: &AppState, organization: &str) -> usize {
        state
            .event_store
            .events_with_fibers()
            .expect("read primary store cache")
            .iter()
            .filter(|(_, _, event)| match event {
                NativeDomainEvent::OrgStateCaptured(org) => {
                    org.assessment_metadata.organization.as_str() == organization
                }
                _ => false,
            })
            .count()
    }

    fn org_snapshot_fixture() -> crate::domain::evidence::OrgStateSnapshot {
        let mut metadata = crate::test_fixtures::make_metadata();
        metadata.organization = "TestOrg".to_string();
        crate::domain::evidence::OrgStateSnapshot {
            archived_repos: 3,
            assessment_metadata: metadata,
            alert_summary: empty_org_summary(),
        }
    }

    async fn state_with_seeded_team(
        events_dir: &Path,
    ) -> (Arc<AppState>, crate::domain::metrics::TeamRoster) {
        let nats = NatsStoreConfig::for_org("TestOrg", crate::config::runtime::DEFAULT_NATS_URL)
            .expect("nats config");
        let state = AppState::with_stores(events_dir, PardosaBackend::Pgno, nats)
            .await
            .expect("with stores");
        let roster = team_roster_fixture("@TestOrg/platform", "platform");
        state
            .record_team(
                "TestOrg",
                &roster,
                "2026-07-16T00:00:00Z",
                OrgMembershipFetchStatus::Fetched,
            )
            .expect("seed the team fiber before the detach seam is planted");
        (state, roster)
    }

    #[tokio::test]
    async fn detach_team_primary_failure_propagates_and_skips_dedicated_and_projection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let (state, roster) = state_with_seeded_team(&events_dir).await;
        let team_key = team_domain_key("TestOrg", "platform").expect("derive team key");

        corrupt_pgno_backing_file(&events_dir.join("events.pgno"));

        let error = state
            .detach_team(
                "TestOrg",
                &roster,
                "2026-07-16T01:00:00Z",
                OrgMembershipFetchStatus::Fetched,
            )
            .expect_err(
                "a failed primary team detach must surface as a persistence error, \
                 not be discarded while the dedicated detach proceeds",
            );
        assert!(
            matches!(error, PersistenceError::Io(_)),
            "primary detach failure must carry a concrete persistence classification, got {error:?}"
        );

        assert_eq!(
            state
                .team_event_store
                .fold_events(0_usize, |acc, _| *acc += 1)
                .expect("fold dedicated team events"),
            1,
            "the dedicated team detach must be skipped when the primary detach failed \
             (only the seeded record_team event remains)"
        );
        assert!(
            state.lock_projection().team_rosters.contains_key(&team_key),
            "persist-then-publish: no projection removal may follow a failed primary detach"
        );
        assert_eq!(
            detached_team_events_in_primary_store(&state, &team_key),
            0,
            "a failed primary detach leaves no detach-marked tombstone behind"
        );
        let dedicated = persisted_fiber_markers(&events_dir.join("team-events.pgno"), &team_key);
        assert_eq!(
            dedicated.envelopes, 1,
            "the dedicated team file still holds exactly the seeded record_team envelope"
        );
        assert_eq!(
            dedicated.detached_markers, 0,
            "the skipped dedicated detach must leave no persisted detach marker on the team fiber: \
             a fold count alone cannot discriminate a skipped detach from a successful one"
        );
    }

    #[tokio::test]
    async fn detach_team_dedicated_failure_returns_error_preserving_primary_detach() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let (state, roster) = state_with_seeded_team(&events_dir).await;
        let team_key = team_domain_key("TestOrg", "platform").expect("derive team key");

        corrupt_pgno_backing_file(&events_dir.join("team-events.pgno"));

        let error = state
            .detach_team(
                "TestOrg",
                &roster,
                "2026-07-16T01:00:00Z",
                OrgMembershipFetchStatus::Fetched,
            )
            .expect_err("a failed dedicated team detach must surface as a persistence error");
        assert!(
            matches!(error, PersistenceError::Indeterminate(_)),
            "dedicated detach failure must carry a concrete persistence classification, got {error:?}"
        );

        assert_eq!(
            team_events_in_primary_store(&state, &team_key),
            2,
            "the already-durable primary detach is retained alongside the seeded record: \
             detach_team is not atomic and claims no rollback of the primary write"
        );
        assert!(
            state.lock_projection().team_rosters.contains_key(&team_key),
            "no projection removal may follow a failed dedicated detach"
        );
        assert_eq!(
            detached_team_events_in_primary_store(&state, &team_key),
            1,
            "the retained primary write must be the detach-marked tombstone itself, \
             not merely a second team event of any kind"
        );
    }

    #[tokio::test]
    async fn detach_team_success_detaches_both_stores_and_removes_projection_roster() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let (state, roster) = state_with_seeded_team(&events_dir).await;
        let team_key = team_domain_key("TestOrg", "platform").expect("derive team key");

        state
            .detach_team(
                "TestOrg",
                &roster,
                "2026-07-16T01:00:00Z",
                OrgMembershipFetchStatus::Fetched,
            )
            .expect("ordinary detach success is unchanged by primary-error propagation");

        assert_eq!(
            team_events_in_primary_store(&state, &team_key),
            2,
            "success still writes the detach event to the primary store"
        );
        assert!(
            !state.lock_projection().team_rosters.contains_key(&team_key),
            "success still removes the detached roster from the resident projection"
        );
        assert_eq!(
            detached_team_events_in_primary_store(&state, &team_key),
            1,
            "success still writes a detach-marked tombstone event to the primary store"
        );
        let dedicated = persisted_fiber_markers(&events_dir.join("team-events.pgno"), &team_key);
        assert_eq!(
            dedicated.detached_markers, 1,
            "success must persist a detach marker on the target team fiber in the dedicated store: \
             replacing the dedicated detach with a successful no-op must fail this assertion"
        );
    }

    #[tokio::test]
    async fn record_org_primary_failure_propagates_and_skips_dedicated_and_projection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let nats = NatsStoreConfig::for_org("TestOrg", crate::config::runtime::DEFAULT_NATS_URL)
            .expect("nats config");
        let state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats)
            .await
            .expect("with stores");

        corrupt_pgno_backing_file(&events_dir.join("events.pgno"));

        let error = state.record_org(org_snapshot_fixture()).expect_err(
            "a failed primary org append must surface as a persistence error, \
             not be discarded while the dedicated write proceeds",
        );
        assert!(
            matches!(error, PersistenceError::Io(_)),
            "primary org append failure must carry a concrete persistence classification, got {error:?}"
        );

        assert_eq!(
            org_count_of(&state),
            0,
            "the dedicated org append must be skipped when the primary append failed"
        );
        assert!(
            state.lock_projection().org_state.is_none(),
            "persist-then-publish: no projection fold may follow a failed primary org append"
        );
    }

    #[tokio::test]
    async fn record_org_dedicated_failure_returns_error_preserving_primary_without_projection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let nats = NatsStoreConfig::for_org("TestOrg", crate::config::runtime::DEFAULT_NATS_URL)
            .expect("nats config");
        let state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats)
            .await
            .expect("with stores");

        corrupt_pgno_backing_file(&events_dir.join("org-events.pgno"));

        let error = state
            .record_org(org_snapshot_fixture())
            .expect_err("a failed dedicated org append must surface as a persistence error");
        assert!(
            matches!(error, PersistenceError::Indeterminate(_)),
            "dedicated org append failure must carry a concrete persistence classification, got {error:?}"
        );

        assert_eq!(
            org_events_in_primary_store(&state, "TestOrg"),
            1,
            "the already-durable primary org append is retained: record_org is not atomic \
             and claims no rollback of the primary write"
        );
        assert!(
            state.lock_projection().org_state.is_none(),
            "no projection fold may follow a failed dedicated org append"
        );
    }

    #[tokio::test]
    async fn record_org_success_appends_both_stores_and_publishes_projection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let nats = NatsStoreConfig::for_org("TestOrg", crate::config::runtime::DEFAULT_NATS_URL)
            .expect("nats config");
        let state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats)
            .await
            .expect("with stores");

        state
            .record_org(org_snapshot_fixture())
            .expect("ordinary org success path is unchanged by primary-error propagation");

        assert_eq!(
            org_events_in_primary_store(&state, "TestOrg"),
            1,
            "success still appends the org event to the primary store"
        );
        assert_eq!(
            org_count_of(&state),
            1,
            "success still appends the org event to the dedicated store"
        );
        assert!(
            state.lock_projection().org_state.is_some(),
            "success still publishes the org snapshot into the resident projection"
        );
    }

    #[tokio::test]
    async fn resync_event_store_re_seeds_org_and_team_stores_not_just_repos() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let nats = NatsStoreConfig::for_org("TestOrg", crate::config::runtime::DEFAULT_NATS_URL)
            .expect("nats config");
        let state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats.clone())
            .await
            .expect("with stores");
        let seed_repo = crate::test_fixtures::all_passing_evidence("seed-repo");
        state
            .record_repo(
                &seed_repo.repository.inventory_key,
                seed_repo.clone(),
                &seed_repo.repository.name,
                "2026-06-11T00:00:00Z",
            )
            .expect("seed repo store so its pgno file is non-empty before the second handle opens");

        {
            let other_writer =
                AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats.clone())
                    .await
                    .expect("second handle opens against the same events_dir");
            let mut metadata = crate::test_fixtures::make_metadata();
            metadata.organization = "TestOrg".to_string();
            other_writer
                .record_org(crate::domain::evidence::OrgStateSnapshot {
                    archived_repos: 3,
                    assessment_metadata: metadata,
                    alert_summary: empty_org_summary(),
                })
                .expect("record org via second handle");
            let roster = team_roster_fixture("@TestOrg/platform", "platform");
            other_writer
                .record_team(
                    "TestOrg",
                    &roster,
                    "2026-07-16T00:00:00Z",
                    OrgMembershipFetchStatus::Fetched,
                )
                .expect("record team via second handle");
        }

        let org_count_before = state
            .org_event_store
            .fold_events(0_usize, |acc, _| *acc += 1)
            .expect("fold org events before resync");
        let team_count_before = state
            .team_event_store
            .fold_events(0_usize, |acc, _| *acc += 1)
            .expect("fold team events before resync");
        assert_eq!(
            (org_count_before, team_count_before),
            (0, 0),
            "long-lived handles must not see the externally-durable org/team writes before resync"
        );

        state
            .resync_event_store(&events_dir, PardosaBackend::Pgno, nats)
            .await
            .expect("resync all three stores");

        let org_count_after = state
            .org_event_store
            .fold_events(0_usize, |acc, _| *acc += 1)
            .expect("fold org events after resync");
        let team_count_after = state
            .team_event_store
            .fold_events(0_usize, |acc, _| *acc += 1)
            .expect("fold team events after resync");
        assert_eq!(
            (org_count_after, team_count_after),
            (1, 1),
            "resync_event_store must re-seed org and team stores, not just repos — \
             an org/team-origin FencedConflict converges through the same catch-all"
        );
    }

    fn plant_stale_local_pgno_sentinel(
        rt: &Arc<tokio::runtime::Runtime>,
        events_dir: &Path,
    ) -> crate::domain::evidence::RepositoryEvidence {
        let sentinel = crate::test_fixtures::all_passing_evidence("stale-local-pgno-sentinel");
        let pgno_nats =
            NatsStoreConfig::for_org("TestOrg", crate::config::runtime::DEFAULT_NATS_URL)
                .expect("pgno-side nats config");
        let planted = rt
            .block_on(AppState::with_stores(
                events_dir,
                PardosaBackend::Pgno,
                pgno_nats,
            ))
            .expect("plant local pgno stores in the same events_dir");
        planted
            .record_repo(
                &sentinel.repository.inventory_key,
                sentinel.clone(),
                &sentinel.repository.name,
                "2026-01-01T00:00:00Z",
            )
            .expect("seed a stale sentinel fact into events.pgno");
        sentinel
    }

    fn record_authoritative_nats_facts(
        rt: &Arc<tokio::runtime::Runtime>,
        events_dir: &Path,
        nats: NatsStoreConfig,
    ) -> crate::domain::evidence::RepositoryEvidence {
        let authoritative = crate::test_fixtures::all_passing_evidence("nats-authoritative-repo");
        let other_writer = rt
            .block_on(AppState::with_stores_and_runtime(
                events_dir,
                PardosaBackend::Nats,
                nats,
                Some(Arc::clone(rt)),
            ))
            .expect("second nats handle against the same streams");
        other_writer
            .record_repo(
                &authoritative.repository.inventory_key,
                authoritative.clone(),
                &authoritative.repository.name,
                "2026-07-16T00:00:00Z",
            )
            .expect("record repo durably in NATS via the second handle");
        let mut metadata = crate::test_fixtures::make_metadata();
        metadata.organization = "TestOrg".to_string();
        other_writer
            .record_org(crate::domain::evidence::OrgStateSnapshot {
                archived_repos: 3,
                assessment_metadata: metadata,
                alert_summary: empty_org_summary(),
            })
            .expect("record org durably in NATS via the second handle");
        let roster = team_roster_fixture("@TestOrg/platform", "platform");
        other_writer
            .record_team(
                "TestOrg",
                &roster,
                "2026-07-16T00:00:00Z",
                OrgMembershipFetchStatus::Fetched,
            )
            .expect("record team durably in NATS via the second handle");
        authoritative
    }

    #[test]
    fn nats_resync_event_store_reads_nats_stores_not_stale_local_pgno_files() {
        let Some(server) = crate::store::tests::TestNatsServer::spawn() else {
            return;
        };

        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
        );

        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");

        let sentinel = plant_stale_local_pgno_sentinel(&rt, &events_dir);

        let nats = NatsStoreConfig::for_org("TestOrg", &server.url).expect("live nats config");
        let state = rt
            .block_on(AppState::with_stores_and_runtime(
                &events_dir,
                PardosaBackend::Nats,
                nats.clone(),
                Some(Arc::clone(&rt)),
            ))
            .expect("nats-backed app state");

        let authoritative = record_authoritative_nats_facts(&rt, &events_dir, nats.clone());

        rt.block_on(state.resync_event_store(&events_dir, PardosaBackend::Nats, nats))
            .expect("resync all three NATS stores");

        let repo_keys = state
            .event_store
            .fold_events(Vec::<String>::new(), |acc, _detached, event| {
                if let crate::event::DomainEvent::RepositoryStateCaptured { domain_key, .. } = event
                {
                    acc.push(domain_key.as_str().to_string());
                }
            })
            .expect("fold repo events after resync");
        assert!(
            repo_keys
                .iter()
                .all(|key| key != sentinel.repository.inventory_key.as_str()),
            "a stale local events.pgno sentinel must never contaminate a NATS resync; \
             recovery must read the configured NATS store through its own retained backend"
        );
        assert!(
            repo_keys
                .iter()
                .any(|key| key == authoritative.repository.inventory_key.as_str()),
            "NATS resync must load the authoritative repository fact durably written to NATS"
        );

        let org_facts = state
            .org_event_store
            .fold_events(Vec::<u32>::new(), |acc, event| {
                acc.push(event.archived_repos);
            })
            .expect("fold org events after resync");
        let team_facts = state
            .team_event_store
            .fold_events(Vec::<(String, String)>::new(), |acc, event| {
                acc.push((
                    event.org.as_str().to_string(),
                    event.team_slug.as_str().to_string(),
                ));
            })
            .expect("fold team events after resync");
        assert_eq!(
            org_facts,
            vec![3_u32],
            "the org store must recover the authoritative org content from its own NATS \
             backend, not merely some event count"
        );
        assert_eq!(
            team_facts,
            vec![("TestOrg".to_string(), "platform".to_string())],
            "the team store must recover the authoritative team identity from its own NATS \
             backend, not merely some event count"
        );

        drop(state);
        drop(server);
    }

    #[test]
    fn nats_resync_event_store_recovers_when_no_local_pgno_files_exist_at_all() {
        let Some(server) = crate::store::tests::TestNatsServer::spawn() else {
            return;
        };

        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let nats = NatsStoreConfig::for_org("TestOrg", &server.url).expect("live nats config");

        let state = rt
            .block_on(AppState::with_stores_and_runtime(
                &events_dir,
                PardosaBackend::Nats,
                nats.clone(),
                Some(Arc::clone(&rt)),
            ))
            .expect("nats-backed app state");

        assert!(
            !events_dir.join("events.pgno").exists(),
            "this case must have no local pgno artefact to fall back to"
        );

        let authoritative = record_authoritative_nats_facts(&rt, &events_dir, nats.clone());

        rt.block_on(state.resync_event_store(&events_dir, PardosaBackend::Nats, nats))
            .expect("a NATS resync must not require any local pgno file to exist");

        assert!(
            repo_keys_of(&state)
                .iter()
                .any(|key| key == authoritative.repository.inventory_key.as_str()),
            "NATS recovery must succeed from the NATS backend with no local files present"
        );
        assert_eq!(
            (org_count_of(&state), team_count_of(&state)),
            (1, 1),
            "all three NATS stores must recover without any local pgno artefact"
        );

        drop(state);
        drop(server);
    }

    #[test]
    fn resync_event_store_keeps_per_store_caches_when_a_middle_store_fails() {
        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let nats = NatsStoreConfig::for_org("TestOrg", crate::config::runtime::DEFAULT_NATS_URL)
            .expect("nats config");

        let state = rt
            .block_on(AppState::with_stores(
                &events_dir,
                PardosaBackend::Pgno,
                nats.clone(),
            ))
            .expect("long-lived pgno app state");

        let authoritative = record_authoritative_pgno_facts(&rt, &events_dir, nats.clone());

        assert_eq!(
            (
                repo_keys_of(&state).len(),
                org_count_of(&state),
                team_count_of(&state)
            ),
            (0, 0, 0),
            "the long-lived handle must not see the externally durable writes before resync"
        );

        crate::store::tests::overwrite_with_mismatched_descriptor_store(
            &events_dir.join("org-events.pgno"),
        );

        let err = rt
            .block_on(state.resync_event_store(&events_dir, PardosaBackend::Pgno, nats))
            .expect_err("a mismatched org store must fail the re-arm");
        assert!(
            err.to_string()
                .contains("schema descriptor identity mismatch"),
            "the failure must be the org store's schema rejection, got: {err}"
        );

        assert!(
            repo_keys_of(&state)
                .iter()
                .any(|key| key == authoritative.repository.inventory_key.as_str()),
            "the repo store is refreshed before the org store fails; partial progress is \
             permitted and must be observable, not silently rolled back"
        );
        assert_eq!(
            org_count_of(&state),
            0,
            "the failing org store must keep its previous cache — validate and decode fully \
             before replacing it"
        );
        assert_eq!(
            team_count_of(&state),
            0,
            "the team store is never attempted after the org store fails; a successful team \
             refresh would have loaded the externally durable team fact"
        );
    }

    fn record_authoritative_pgno_facts(
        rt: &Arc<tokio::runtime::Runtime>,
        events_dir: &Path,
        nats: NatsStoreConfig,
    ) -> crate::domain::evidence::RepositoryEvidence {
        let authoritative = crate::test_fixtures::all_passing_evidence("pgno-authoritative-repo");
        let other_writer = rt
            .block_on(AppState::with_stores(
                events_dir,
                PardosaBackend::Pgno,
                nats,
            ))
            .expect("second pgno handle against the same events_dir");
        other_writer
            .record_repo(
                &authoritative.repository.inventory_key,
                authoritative.clone(),
                &authoritative.repository.name,
                "2026-07-16T00:00:00Z",
            )
            .expect("record repo durably via the second handle");
        let roster = team_roster_fixture("@TestOrg/platform", "platform");
        other_writer
            .record_team(
                "TestOrg",
                &roster,
                "2026-07-16T00:00:00Z",
                OrgMembershipFetchStatus::Fetched,
            )
            .expect("record team durably via the second handle");
        authoritative
    }

    fn repo_keys_of(state: &AppState) -> Vec<String> {
        state
            .event_store
            .fold_events(Vec::<String>::new(), |acc, _detached, event| {
                if let crate::event::DomainEvent::RepositoryStateCaptured { domain_key, .. } = event
                {
                    acc.push(domain_key.as_str().to_string());
                }
            })
            .expect("fold repo events")
    }

    fn org_count_of(state: &AppState) -> usize {
        state
            .org_event_store
            .fold_events(0_usize, |acc, _| *acc += 1)
            .expect("fold org events")
    }

    fn team_count_of(state: &AppState) -> usize {
        state
            .team_event_store
            .fold_events(0_usize, |acc, _| *acc += 1)
            .expect("fold team events")
    }

    #[tokio::test]
    async fn reconstruct_org_state_from_dual_event_logs_without_live_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let nats = NatsStoreConfig::for_org("TestOrg", crate::config::runtime::DEFAULT_NATS_URL)
            .expect("nats config");
        let state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats.clone())
            .await
            .expect("with stores");
        let mut repo = crate::test_fixtures::all_passing_evidence("event-repo");
        repo.repository.archived = true;
        let domain_key = repo.repository.inventory_key.clone();
        let repo_name = repo.repository.name.clone();
        state
            .record_repo(&domain_key, repo, &repo_name, "2026-06-11T00:00:00Z")
            .expect("record repo");

        let mut metadata = crate::test_fixtures::make_metadata();
        metadata.organization = "TestOrg".to_string();
        metadata.run_id = "org-run-from-event".to_string();
        let mut alert_summary = empty_org_summary();
        alert_summary.total_open_secret_alerts = 9;
        state
            .record_org(crate::domain::evidence::OrgStateSnapshot {
                archived_repos: 7,
                assessment_metadata: metadata.clone(),
                alert_summary,
            })
            .expect("record org state");
        assert!(events_dir.join("events.pgno").exists());
        assert!(events_dir.join("org-events.pgno").exists());
        drop(state);

        let restarted = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats)
            .await
            .expect("restart from event logs");
        let projection = restarted.lock_projection().clone();
        let org = projection
            .org_state
            .clone()
            .expect("org state must replay from org event log");

        assert_eq!(projection.repositories.len(), 1);
        assert_eq!(org.archived_repos, 7);
        assert_eq!(org.assessment_metadata.run_id, metadata.run_id);
        assert_eq!(org.alert_summary.total_open_secret_alerts, 9);
        assert!(
            !restarted.is_ready(),
            "replayed repo and org events are not ready before root publication",
        );
        let evidence = crate::domain::evidence::Evidence {
            assessment_metadata: org.assessment_metadata,
            collection_statistics: crate::domain::metrics::CollectionStatistics {
                total_repos: 0,
                public_repos: 0,
                internal_repos: 0,
                private_repos: 0,
                archived_repos: org.archived_repos,
            },
            metrics: crate::test_fixtures::make_minimal_metrics(),
            secret_scanning_observability:
                crate::aggregate::metrics::build_secret_scanning_observability_summary(
                    &[],
                    Some(&org.alert_summary),
                ),
            repositories: projection.sorted_snapshot(),
            deleted: projection
                .deleted_snapshot()
                .into_iter()
                .map(|(_, record)| record)
                .collect(),
        };
        let pages = crate::report::html::render_dashboard(
            &evidence,
            &crate::config::dashboard::DashboardConfig::default(),
        )
        .expect("render replayed org state");
        assert!(pages["report.html"].contains("org-run-from-event"));
        assert!(pages["index.html"].contains("7 archived"));
        assert!(pages["index.html"].contains("9 open org alerts"));
    }

    #[tokio::test]
    async fn org_only_event_log_is_not_ready_before_root_publication() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let nats = NatsStoreConfig::for_org("TestOrg", crate::config::runtime::DEFAULT_NATS_URL)
            .expect("nats config");
        let state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats.clone())
            .await
            .expect("with stores");
        let mut metadata = crate::test_fixtures::make_metadata();
        metadata.organization = "TestOrg".to_string();
        metadata.run_id = "org-only-run".to_string();
        state
            .record_org(crate::domain::evidence::OrgStateSnapshot {
                archived_repos: 3,
                assessment_metadata: metadata,
                alert_summary: empty_org_summary(),
            })
            .expect("record org state");
        drop(state);

        let restarted = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats)
            .await
            .expect("restart from org event log");
        {
            let projection = restarted.lock_projection();
            assert_eq!(projection.repositories.len(), 0);
            assert_eq!(
                projection
                    .org_state
                    .as_ref()
                    .expect("org state")
                    .archived_repos,
                3
            );
        }
        assert!(
            !restarted.is_ready(),
            "org-only event-log projection is not ready before root publication",
        );
    }

    #[tokio::test]
    async fn cold_start_from_three_empty_stores_reaches_ready_after_root_publication() {
        use cherry_pit_web::serve::ServerState;

        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let nats = NatsStoreConfig::for_org("TestOrg", crate::config::runtime::DEFAULT_NATS_URL)
            .expect("nats config");
        let state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats)
            .await
            .expect("cold start must open all three stores from empty");

        let repo_count = state
            .event_store
            .fold_events(0_usize, |acc, _, _| *acc += 1)
            .expect("fold repo events");
        let org_count = state
            .org_event_store
            .fold_events(0_usize, |acc, _| *acc += 1)
            .expect("fold org events");
        let team_count = state
            .team_event_store
            .fold_events(0_usize, |acc, _| *acc += 1)
            .expect("fold team events");
        assert_eq!(
            (repo_count, org_count, team_count),
            (0, 0, 0),
            "this test characterises a genuinely empty three-store cold start, \
             with no pre-seeded repository, org, or team evidence"
        );
        assert!(state.lock_projection().is_empty());

        assert!(
            !state.is_ready(),
            "an empty cold start is not ready before root publication"
        );

        let mut metadata = crate::test_fixtures::make_metadata();
        metadata.organization = "TestOrg".to_string();
        metadata.run_id = "cold-start-first-run".to_string();

        state
            .record_org(crate::domain::evidence::OrgStateSnapshot {
                archived_repos: 0,
                assessment_metadata: metadata,
                alert_summary: empty_org_summary(),
            })
            .expect("first run records org state");

        assert!(
            !state.is_ready(),
            "recording org evidence alone must not claim content readiness"
        );
        state.set_html_cache(HashMap::from([(
            "index.html".to_string(),
            CachedPage::new("index.html", b"<html>report</html>".to_vec()),
        )]));
        assert!(
            state.is_ready(),
            "published root content makes the cold start ready"
        );
    }

    #[tokio::test]
    async fn line_order_replay_matches_live_projection_after_detach() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let state = AppState::with_stores(
            &events_dir,
            PardosaBackend::Pgno,
            NatsStoreConfig::for_org("org", crate::config::runtime::DEFAULT_NATS_URL).unwrap(),
        )
        .await
        .expect("with_stores");
        let timestamp = "2026-06-11T00:00:00Z";
        let removed = crate::test_fixtures::all_passing_evidence("removed-repo");
        let kept_a = crate::test_fixtures::all_passing_evidence("kept-a");
        let kept_b = crate::test_fixtures::all_passing_evidence("kept-b");

        for evidence in [removed.clone(), kept_a.clone(), kept_b.clone()] {
            let domain_key = evidence.repository.inventory_key.clone();
            let repo_name = evidence.repository.name.clone();
            state
                .record_repo(&domain_key, evidence, &repo_name, timestamp)
                .expect("record repo");
        }
        state
            .remove_repo(
                &removed.repository.inventory_key,
                &removed.repository.name,
                timestamp,
            )
            .expect("remove repo");

        let events = state.event_store.events().expect("line-order events");
        assert!(events.iter().any(|(detached, _)| *detached));
        let replayed = fold_public_event_stream(events).sorted_snapshot();
        let live = state.projection_snapshot();

        assert_eq!(replayed, live);
        assert!(
            !live
                .iter()
                .any(|e| e.repository.inventory_key == removed.repository.inventory_key)
        );
        assert!(
            live.iter()
                .any(|e| e.repository.inventory_key == kept_a.repository.inventory_key)
        );
        assert!(
            live.iter()
                .any(|e| e.repository.inventory_key == kept_b.repository.inventory_key)
        );
    }

    #[tokio::test]
    async fn mark_repo_deleted_writes_event_and_replay_rebuilds_deleted_projection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let nats = NatsStoreConfig::for_org("org", crate::config::runtime::DEFAULT_NATS_URL)
            .expect("nats config");
        let state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats.clone())
            .await
            .expect("with_stores");
        let timestamp = "2026-06-24T00:00:00Z";
        let deleted = crate::test_fixtures::all_passing_evidence("write-wrapper-deleted");
        let domain_key = deleted.repository.inventory_key.clone();
        let repo_name = deleted.repository.name.clone();

        state
            .record_repo(&domain_key, deleted, &repo_name, timestamp)
            .expect("record repo");
        state
            .mark_repo_deleted(&domain_key, &repo_name, timestamp)
            .expect("mark repo deleted");

        assert!(!state.projection_contains(&domain_key));
        assert!(state.projection_deleted_contains(&domain_key));
        let deleted_snapshot = state.projection_deleted_snapshot();
        let deleted_record = deleted_snapshot
            .iter()
            .find(|(key, _)| key == &domain_key)
            .map(|(_, record)| record)
            .expect("deleted record");
        assert_eq!(deleted_record.repo_name, repo_name);
        assert_eq!(deleted_record.detected_at, timestamp);

        drop(state);
        let restarted = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats)
            .await
            .expect("restart");
        assert!(!restarted.projection_contains(&domain_key));
        assert!(restarted.projection_deleted_contains(&domain_key));
    }

    #[tokio::test]
    async fn detached_repository_drops_from_rendered_report_after_refold() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let state = AppState::with_stores(
            &events_dir,
            PardosaBackend::Pgno,
            NatsStoreConfig::for_org("org", crate::config::runtime::DEFAULT_NATS_URL).unwrap(),
        )
        .await
        .expect("with_stores");
        let timestamp = "2026-06-11T00:00:00Z";
        let removed = crate::test_fixtures::make_repository_evidence(
            "removed-render-repo",
            crate::domain::repository::Visibility::Public,
            false,
            crate::test_fixtures::make_checks(
                crate::test_fixtures::policy_pass_setting(),
                crate::test_fixtures::secret_enabled_observable(false),
                crate::test_fixtures::dependabot_enabled(),
                crate::test_fixtures::branch_pass(),
                crate::test_fixtures::codeowners_absent(),
            ),
        );
        let kept = crate::test_fixtures::make_repository_evidence(
            "kept-render-repo",
            crate::domain::repository::Visibility::Public,
            false,
            crate::test_fixtures::make_checks(
                crate::test_fixtures::policy_pass_setting(),
                crate::test_fixtures::secret_enabled_observable(false),
                crate::test_fixtures::dependabot_enabled(),
                crate::test_fixtures::branch_pass(),
                crate::test_fixtures::codeowners_absent(),
            ),
        );

        for evidence in [removed.clone(), kept.clone()] {
            let domain_key = evidence.repository.inventory_key.clone();
            let repo_name = evidence.repository.name.clone();
            state
                .record_repo(&domain_key, evidence, &repo_name, timestamp)
                .expect("record repo");
        }
        let before = rendered_evidence_from_projection(state.projection_snapshot());
        let before_pages = crate::report::html::render_dashboard(
            &before,
            &crate::config::dashboard::DashboardConfig::default(),
        )
        .expect("render before detach");
        assert!(before_pages["orphans.html"].contains("removed-render-repo"));
        assert!(before_pages["orphans.html"].contains("kept-render-repo"));

        state
            .remove_repo(
                &removed.repository.inventory_key,
                &removed.repository.name,
                timestamp,
            )
            .expect("remove repo");
        state.refresh_projection().expect("refold after detach");
        let after = rendered_evidence_from_projection(state.projection_snapshot());
        let after_pages = crate::report::html::render_dashboard(
            &after,
            &crate::config::dashboard::DashboardConfig::default(),
        )
        .expect("render after detach");

        assert!(!after_pages["orphans.html"].contains("removed-render-repo"));
        assert!(after_pages["orphans.html"].contains("kept-render-repo"));
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
        assert!(state.webhook.secret.is_some());
        let cache = &state.github.repo_detail_cache;
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
        use cherry_pit_web::serve::ServerState;
        let state = AppStateBuilder::new().build().await;
        assert!(
            !state.is_ready(),
            "should not be ready with no run and no cache"
        );
    }

    #[tokio::test]
    async fn is_ready_true_when_html_cache_populated() {
        use cherry_pit_web::serve::ServerState;
        let state = AppStateBuilder::new().build().await;
        let mut pages = HashMap::new();
        pages.insert(
            "index.html".to_string(),
            CachedPage::new("index.html", b"<html>test</html>".to_vec()),
        );
        state.evidence.html_cache.store(Arc::new(Some(pages)));
        assert!(state.is_ready(), "should be ready when html_cache is Some");
    }

    #[tokio::test]
    async fn is_ready_false_when_cache_populated_but_jetstream_connect_failed() {
        use cherry_pit_web::serve::ServerState;
        let state = AppStateBuilder::new().build().await;
        let mut pages = HashMap::new();
        pages.insert(
            "index.html".to_string(),
            CachedPage::new("index.html", b"<html>cached</html>".to_vec()),
        );
        state.evidence.html_cache.store(Arc::new(Some(pages)));
        state.event_store.mark_backend_connect_failure_for_test();

        assert!(
            !state.is_ready(),
            "warm cache must not mask last-known JetStream connect failure"
        );
    }

    #[tokio::test]
    async fn sweep_lock_serialises_concurrent_acquirers() {
        use std::time::{Duration, Instant};

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
                .evidence
                .org_summary
                .store(Arc::new(Some(Arc::clone(&summary_for_task_a))));
            tokio::time::sleep(hold_for).await;
            let guard_after_hold = state_a.evidence.org_summary.load_full();
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
            let guard_at_acquire = state_b.evidence.org_summary.load_full();
            let observed_at_acquire = (*guard_at_acquire)
                .as_ref()
                .map(Arc::clone)
                .expect("A set first");
            state_b
                .evidence
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

    #[tokio::test]
    async fn drain_worker_pool_aborts_pool_handle_after_timeout() {
        use std::time::Duration;

        let state = AppState::new().await;
        let pool_handle = tokio::spawn(std::future::pending::<()>());
        let pool_abort_probe = pool_handle.abort_handle();
        let delivery_handle = tokio::spawn(async {});
        assert!(
            state
                .worker_pool_started
                .set(std::sync::Mutex::new(Some((pool_handle, delivery_handle))))
                .is_ok()
        );

        let drained = tokio::time::timeout(
            Duration::from_millis(250),
            state.drain_worker_pool(Duration::from_millis(50)),
        )
        .await
        .expect("drain should return within outer bound");

        assert!(!drained.0, "stuck pool handle should report timed out");
        assert!(drained.1, "trivial delivery handle should drain");
        tokio::time::timeout(Duration::from_millis(50), async {
            while !pool_abort_probe.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("timed-out pool task should be aborted, not detached");
    }

    #[tokio::test(start_paused = true)]
    async fn drain_worker_pool_uses_one_budget_for_pool_and_delivery() {
        let state = AppState::new().await;
        let pool_handle = tokio::spawn(std::future::pending::<()>());
        let delivery_handle = tokio::spawn(std::future::pending::<()>());
        assert!(
            state
                .worker_pool_started
                .set(std::sync::Mutex::new(Some((pool_handle, delivery_handle))))
                .is_ok()
        );
        let timeout = std::time::Duration::from_secs(3);
        let started = tokio::time::Instant::now();

        let drained = state.drain_worker_pool(timeout).await;

        let elapsed = started.elapsed();
        assert_eq!(drained, (false, false));
        assert!(
            elapsed <= timeout + std::time::Duration::from_millis(1),
            "worker and delivery drains must share one timeout budget; elapsed={elapsed:?}, budget={timeout:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn drain_worker_pool_returns_when_handles_finish_before_budget() {
        let state = AppState::new().await;
        let pool_handle = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        });
        let delivery_handle = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        });
        assert!(
            state
                .worker_pool_started
                .set(std::sync::Mutex::new(Some((pool_handle, delivery_handle))))
                .is_ok()
        );
        let timeout = std::time::Duration::from_secs(3);
        let started = tokio::time::Instant::now();

        let drained = state.drain_worker_pool(timeout).await;

        let elapsed = started.elapsed();
        assert_eq!(drained, (true, true));
        assert!(
            elapsed < std::time::Duration::from_millis(20),
            "cooperative handles should finish before the drain budget; elapsed={elapsed:?}, budget={timeout:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn drain_worker_pool_flushes_queued_delivery_outcome() {
        let state = AppState::new().await;
        let run_id = uuid::Uuid::now_v7();
        state.evidence().scheduled_run.store(Arc::new(Some(Arc::new(
            crate::app::evidence_service::ScheduledRun {
                id: run_id,
                tracker: crate::app::work_queue::BatchTracker::new(1),
                failure: std::sync::Mutex::new(None),
            },
        ))));
        let (outcome_tx, outcome_rx) = tokio::sync::mpsc::channel(1);
        outcome_tx
            .send(crate::app::worker_pool::JobOutcome::Success {
                domain_key: "queued-repo".to_string(),
                result: crate::test_fixtures::all_passing_evidence("queued-repo"),
                source: crate::app::work_queue::JobSource::ScheduledBatch,
                duration: std::time::Duration::from_millis(1),
                correlation: cherry_pit_core::CorrelationContext::correlated(run_id),
            })
            .await
            .expect("queue outcome");
        drop(outcome_tx);
        let pool_handle = tokio::spawn(std::future::pending::<()>());
        let delivery_state = Arc::clone(&state);
        let delivery_handle = tokio::spawn(crate::app::daemon::delivery_loop(
            outcome_rx,
            delivery_state,
        ));
        assert!(
            state
                .worker_pool_started
                .set(std::sync::Mutex::new(Some((pool_handle, delivery_handle))))
                .is_ok()
        );

        let drained = state
            .drain_worker_pool(std::time::Duration::from_secs(3))
            .await;

        assert_eq!(drained, (false, true));
        assert!(
            state.projection_contains("queued-repo"),
            "concurrent drain must not drop an already queued delivery outcome"
        );
    }

    #[tokio::test]
    async fn status_payload_identifies_coverage_provenance_faithfully() {
        let state = AppState::new_with_cache_capacity(10).await;
        let empty_payload = state.status_payload();
        assert_eq!(empty_payload["coverage"]["provenance"], "unknown");
        assert_eq!(empty_payload["coverage"]["capped"], false);
        assert!(empty_payload["coverage"]["selected"].is_null());

        let mut completed = RunMetadata::new("Org".to_string(), "1.0".to_string());
        completed.complete();
        completed.coverage = crate::domain::evidence::CollectionCoverage::known(
            50,
            std::num::NonZeroU64::new(100).unwrap(),
        );
        state.last_completed_run.store(Arc::new(Some(completed)));

        let mut current_unknown = RunMetadata::new("Org".to_string(), "1.0".to_string());
        current_unknown.coverage = crate::domain::evidence::CollectionCoverage::Unknown;
        state.current_run.store(Arc::new(Some(current_unknown)));

        let active_unknown_payload = state.status_payload();
        assert_eq!(
            active_unknown_payload["coverage"]["provenance"], "unknown",
            "active run with unknown coverage must not borrow completed run coverage"
        );
        assert_eq!(active_unknown_payload["coverage"]["capped"], false);

        state.current_run.store(Arc::new(None));
        let completed_payload = state.status_payload();
        assert_eq!(
            completed_payload["coverage"]["provenance"],
            "last_completed_run"
        );
        assert_eq!(completed_payload["coverage"]["capped"], false);
        assert_eq!(completed_payload["coverage"]["selected"], 50);
        assert_eq!(completed_payload["coverage"]["total"], 50);
        assert_eq!(completed_payload["coverage"]["limit"], 100);

        let mut current_known = RunMetadata::new("Org".to_string(), "1.0".to_string());
        current_known.coverage = crate::domain::evidence::CollectionCoverage::known(
            776,
            std::num::NonZeroU64::new(10).unwrap(),
        );
        state.current_run.store(Arc::new(Some(current_known)));

        let current_payload = state.status_payload();
        assert_eq!(current_payload["coverage"]["provenance"], "current_run");
        assert_eq!(current_payload["coverage"]["capped"], true);
        assert_eq!(current_payload["coverage"]["selected"], 10);
        assert_eq!(current_payload["coverage"]["total"], 776);
        assert_eq!(current_payload["coverage"]["limit"], 10);

        state.current_run.store(Arc::new(None));
        state.last_completed_run.store(Arc::new(None));
        let mut org_event = synthetic_org_state("Org");
        org_event.assessment_metadata.coverage = crate::event::CollectionCoverage::Known {
            total: 200,
            limit: std::num::NonZeroU64::new(25).unwrap(),
        };
        state.fold_org_event_into_projection(org_event);
        let durable_payload = state.status_payload();
        assert_eq!(
            durable_payload["coverage"]["provenance"],
            "durable_assessment"
        );
        assert_eq!(durable_payload["coverage"]["capped"], true);
        assert_eq!(durable_payload["coverage"]["selected"], 25);
        assert_eq!(durable_payload["coverage"]["total"], 200);
        assert_eq!(durable_payload["coverage"]["limit"], 25);

        let mut completed_unknown = RunMetadata::new("Org".to_string(), "1.0".to_string());
        completed_unknown.complete();
        completed_unknown.coverage = crate::domain::evidence::CollectionCoverage::Unknown;
        state
            .last_completed_run
            .store(Arc::new(Some(completed_unknown)));
        let completed_unknown_payload = state.status_payload();
        assert_eq!(
            completed_unknown_payload["coverage"]["provenance"], "unknown",
            "completed run with unknown coverage must not borrow durable assessment"
        );
        assert_eq!(completed_unknown_payload["coverage"]["capped"], false);
    }
}
