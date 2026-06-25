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
use std::error::Error;
use std::fmt::Write as _;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;

use arc_swap::ArcSwap;
use cherry_pit_core::ReadPort;
use jiff::Timestamp;
use pardosa::store::JetStreamBackend as PardosaJetStreamBackend;
use pardosa::store::RecoveryOutcome;
use pardosa_nats::{JetStreamBackend as SubstrateJetStreamBackend, JetStreamConfig, RuntimeHandle};
use pardosa_schema::{NonEmptyEventString, Timestamp as EventTimestamp};
use sha2::{Digest, Sha256};

pub use crate::infra::server::state::{CachedPage, PageUpdateEvent};

pub type EventStoreImpl = crate::store::NativeStore;
pub type OrgEventStoreImpl = crate::store::NativeOrgStore;

pub use crate::app::evidence_service::EvidenceState;
pub use crate::app::github_infra::GithubState;
pub use crate::app::webhook_context::WebhookState;

use crate::app::collect::JobContext;
use crate::app::work_queue::WorkQueue;
use crate::domain::evidence::RepositoryEvidence;
use crate::domain::run::RunMetadata;
use crate::error::PersistenceError;
use crate::event::convert::EventConversionError;
use crate::event::{DomainEvent as NativeDomainEvent, OrgStateCaptured};

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
/// Access grouped fields via behavior methods that hide sub-aggregate storage.
pub(crate) type WorkerPoolHandles =
    std::sync::Mutex<Option<(tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()>)>>;
pub(crate) type WorkerShutdownToken = tokio_util::sync::CancellationToken;

pub struct AppState {
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

    /// Durable native pardosa event store.
    pub event_store: Arc<EventStoreImpl>,

    /// Durable native pardosa org event store.
    pub org_event_store: Arc<OrgEventStoreImpl>,

    /// Materialised projection state rebuilt from [`Self::event_store`].
    pub(crate) projection_state: Arc<Mutex<crate::projection::EvidenceProjection>>,

    /// Webhook ingestion concerns (secret, replay, debounce).
    webhook: WebhookState,
    /// GitHub API infrastructure (budget, rate limit, client, cache).
    github: GithubState,
    /// Evidence data store and publication infrastructure.
    evidence: EvidenceState,

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

impl LastRecoveryStatus {
    fn from_outcome(store: &'static str, recovery: &RecoveryOutcome) -> Self {
        Self {
            at: Timestamp::now(),
            store,
            reader_error: recovery.reader_error.as_str(),
            recovered_records: recovery.recovered_records,
            truncated_bytes: recovery.truncated_bytes,
            last_durable_offset: recovery.last_durable_offset,
            manifest_message_count: recovery.manifest_message_count,
        }
    }
}

impl AppState {
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
    pub(crate) fn ws_subscribe(&self) -> tokio::sync::broadcast::Receiver<PageUpdateEvent> {
        self.evidence.ws_broadcast.subscribe()
    }

    pub(crate) fn set_active_batch_tracker(
        &self,
        tracker: Option<Arc<crate::app::work_queue::BatchTracker>>,
    ) {
        self.evidence.batch_tracker.store(Arc::new(tracker));
    }

    pub(crate) fn complete_active_batch(&self) {
        let tracker_guard = self.evidence.batch_tracker.load();
        if let Some(tracker) = tracker_guard.as_ref() {
            tracker.complete_one();
        }
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
    pub(crate) fn projection_len(&self) -> usize {
        let projection = self.lock_projection();
        match crate::projection::EvidenceProjectionReadPort::resolve(
            &projection,
            crate::projection::EvidenceProjectionQuery::Len,
        ) {
            crate::projection::EvidenceProjectionResponse::Len(len) => len,
            _ => 0,
        }
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
        let projection = self.lock_projection();
        match crate::projection::EvidenceProjectionReadPort::resolve(
            &projection,
            crate::projection::EvidenceProjectionQuery::ByKey(key.to_string()),
        ) {
            crate::projection::EvidenceProjectionResponse::One(evidence) => *evidence,
            _ => None,
        }
    }

    /// True when `key` is materialised in `projection_state`.
    ///
    /// Lock-and-release accessor; equivalent to
    /// `self.projection_get(key).is_some()` but avoids the clone.
    /// Guard does not escape (D-CD-3); panics on poisoned mutex.
    pub(crate) fn projection_contains(&self, key: &str) -> bool {
        let projection = self.lock_projection();
        match crate::projection::EvidenceProjectionReadPort::resolve(
            &projection,
            crate::projection::EvidenceProjectionQuery::Contains(key.to_string()),
        ) {
            crate::projection::EvidenceProjectionResponse::Contains(contains) => contains,
            _ => false,
        }
    }

    /// Sorted snapshot of all evidence in `projection_state`.
    ///
    /// Lock-and-release wrapper over
    /// [`crate::projection::EvidenceProjection::sorted_snapshot`]; the
    /// guard does not escape (D-CD-3). Panics on poisoned mutex. Cost
    /// is `O(n log n)` per call; see the underlying method for
    /// ordering rationale.
    pub(crate) fn projection_snapshot(&self) -> Vec<crate::domain::evidence::RepositoryEvidence> {
        let projection = self.lock_projection();
        match crate::projection::EvidenceProjectionReadPort::resolve(
            &projection,
            crate::projection::EvidenceProjectionQuery::SortedSnapshot,
        ) {
            crate::projection::EvidenceProjectionResponse::Many(evidence) => evidence,
            _ => Vec::new(),
        }
    }

    pub(crate) fn projection_deleted_snapshot(
        &self,
    ) -> Vec<(String, crate::projection::DeletedRepoRecord)> {
        let projection = self.lock_projection();
        match crate::projection::EvidenceProjectionReadPort::resolve(
            &projection,
            crate::projection::EvidenceProjectionQuery::DeletedSnapshot,
        ) {
            crate::projection::EvidenceProjectionResponse::Deleted(deleted) => deleted,
            _ => Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn projection_deleted_contains(&self, key: &str) -> bool {
        self.projection_deleted_snapshot()
            .iter()
            .any(|(deleted_key, _)| deleted_key == key)
    }

    pub(crate) fn projection_org_state(&self) -> Option<crate::projection::OrgReadModel> {
        let projection = self.lock_projection();
        match crate::projection::EvidenceProjectionReadPort::resolve(
            &projection,
            crate::projection::EvidenceProjectionQuery::OrgState,
        ) {
            crate::projection::EvidenceProjectionResponse::OrgState(org_state) => *org_state,
            _ => None,
        }
    }

    /// Test-only accessor for the materialised `projection_state`.
    #[doc(hidden)]
    pub fn projection_state_for_test(&self) -> Arc<Mutex<crate::projection::EvidenceProjection>> {
        Arc::clone(&self.projection_state)
    }
}

fn open_event_store(
    events_dir: &Path,
    backend: crate::config::runtime::PardosaBackend,
    nats: crate::config::runtime::NatsStoreConfig,
    handle: tokio::runtime::Handle,
) -> Result<EventStoreImpl, std::io::Error> {
    match backend {
        crate::config::runtime::PardosaBackend::Pgno => {
            std::fs::create_dir_all(events_dir)?;
            let path = events_dir.join("events.pgno");
            if path.exists() && path.metadata()?.len() > 0 {
                EventStoreImpl::open_pgno(&path).map_err(std::io::Error::other)
            } else {
                EventStoreImpl::create_pgno(&path).map_err(std::io::Error::other)
            }
        }
        crate::config::runtime::PardosaBackend::Nats => {
            open_or_create_jetstream(nats, handle).map_err(std::io::Error::other)
        }
    }
}

fn open_org_event_store(
    events_dir: &Path,
    backend: crate::config::runtime::PardosaBackend,
    nats: crate::config::runtime::NatsStoreConfig,
    handle: tokio::runtime::Handle,
) -> Result<OrgEventStoreImpl, std::io::Error> {
    match backend {
        crate::config::runtime::PardosaBackend::Pgno => {
            std::fs::create_dir_all(events_dir)?;
            let path = events_dir.join("org-events.pgno");
            if path.exists() && path.metadata()?.len() > 0 {
                OrgEventStoreImpl::open_pgno(&path).map_err(std::io::Error::other)
            } else {
                OrgEventStoreImpl::create_pgno(&path).map_err(std::io::Error::other)
            }
        }
        crate::config::runtime::PardosaBackend::Nats => {
            open_or_create_org_jetstream(nats, handle).map_err(std::io::Error::other)
        }
    }
}

fn open_or_create_jetstream(
    nats: crate::config::runtime::NatsStoreConfig,
    handle: tokio::runtime::Handle,
) -> Result<EventStoreImpl, crate::store::StoreError> {
    let open_nats = nats.clone();
    let open_handle = handle.clone();
    open_or_create_jetstream_with(
        move || EventStoreImpl::open_jetstream(jetstream_backend(open_nats, open_handle)),
        move || EventStoreImpl::create_jetstream(jetstream_backend(nats, handle)),
    )
}

fn open_or_create_org_jetstream(
    nats: crate::config::runtime::NatsStoreConfig,
    handle: tokio::runtime::Handle,
) -> Result<OrgEventStoreImpl, crate::store::StoreError> {
    let open_nats = nats.clone();
    let open_handle = handle.clone();
    open_or_create_org_jetstream_with(
        move || OrgEventStoreImpl::open_jetstream(jetstream_backend(open_nats, open_handle)),
        move || OrgEventStoreImpl::create_jetstream(jetstream_backend(nats, handle)),
    )
}

fn open_or_create_org_jetstream_with(
    open: impl FnOnce() -> Result<OrgEventStoreImpl, crate::store::StoreError>,
    create: impl FnOnce() -> Result<OrgEventStoreImpl, crate::store::StoreError>,
) -> Result<OrgEventStoreImpl, crate::store::StoreError> {
    match open() {
        Ok(store) => Ok(store),
        Err(e @ crate::store::StoreError::BackendInfrastructure { .. }) => Err(e),
        Err(_) => create(),
    }
}

fn open_or_create_jetstream_with(
    open: impl FnOnce() -> Result<EventStoreImpl, crate::store::StoreError>,
    create: impl FnOnce() -> Result<EventStoreImpl, crate::store::StoreError>,
) -> Result<EventStoreImpl, crate::store::StoreError> {
    match open() {
        Ok(store) => Ok(store),
        Err(e @ crate::store::StoreError::BackendInfrastructure { .. }) => Err(e),
        Err(_) => create(),
    }
}

fn jetstream_backend(
    nats: crate::config::runtime::NatsStoreConfig,
    handle: tokio::runtime::Handle,
) -> PardosaJetStreamBackend {
    let mut builder = JetStreamConfig::builder()
        .stream_name(nats.stream_name)
        .subject(nats.subject)
        .durable_consumer(nats.durable_consumer)
        .nats_url(nats.nats_url)
        .runtime_handle(RuntimeHandle::from_tokio(handle))
        .single_writer_fence_enabled(true);
    if let Some(path) = nats.credentials_path {
        builder = builder.credentials_path(path);
    }
    let cfg = builder.build().expect("validated NATS store config");
    let substrate = SubstrateJetStreamBackend::open(cfg);
    PardosaJetStreamBackend::open(substrate)
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
) -> Result<EventStoreImpl, std::io::Error> {
    tokio::task::spawn_blocking(move || open_event_store(&events_dir, backend, nats, handle))
        .await
        .map_err(std::io::Error::other)?
}

async fn open_org_event_store_blocking(
    events_dir: PathBuf,
    backend: crate::config::runtime::PardosaBackend,
    nats: crate::config::runtime::NatsStoreConfig,
    handle: tokio::runtime::Handle,
) -> Result<OrgEventStoreImpl, std::io::Error> {
    tokio::task::spawn_blocking(move || open_org_event_store(&events_dir, backend, nats, handle))
        .await
        .map_err(std::io::Error::other)?
}

fn projection_from_stores(
    store: &EventStoreImpl,
    org_store: &OrgEventStoreImpl,
) -> Result<crate::projection::EvidenceProjection, std::io::Error> {
    let projection = store
        .fold_events(
            crate::projection::EvidenceProjection::default(),
            |projection, detached, event| {
                fold_native_event(projection, detached, event);
            },
        )
        .map_err(std::io::Error::other)?;
    org_store
        .fold_events(projection, |projection, event| {
            fold_org_event(projection, event.clone());
        })
        .map_err(std::io::Error::other)
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
        } => {
            if detached {
                projection.repositories.remove(domain_key.as_str());
            } else if let Some(evidence) = evidence.as_ref() {
                projection.deleted.remove(domain_key.as_str());
                projection
                    .repositories
                    .insert(domain_key.as_str().to_string(), (*evidence).clone().into());
            }
        }
        NativeDomainEvent::RepositoryDeleted {
            domain_key,
            repo_name,
            detected_at,
        } => {
            projection.repositories.remove(domain_key.as_str());
            projection.deleted.insert(
                domain_key.as_str().to_string(),
                crate::projection::DeletedRepoRecord {
                    repo_name: repo_name.as_str().to_string(),
                    detected_at: event_timestamp_string(*detected_at),
                },
            );
        }
    }
}

fn fold_org_event(projection: &mut crate::projection::EvidenceProjection, event: OrgStateCaptured) {
    projection.apply_org_state(event.into());
}

fn native_store_persistence(error: crate::store::StoreError) -> PersistenceError {
    match error {
        crate::store::StoreError::ConcurrencyConflict { source } => {
            PersistenceError::FencedConflict { source }
        }
        crate::store::StoreError::TornWriteRecovery { source } => {
            PersistenceError::TornWriteRecovery { source }
        }
        other => {
            log_error_chain("gh_report_persistence_load_failed", &other);
            PersistenceError::LoadFailed {
                reason: other.to_string(),
            }
        }
    }
}

fn conversion_persistence(error: &EventConversionError) -> PersistenceError {
    log_error_chain("gh_report_event_conversion_failed", error);
    PersistenceError::LoadFailed {
        reason: error.to_string(),
    }
}

pub(crate) fn emit_nats_connect_diagnostics(nats_url: &str, credentials_path: Option<&Path>) {
    let creds_path = credentials_path
        .map(Path::display)
        .map(|path| path.to_string());
    let creds_path_display = creds_path.as_deref().unwrap_or("");
    let creds = credentials_path.map_or(CredsDiagnostic::missing_path(), creds_diagnostic);
    tracing::info!(
        nats_url = nats_url,
        creds_path = creds_path_display,
        creds_exists = creds.exists,
        creds_len = creds.len,
        creds_sha256_prefix = creds.sha256_prefix.as_deref().unwrap_or(""),
        "nats connect diagnostics"
    );
}

struct CredsDiagnostic {
    exists: bool,
    len: u64,
    sha256_prefix: Option<String>,
}

impl CredsDiagnostic {
    const fn missing_path() -> Self {
        Self {
            exists: false,
            len: 0,
            sha256_prefix: None,
        }
    }
}

fn creds_diagnostic(path: &Path) -> CredsDiagnostic {
    let metadata = path.metadata();
    let exists = metadata.is_ok();
    let len = metadata.as_ref().map_or(0, std::fs::Metadata::len);
    match std::fs::read(path) {
        Ok(bytes) => {
            let digest = Sha256::digest(&bytes);
            CredsDiagnostic {
                exists: true,
                len: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                sha256_prefix: Some(hex_prefix_8(&digest)),
            }
        }
        Err(_) => CredsDiagnostic {
            exists,
            len,
            sha256_prefix: None,
        },
    }
}

fn hex_prefix_8(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(8);
    for byte in bytes.iter().take(4) {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

pub(crate) fn log_error_chain(event: &'static str, error: &(dyn Error + 'static)) {
    let error_chain = error_chain_json(error);
    tracing::error!(
        diagnostic_event = event,
        error_chain = error_chain.as_str(),
        error = %error,
        "persistence error chain captured before flattening"
    );
}

fn error_chain_json(error: &(dyn Error + 'static)) -> String {
    let mut chain = String::from("[");
    let mut current = Some(error);
    let mut level = 0_u64;
    while let Some(error) = current {
        if level > 0 {
            chain.push(',');
        }
        let display = json_escape(&error.to_string());
        let debug = json_escape(&format!("{error:?}"));
        write!(
            chain,
            "{{\"level\":{level},\"display\":\"{display}\",\"debug\":\"{debug}\"}}"
        )
        .expect("string write succeeds");
        current = error.source();
        level += 1;
    }
    chain.push(']');
    chain
}

fn json_escape(value: &str) -> String {
    value
        .chars()
        .flat_map(char::escape_default)
        .collect::<String>()
}

fn non_empty<const MAX: usize>(
    field: &'static str,
    value: &str,
) -> Result<NonEmptyEventString<MAX>, PersistenceError> {
    NonEmptyEventString::try_new(value).map_err(|_| {
        conversion_persistence(&if value.is_empty() {
            EventConversionError::Empty { field }
        } else {
            EventConversionError::TooLong { field }
        })
    })
}

fn event_timestamp(field: &'static str, value: &str) -> Result<EventTimestamp, PersistenceError> {
    let parsed = crate::domain::time::parse_iso8601(value).ok_or_else(|| {
        conversion_persistence(&EventConversionError::BadTimestamp {
            field,
            value: value.to_string(),
        })
    })?;
    let nanos = u64::try_from(parsed.as_nanosecond()).map_err(|_| {
        conversion_persistence(&EventConversionError::BadTimestamp {
            field,
            value: value.to_string(),
        })
    })?;
    EventTimestamp::from_nanos(nanos).ok_or_else(|| {
        conversion_persistence(&EventConversionError::BadTimestamp {
            field,
            value: value.to_string(),
        })
    })
}

fn event_timestamp_string(timestamp: EventTimestamp) -> String {
    jiff::Timestamp::from_nanosecond(i128::from(timestamp.as_nanos()))
        .map_or_else(|_| String::new(), |value| value.to_string())
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

/// Per-construction unique tempdir plus native pardosa `.pgno` event store.
#[cfg(test)]
#[expect(
    clippy::unused_async,
    reason = "pardosa store facade is synchronous by PGN-0010:R5 / PGN-0015:R6; async fn preserves a uniform .await consumer seam across the sync-over-async backend boundary"
)]
async fn noop_event_store() -> Arc<EventStoreImpl> {
    let dir = tempfile::tempdir().expect("test tempdir");
    let path = dir.keep().join("events.pgno");
    Arc::new(EventStoreImpl::create_pgno(&path).expect("create test pardosa store"))
}

#[cfg(test)]
#[expect(
    clippy::unused_async,
    reason = "pardosa store facade is synchronous by PGN-0010:R5 / PGN-0015:R6; async fn preserves a uniform .await consumer seam across the sync-over-async backend boundary"
)]
async fn noop_org_event_store() -> Arc<OrgEventStoreImpl> {
    let dir = tempfile::tempdir().expect("test tempdir");
    let path = dir.keep().join("org-events.pgno");
    Arc::new(OrgEventStoreImpl::create_pgno(&path).expect("create test pardosa org store"))
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
        let projection_state =
            Arc::new(Mutex::new(crate::projection::EvidenceProjection::default()));
        Arc::new(Self {
            started_at: Timestamp::now(),
            owner_id: uuid::Uuid::now_v7(),
            current_run: ArcSwap::from_pointee(None),
            last_completed_run: ArcSwap::from_pointee(None),
            last_recovery: ArcSwap::from_pointee(None),
            work_queue: Arc::new(WorkQueue::new(crate::config::WORK_QUEUE_CAPACITY)),
            worker_pool_started: tokio::sync::OnceCell::new(),
            worker_pool_cancel: WorkerShutdownToken::new(),
            event_store,
            org_event_store,
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
    /// `<events_dir>` or opening/creating `events.pgno`; for `Nats`, M1
    /// returns an explicit startup error until the runtime handle wiring
    /// is supplied by a follow-up.
    ///
    pub async fn with_stores(
        events_dir: &Path,
        backend: crate::config::runtime::PardosaBackend,
        nats: crate::config::runtime::NatsStoreConfig,
    ) -> Result<Arc<Self>, std::io::Error> {
        let handle = tokio::runtime::Handle::current();
        let events_dir = events_dir.to_path_buf();
        if matches!(backend, crate::config::runtime::PardosaBackend::Nats) {
            emit_nats_connect_diagnostics(&nats.nats_url, nats.credentials_path.as_deref());
        }
        let event_store =
            open_event_store_blocking(events_dir.clone(), backend, nats.clone(), handle.clone())
                .await?;
        let org_event_store =
            open_org_event_store_blocking(events_dir, backend, nats.org_events(), handle).await?;
        let event_store = Arc::new(event_store);
        let org_event_store = Arc::new(org_event_store);
        let last_recovery = org_event_store
            .last_recovery()
            .map(|recovery| LastRecoveryStatus::from_outcome("orgs", recovery))
            .or_else(|| {
                event_store
                    .last_recovery()
                    .map(|recovery| LastRecoveryStatus::from_outcome("repositories", recovery))
            });
        let projection_state = Arc::new(Mutex::new(projection_from_stores(
            event_store.as_ref(),
            org_event_store.as_ref(),
        )?));
        Ok(Arc::new(Self {
            started_at: Timestamp::now(),
            owner_id: uuid::Uuid::now_v7(),
            current_run: ArcSwap::from_pointee(None),
            last_completed_run: ArcSwap::from_pointee(None),
            last_recovery: ArcSwap::from_pointee(last_recovery),
            work_queue: Arc::new(WorkQueue::new(crate::config::WORK_QUEUE_CAPACITY)),
            worker_pool_started: tokio::sync::OnceCell::new(),
            worker_pool_cancel: WorkerShutdownToken::new(),
            event_store,
            org_event_store,
            projection_state,
            webhook: WebhookState::from_environment(),
            github: GithubState::new(),
            evidence: EvidenceState::new(),
            sweep_lock: Arc::new(tokio::sync::Mutex::new(())),
        }))
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
        let projection =
            projection_from_stores(self.event_store.as_ref(), self.org_event_store.as_ref())?;
        let mut guard = self
            .projection_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = projection;
        Ok(())
    }

    fn fold_repository_event_into_projection(&self, detached: bool, event: &NativeDomainEvent) {
        let mut guard = self
            .projection_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        fold_native_event(&mut guard, detached, event);
    }

    fn fold_org_event_into_projection(&self, event: OrgStateCaptured) {
        let mut guard = self
            .projection_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        fold_org_event(&mut guard, event);
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
        let native_evidence = crate::event::RepositoryEvidence::try_from(evidence)
            .map_err(|e| conversion_persistence(&e))?;
        let event = repo_event(domain_key, repo_name, timestamp, Some(native_evidence))?;
        self.event_store
            .record(domain_key, event.clone())
            .map_err(native_store_persistence)?;
        self.fold_repository_event_into_projection(false, &event);
        Ok(())
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
        let event = repo_event(domain_key, repo_name, timestamp, None)?;
        self.event_store
            .detach(domain_key, event.clone())
            .map_err(native_store_persistence)?;
        self.fold_repository_event_into_projection(true, &event);
        Ok(())
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
        let event = deleted_repo_event(domain_key, repo_name, detected_at)?;
        self.event_store
            .record(domain_key, event.clone())
            .map_err(native_store_persistence)?;
        self.fold_repository_event_into_projection(false, &event);
        Ok(())
    }

    /// Record a live org snapshot in the native org store.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when native conversion or store append fails.
    pub fn record_org(
        &self,
        snapshot: crate::domain::evidence::OrgStateSnapshot,
    ) -> Result<(), PersistenceError> {
        let event = OrgStateCaptured::try_from(snapshot).map_err(|e| conversion_persistence(&e))?;
        let org_key = event.assessment_metadata.organization.as_str().to_string();
        self.org_event_store
            .record(&org_key, event.clone())
            .map_err(native_store_persistence)?;
        self.fold_org_event_into_projection(event);
        Ok(())
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
        let event_store = noop_event_store().await;
        let org_event_store = noop_org_event_store().await;
        let projection_state =
            Arc::new(Mutex::new(crate::projection::EvidenceProjection::default()));

        Arc::new(AppState {
            started_at: Timestamp::now(),
            owner_id: uuid::Uuid::now_v7(),
            current_run: ArcSwap::from_pointee(None),
            last_completed_run: ArcSwap::from_pointee(None),
            last_recovery: ArcSwap::from_pointee(None),
            work_queue: Arc::new(WorkQueue::new(crate::config::WORK_QUEUE_CAPACITY)),
            worker_pool_started: tokio::sync::OnceCell::new(),
            worker_pool_cancel: WorkerShutdownToken::new(),
            event_store,
            org_event_store,
            projection_state,
            webhook,
            github,
            evidence: EvidenceState::new(),
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
                    .github_client()
                    .expect("ensure_worker_pool called before github_client initialized")
                    .clone();

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

                let pool_handle = tokio::spawn(async move {
                    crate::app::worker_pool::run_worker_pool(
                        queue,
                        evaluator,
                        budget,
                        rate_limit,
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

impl AppState {
    /// Build the JSON payload for the `/api/v1/status` endpoint.
    ///
    /// Returns current and last completed run metadata plus uptime.
    /// Registered as an extra route in [`crate::server::status_router`],
    /// not as a built-in route of the generic server module.
    pub(crate) fn status_payload(&self) -> serde_json::Value {
        let current = self.current_run.load();
        let last = self.last_completed_run.load();
        let last_recovery = self.last_recovery.load();
        let uptime_duration = Timestamp::now().duration_since(self.started_at);
        let uptime = u64::try_from(uptime_duration.as_secs().max(0)).unwrap_or(0);
        serde_json::json!({
            "current_run": current.as_ref(),
            "last_completed_run": last.as_ref(),
            "last_recovery": last_recovery.as_ref(),
            "uptime_secs": uptime,
        })
    }
}

impl crate::infra::server::state::ServerState for AppState {
    fn html_cache(&self) -> &ArcSwap<Option<HashMap<String, CachedPage>>> {
        &self.evidence.html_cache
    }

    fn ws_broadcast(&self) -> &tokio::sync::broadcast::Sender<PageUpdateEvent> {
        &self.evidence.ws_broadcast
    }

    fn is_ready(&self) -> bool {
        self.event_store.backend_reachable()
            && self.org_event_store.backend_reachable()
            && (self.last_completed_run.load().is_some()
                || self.evidence.html_cache.load().is_some()
                || !self.lock_projection().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::runtime::{NatsStoreConfig, PardosaBackend};
    use crate::domain::cache::CachedRepoDetail;
    use crate::domain::evidence::Evidence;
    use crate::infra::server::state::ServerState;
    use std::io::Write;
    use std::sync::Arc;
    use tracing::field::{Field, Visit};
    use tracing_subscriber::layer::{Context, SubscriberExt};

    const SYNTHETIC_RECOVERY_RECORDS: u64 = 7;

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
            domain_key: NonEmptyEventString::try_new(domain_key).expect("domain key fits"),
            repo_name: NonEmptyEventString::try_new(repo_name).expect("repo name fits"),
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

    #[tokio::test(flavor = "multi_thread")]
    async fn nats_open_dead_port_returns_error_without_nested_runtime_panic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let events_dir = tmp.path().join("events");
        let nats = NatsStoreConfig::for_org("org", "nats://127.0.0.1:1").unwrap();

        let result = AppState::with_stores(&events_dir, PardosaBackend::Nats, nats).await;

        let error = match result {
            Ok(_) => panic!("dead-port Nats open must fail"),
            Err(error) => error.to_string(),
        };
        assert!(
            !error.contains("Cannot start a runtime from within a runtime"),
            "Nats open must return a typed connect error, not panic with nested-runtime failure: {error}"
        );
        assert!(
            error.contains("connect") || error.contains("Connection") || error.contains("refused"),
            "dead-port Nats open should reach connect and surface it as io::Error, got: {error}"
        );
    }

    fn synthetic_domain_event(i: u64) -> NativeDomainEvent {
        let domain_key = format!("domain-{i}");
        let repo_name = format!("repo-{i}");
        NativeDomainEvent::RepositoryStateCaptured {
            domain_key: NonEmptyEventString::try_new(&domain_key).expect("domain key fits"),
            repo_name: NonEmptyEventString::try_new(&repo_name).expect("repo name fits"),
            timestamp: EventTimestamp::from_nanos(i + 1).expect("timestamp fits"),
            evidence: None,
        }
    }

    fn synthesize_torn_footer_store(path: &Path, records: u64) -> u64 {
        {
            let store = EventStoreImpl::create_pgno(path).expect("create synthetic store");
            for i in 0..records {
                store
                    .record(&format!("domain-{i}"), synthetic_domain_event(i))
                    .expect("record synthetic event");
            }
        }
        {
            let mut store = pardosa::store::EventStore::<NativeDomainEvent>::open_with_backend(
                pardosa::store::PgnoBackend::open(path),
            )
            .expect("open backend-backed synthetic store");
            let _ = store.writer().sync().expect("sync synthetic manifest");
        }
        let mut os = path.as_os_str().to_os_string();
        os.push(".pgix");
        let manifest_path = PathBuf::from(os);
        let manifest = pardosa_file::manifest::parse_manifest(
            &std::fs::read(&manifest_path).expect("synthetic manifest bytes"),
        )
        .expect("synthetic manifest parses");
        assert_eq!(
            u64::try_from(manifest.records.len()).expect("manifest records fit"),
            records
        );
        {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(path)
                .expect("open synthetic pgno for torn tail");
            file.write_all(b"stale-footer-tail")
                .expect("append torn synthetic tail");
        }
        manifest.data_end
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn status_payload_contains_last_recovery_after_recovered_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        std::fs::create_dir_all(&events_dir).expect("events dir");
        let path = events_dir.join("events.pgno");
        let data_end = synthesize_torn_footer_store(&path, SYNTHETIC_RECOVERY_RECORDS);

        let state = AppState::with_stores(
            &events_dir,
            PardosaBackend::Pgno,
            NatsStoreConfig::for_org("org", crate::config::runtime::DEFAULT_NATS_URL).unwrap(),
        )
        .await
        .expect("with stores");
        let payload = state.status_payload();
        let last_recovery = payload
            .get("last_recovery")
            .and_then(serde_json::Value::as_object)
            .expect("last_recovery object");

        assert_eq!(
            last_recovery.get("store"),
            Some(&serde_json::json!("repositories"))
        );
        assert_eq!(
            last_recovery.get("manifest_message_count"),
            Some(&serde_json::json!(SYNTHETIC_RECOVERY_RECORDS))
        );
        assert_eq!(
            last_recovery.get("recovered_records"),
            Some(&serde_json::json!(SYNTHETIC_RECOVERY_RECORDS))
        );
        assert!(
            last_recovery
                .get("truncated_bytes")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|n| n > 0),
            "last_recovery must report discarded tail bytes: {last_recovery:?}"
        );
        assert_eq!(
            last_recovery.get("last_durable_offset"),
            Some(&serde_json::json!(data_end))
        );
    }

    #[test]
    fn jetstream_connect_open_error_surfaces_without_create_attempt() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let create_called = AtomicBool::new(false);
        let connect = crate::store::StoreError::BackendInfrastructure {
            op: pardosa::store::BackendOp::Sync,
            source: Box::new(pardosa::store::BackendError::Connect {
                op: pardosa::store::BackendOp::Sync,
                source: Box::new(std::io::Error::other("nats down")),
            }),
        };

        let result = open_or_create_jetstream_with(
            || Err(connect),
            || {
                create_called.store(true, Ordering::Release);
                Err(crate::store::StoreError::Infrastructure(
                    "create should not be attempted after connect failure".to_string(),
                ))
            },
        );

        assert!(
            matches!(
                result,
                Err(crate::store::StoreError::BackendInfrastructure { .. })
            ),
            "connect failure must surface through BackendInfrastructure"
        );
        assert!(
            !create_called.load(Ordering::Acquire),
            "connect failure on open must not fall through to create_jetstream"
        );
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
    fn native_store_persistence_preserves_fenced_conflict_variant() {
        let err = crate::store::StoreError::ConcurrencyConflict {
            source: Box::new(pardosa::store::PardosaError::ConcurrencyConflict {
                source: Box::new(std::io::Error::other("wrong last sequence")),
            }),
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
    fn native_store_persistence_preserves_torn_write_recovery_variant() {
        let err = crate::store::StoreError::TornWriteRecovery {
            source: Box::new(pardosa::store::PardosaError::CursorRead {
                source: Box::new(pardosa::store::replay::Error::File(
                    pardosa_file::FileError::TornWriteRecovery {
                        source: Box::new(
                            pardosa_file::manifest::RecoveryError::DataEndExceedsFile {
                                manifest_data_end: 12,
                                pgno_len: 8,
                            },
                        ),
                    },
                )),
            }),
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
        let err = crate::store::StoreError::BackendInfrastructure {
            op: pardosa::store::BackendOp::Sync,
            source: Box::new(pardosa::store::BackendError::Connect {
                op: pardosa::store::BackendOp::Sync,
                source: Box::new(std::io::Error::other("nats: authorization violation")),
            }),
        };

        let output = capture_events(|| {
            let persistence = native_store_persistence(err);

            let PersistenceError::LoadFailed { reason } = persistence else {
                panic!("backend infrastructure failure should flatten to LoadFailed");
            };
            assert!(
                reason.contains("authorization violation"),
                "flattened reason remains operator-visible: {reason}"
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
    fn nats_connect_diagnostics_log_creds_fingerprint_without_secret_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("user.creds");
        let secret = "super-secret-material-for-test";
        std::fs::write(&path, secret).expect("write creds");

        let output = capture_events(|| {
            emit_nats_connect_diagnostics("tls://connect.nats.mattilsynet.io:4222", Some(&path));
        });

        assert!(output.contains("nats_url=tls://connect.nats.mattilsynet.io:4222"));
        assert!(output.contains("creds_exists=true"));
        assert!(output.contains(&format!("creds_len={};", secret.len())));
        assert!(output.contains("creds_sha256_prefix="));
        assert!(
            !output.contains(secret),
            "diagnostics must not log credential bytes: {output}"
        );
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
                alert_summary: alert_summary.clone(),
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
            restarted.is_ready(),
            "a cold instance with repo or org events must be ready without a live GitHub run",
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
    async fn org_only_event_log_is_ready_after_coldstart() {
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
            restarted.is_ready(),
            "org-only event-log projection should be ready without repo events or GitHub API",
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
        state.evidence.html_cache.store(Arc::new(Some(pages)));
        assert!(state.is_ready(), "should be ready when html_cache is Some");
    }

    #[tokio::test]
    async fn is_ready_false_when_cache_populated_but_jetstream_connect_failed() {
        use crate::infra::server::state::ServerState;
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
        let (outcome_tx, outcome_rx) = tokio::sync::mpsc::channel(1);
        outcome_tx
            .send(crate::app::worker_pool::JobOutcome::Success {
                domain_key: "queued-repo".to_string(),
                result: crate::test_fixtures::all_passing_evidence("queued-repo"),
                source: crate::app::work_queue::JobSource::ScheduledBatch,
                duration: std::time::Duration::from_millis(1),
                correlation: cherry_pit_core::CorrelationContext::none(),
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
}
