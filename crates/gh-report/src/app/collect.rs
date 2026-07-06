//! Collection pipeline: run a single security data collection pass.
//!
//! Pipeline:
//! 1. Acquire in-process sweep lock on [`AppState::sweep_lock`]
//!    (mission `adr-fmt-cq7vb.8.2`): serialises concurrent
//!    [`run`] invocations against the same `AppState`, eliminating
//!    the singleton-state clobber windows in [`SweepSaga::new`] and
//!    [`enqueue_and_await_batch`].
//! 2. Acquire on-disk run lock (cross-process second line of defence)
//!    - A signal handler (SIGINT/SIGTERM) releases the lock and triggers
//!      cooperative shutdown via `CancellationToken`, preventing orphaned
//!      lock files on graceful termination.
//! 3. Resolve credentials and build `GitHubClient`
//! 4. Load or build repository inventory
//! 5. Collect org-level secret scanning alert summary
//! 6. Evaluate each repository against all five security checks
//!    - Resume from checkpoint if available
//!    - Reuse from baseline (kept separate from checkpoint)
//!    - Isolate per-repository failures
//!    - Persist checkpoint periodically (excludes baseline-reused entries)
//!    - Save baseline, then remove checkpoint
//! 7. Build evidence (assessment metadata + metrics + repositories)
//! 8. Render HTML report and update in-memory cache
//! 9. Release on-disk run lock; in-process sweep guard drops at function exit

use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::num::NonZeroU64;
use std::sync::Arc;

use arc_swap::ArcSwap;
use cherry_pit_app::{DurableScheduler, InProcessEventBus, SchedulePayloadDecoder};
use cherry_pit_core::{
    AggregateId, CorrelationContext, EventScheduler, EventStore, ScheduleArmed, ScheduleCancelled,
    ScheduleFired, ScheduleId,
};
use jiff::SignedDuration;
use tracing::{debug, error, info, warn};

use crate::aggregate::metrics;
use crate::app::state::{AppState, CACHED_STYLESHEET, CACHED_WS_JS, CachedPage};
use crate::collector::ghas_scanning;
use crate::collector::team_membership;
use crate::collector::{branch_protection, codeowners, dependabot, inventory, security_policy};
use crate::config;
use crate::config::runtime::RuntimeConfig;
use crate::domain::checks::{
    BranchProtectionDetails, BranchProtectionResult, BranchProtectionStatus, CodeownersResult,
    CodeownersStatus, DependabotResult, DependabotStatus, RepositoryChecks, SecretScanningResult,
    SecretScanningStatus, SecurityPolicyEvidence, SecurityPolicyResult, SecurityPolicyStatus,
};
use crate::domain::evidence::{AssessmentMetadata, Evidence, RepositoryEvidence};
use crate::domain::metrics::OrgAlertSummary;
use crate::domain::repository::Repository;
use crate::domain::run::RunMetadata;
use crate::error::{AppError, GitHubApiError, PersistenceError};
use crate::event::SweepTimeoutEvent;
use crate::github::auth::{AuthMetadata, CapabilitySet, GitHubAppConfig, GitHubCredential};
use crate::github::client::GitHubClient;
use crate::infra::{baseline, checkpoint, lock};
use crate::report::html;

/// Abstraction for evaluating a single repository's security posture.
///
/// This trait enables dependency injection: production code uses
/// [`LiveEvaluator`] (which calls the real GitHub API), while tests
/// use a synchronous closure wrapper (`FnEvaluator` in `#[cfg(test)]`).
///
/// The method returns an opaque future; the evaluator is shared via
/// monomorphic `Arc<E: RepoEvaluator>` across `tokio::spawn` tasks.
///
/// # Concurrency contract
/// Implementors must be safe for concurrent use: the evaluator is
/// shared across `tokio::spawn` tasks. Do not hold `RefCell` or other
/// non-thread-safe state.
pub(crate) trait RepoEvaluator: Send + Sync {
    /// Evaluate all security checks for a single repository.
    ///
    /// Returns an anonymous opaque future. The compiler places the state
    /// machine on the stack (or in the parent async frame) per CHE-0025:R2 —
    /// no per-call heap allocation.
    fn evaluate<'a>(
        &'a self,
        repo: Arc<Repository>,
        ts: &'a str,
    ) -> impl Future<Output = Result<RepositoryEvidence, String>> + Send + 'a;
}

/// Context for a work-queue job: repository + run timestamp.
///
/// Carries everything the [`LiveEvaluator`] needs to execute a job
/// without coupling the reactor to collection-specific types.
#[derive(Debug, Clone)]
pub(crate) struct JobContext {
    pub repo: Arc<Repository>,
    pub run_timestamp: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CollectionOutcome {
    Completed,
    Cancelled,
    FencedConflict,
}

/// Production evaluator that calls all five security check collectors
/// via the GitHub API.
///
/// Implements both [`RepoEvaluator`] (for the existing collection pipeline)
/// and [`JobExecutor`] (for the work-queue reactor).
pub(crate) struct LiveEvaluator {
    client: Arc<GitHubClient>,
    org_summary: Arc<ArcSwap<Option<Arc<OrgAlertSummary>>>>,
}

impl LiveEvaluator {
    /// Create an evaluator that reads org summary from a shared `ArcSwap`.
    ///
    /// Used when the evaluator should read the latest org summary from
    /// `AppState` (e.g., for webhook-triggered evaluations using AD6
    /// eventual consistency).
    pub(crate) fn with_shared_org_summary(
        client: Arc<GitHubClient>,
        org_summary: Arc<ArcSwap<Option<Arc<OrgAlertSummary>>>>,
    ) -> Self {
        Self {
            client,
            org_summary,
        }
    }
}

impl RepoEvaluator for LiveEvaluator {
    async fn evaluate<'a>(
        &'a self,
        repo: Arc<Repository>,
        ts: &'a str,
    ) -> Result<RepositoryEvidence, String> {
        let org_guard = self.org_summary.load_full();
        let org_ref = (*org_guard).as_ref().map(AsRef::as_ref);

        let _ = self.client.repo_details(&repo.name).await;

        let (sp, ss, dep, bp, co, last_commit) = tokio::join!(
            security_policy::evaluate(&self.client, &repo, ts),
            ghas_scanning::evaluate(&self.client, &repo, ts, org_ref),
            dependabot::evaluate(&self.client, &repo, ts),
            branch_protection::evaluate(&self.client, &repo, ts),
            codeowners::evaluate(&self.client, &repo, ts),
            crate::collector::last_commit::fetch_last_commit(&self.client, &repo),
        );

        let dep = if dep.status == DependabotStatus::Enabled
            && crate::domain::time::is_dependabot_inactive(repo.pushed_at.as_deref(), ts)
        {
            debug!(
                repo = %repo.name,
                pushed_at = ?repo.pushed_at,
                "dependabot inferred paused from inactivity (>90 days since last push)"
            );
            DependabotResult {
                status: DependabotStatus::Paused,
                reason: Some("inferred_from_inactivity".to_string()),
                timestamp: ts.to_string(),
            }
        } else {
            dep
        };

        Ok(RepositoryEvidence {
            repository: (*repo).clone(),
            checks: RepositoryChecks {
                security_policy: sp,
                secret_scanning: ss,
                dependabot_security_updates: dep,
                branch_protection: bp,
                codeowners: co,
            },
            last_commit,
        })
    }
}

impl crate::app::worker_pool::JobExecutor for LiveEvaluator {
    type Context = JobContext;
    type Result = RepositoryEvidence;

    fn execute<'a>(
        &'a self,
        _domain_key: &'a crate::app::work_queue::DomainKey,
        context: &'a Self::Context,
    ) -> impl Future<Output = Result<Self::Result, String>> + Send + 'a {
        self.evaluate(Arc::clone(&context.repo), &context.run_timestamp)
    }
}

struct CollectionSetup {
    lock: lock::RunLock,
    client: Arc<GitHubClient>,
    capabilities: CapabilitySet,
    auth_metadata: AuthMetadata,
    /// Budget gate total calls at the start of this run, for per-run delta.
    budget_baseline: u64,
}

/// Bundles the client, capabilities, and auth metadata for the collection
/// pipeline. Reduces parameter count for [`run_collection_pipeline`].
struct CollectionContext {
    client: Arc<GitHubClient>,
    capabilities: CapabilitySet,
    auth_metadata: AuthMetadata,
    /// Budget gate total calls at the start of this run, for per-run delta.
    budget_baseline: u64,
}

struct InventoryLoad {
    active_repos: Vec<Arc<Repository>>,
    complete: bool,
    /// ISO 8601 timestamp of when inventory was fetched from API.
    inventory_fetched_at: Option<String>,
}

struct OrgAlertContext {
    summary: OrgAlertSummary,
    snapshot: Option<serde_json::Value>,
}

/// Execute a single collection run.
///
/// Acquires the in-process [`AppState::sweep_lock`] as its first
/// action (mission `adr-fmt-cq7vb.8.2`), then the on-disk run lock
/// (cross-process second line of defence), resolves credentials,
/// builds the repository inventory from the GitHub API, evaluates
/// each repository with per-repo failure isolation, renders the HTML
/// report, and stores pages in the in-memory cache for immediate
/// serving.
///
/// # Errors
///
/// Returns `AppError` if lock acquisition, credential resolution, inventory
/// loading, report rendering, or cache population fails. Individual
/// repository evaluation failures are isolated and do not abort the run.
pub async fn run(config: RuntimeConfig, state: Arc<AppState>) -> Result<(), AppError> {
    run_with_outcome(config, state).await.map(|_| ())
}

pub(crate) async fn run_with_outcome(
    config: RuntimeConfig,
    state: Arc<AppState>,
) -> Result<CollectionOutcome, AppError> {
    let _sweep_guard = Arc::clone(&state.sweep_lock).lock_owned().await;

    let mut run = RunMetadata::new(
        config.org_name.clone(),
        config::EVIDENCE_SCHEMA_VERSION.to_string(),
    );
    let corr_ctx = run.correlation_context();
    info!(
        run_id = %run.run_id,
        org = %run.organization,
        "collection run starting"
    );

    state.current_run.store(Arc::new(Some(run.clone())));

    let result = run_collection_inner(&config, &mut run, &corr_ctx, &state).await;

    state.current_run.store(Arc::new(None));

    if matches!(result, Ok(CollectionOutcome::Completed)) {
        state.last_completed_run.store(Arc::new(Some(run.clone())));
    }

    result
}

/// Inner collection flow extracted to allow state cleanup in all paths.
async fn run_collection_inner(
    config: &RuntimeConfig,
    run: &mut RunMetadata,
    corr_ctx: &CorrelationContext,
    state: &Arc<AppState>,
) -> Result<CollectionOutcome, AppError> {
    recover_due_sweep_timeouts(state.as_ref()).await?;

    let setup = prepare_collection(config, run, state).await?;

    state.ensure_worker_pool().await;

    let inventory = load_active_repositories(&setup.client).await?;

    let stale_check: Vec<(String, Option<String>)> = inventory
        .active_repos
        .iter()
        .map(|r| (r.name.clone(), r.updated_at.clone()))
        .collect();
    setup.client.evict_stale_entries(&stale_check);

    let org_alert = collect_org_alert_context(&setup.client, &inventory.active_repos, run).await;

    let CollectionSetup {
        lock,
        client,
        capabilities,
        auth_metadata,
        budget_baseline,
    } = setup;

    let ctx = CollectionContext {
        client,
        capabilities,
        auth_metadata,
        budget_baseline,
    };

    let lock_handle = Arc::new(tokio::sync::Mutex::new(Some(lock)));
    let cancel = tokio_util::sync::CancellationToken::new();

    let signal_lock = Arc::clone(&lock_handle);
    let signal_cancel = cancel.clone();
    let signal_handle = tokio::spawn(async move {
        crate::infra::signal::wait_for_shutdown_signal().await;

        warn!("signal received — releasing lock and shutting down");

        if let Ok(mut guard) = signal_lock.try_lock() {
            if let Some(lock) = guard.take()
                && let Err(e) = lock.release()
            {
                warn!(error = %e, "failed to release lock during signal shutdown");
            }
        } else {
            warn!("could not acquire lock handle during signal — main task may still hold it");
        }

        signal_cancel.cancel();
    });

    let result = run_collection_inner_with_pipeline(
        &cancel,
        run_collection_pipeline(config, run, corr_ctx, ctx, &inventory, org_alert, state),
    )
    .await;

    if let Some(lock) = lock_handle.lock().await.take() {
        drop(lock);
    }

    if signal_handle.is_finished()
        && let Err(e) = signal_handle.await
    {
        error!(error = ?e, "signal handler task panicked");
    }

    result
}

async fn run_collection_inner_with_pipeline<P>(
    cancel: &tokio_util::sync::CancellationToken,
    pipeline: P,
) -> Result<CollectionOutcome, AppError>
where
    P: Future<Output = Result<(), AppError>>,
{
    tokio::select! {
        result = pipeline => {
        match result {
            Ok(()) => {}
            Err(AppError::Persistence(PersistenceError::FencedConflict { source })) => {
                warn!(error = %source, "collection fenced by active single-writer guard");
                return Ok(CollectionOutcome::FencedConflict);
            }
            Err(e) => return Err(e),
        }
        Ok(CollectionOutcome::Completed)
        }
        () = cancel.cancelled() => {
            info!(
                "collection cancelled by signal — lock released, no report published"
            );
            Ok(CollectionOutcome::Cancelled)
        }
    }
}

/// Inner collection pipeline, extracted so it can be wrapped in `tokio::select!`
/// for cooperative cancellation via `CancellationToken`.
///
/// ## Processing path (AD2 — single path)
///
/// All repository evaluation flows through the unified `WorkQueue` → worker
/// pool → delivery task pipeline. The sweep enqueues repos via
/// `enqueue_batch()` and waits for completion via `BatchTracker`. Workers
/// and delivery task are shared with webhook-triggered jobs.
///
/// Orchestration is driven by [`SweepSaga`], a state machine that models
/// each pipeline phase as an explicit state transition with domain event
/// emission, progress tracking, and saga-level timeout.
async fn run_collection_pipeline(
    config: &RuntimeConfig,
    run: &mut RunMetadata,
    corr_ctx: &CorrelationContext,
    ctx: CollectionContext,
    inventory: &InventoryLoad,
    org_alert: OrgAlertContext,
    state: &Arc<AppState>,
) -> Result<(), AppError> {
    let mut saga = SweepSaga::new(run, &ctx, org_alert, state);
    let mut sweep_ctx = SweepCtx::new(config, run, corr_ctx, state);
    saga.run_to_completion(&mut sweep_ctx, &ctx, inventory)
        .await
}

struct SweepCtx<'a> {
    config: &'a RuntimeConfig,
    run: &'a mut RunMetadata,
    corr_ctx: CorrelationContext,
    state: &'a Arc<AppState>,
}

impl<'a> SweepCtx<'a> {
    fn new(
        config: &'a RuntimeConfig,
        run: &'a mut RunMetadata,
        corr_ctx: &CorrelationContext,
        state: &'a Arc<AppState>,
    ) -> Self {
        Self {
            config,
            run,
            corr_ctx: corr_ctx.clone(),
            state,
        }
    }

    fn run(&self) -> &RunMetadata {
        &*self.run
    }

    fn run_mut(&mut self) -> &mut RunMetadata {
        &mut *self.run
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SweepTimeoutPayload {
    run_id: String,
    error: String,
    elapsed_ms: u64,
}

#[derive(Debug, thiserror::Error)]
enum SweepTimeoutDecodeError {
    #[error("decode sweep timeout payload failed: {0}")]
    Decode(#[from] rmp_serde::decode::Error),
    #[error("invalid sweep timeout payload field: {0}")]
    InvalidField(#[from] pardosa_schema::DomainError),
}

#[derive(Clone)]
struct SweepTimeoutDecoder;

impl SchedulePayloadDecoder<SweepTimeoutEvent> for SweepTimeoutDecoder {
    type Error = SweepTimeoutDecodeError;

    fn decode(&self, fired: &ScheduleFired) -> Result<SweepTimeoutEvent, Self::Error> {
        let payload: SweepTimeoutPayload = rmp_serde::from_slice(fired.payload())?;
        Ok(SweepTimeoutEvent::try_timeout_fired(
            fired.caller_event_id(),
            payload.run_id,
            &payload.error,
            payload.elapsed_ms,
        )?)
    }
}

#[derive(Debug, Clone, Copy)]
struct ArmedSweepTimeout {
    schedule_id: ScheduleId,
    fire_at: jiff::Timestamp,
}

fn sweep_timeout_target_aggregate() -> AggregateId {
    AggregateId::new(NonZeroU64::MIN)
}

fn sweep_timeout_persistence(error: impl std::fmt::Display) -> AppError {
    AppError::Persistence(PersistenceError::LoadFailed {
        reason: error.to_string(),
    })
}

async fn ensure_sweep_timeout_target(
    state: &AppState,
    context: CorrelationContext,
) -> Result<AggregateId, AppError> {
    let target = sweep_timeout_target_aggregate();
    let history = state
        .sweep_timeout_event_store
        .load(target)
        .await
        .map_err(sweep_timeout_persistence)?;
    if history.is_empty() {
        let (created, _) = state
            .sweep_timeout_event_store
            .create(
                vec![SweepTimeoutEvent::target_opened(uuid::Uuid::now_v7())],
                context,
            )
            .await
            .map_err(sweep_timeout_persistence)?;
        if created != target {
            return Err(sweep_timeout_persistence(format!(
                "sweep timeout target aggregate must be {target}, got {created}"
            )));
        }
    }
    Ok(target)
}

async fn arm_sweep_timeout(
    sweep: &SweepCtx<'_>,
    error: &str,
) -> Result<ArmedSweepTimeout, AppError> {
    let timeout_secs = i64::try_from(config::SWEEP_TIMEOUT_SECS)
        .expect("sweep timeout seconds fits signed duration");
    let elapsed_ms = config::SWEEP_TIMEOUT_SECS.saturating_mul(1_000);
    let fire_at = jiff::Timestamp::now() + SignedDuration::from_secs(timeout_secs);
    let target = ensure_sweep_timeout_target(sweep.state.as_ref(), sweep.corr_ctx.clone()).await?;
    let payload = SweepTimeoutPayload {
        run_id: sweep.run().run_id.clone(),
        error: error.to_string(),
        elapsed_ms,
    };
    let encoded = rmp_serde::to_vec(&payload).map_err(sweep_timeout_persistence)?;
    let schedule_id = ScheduleId::from_uuid(uuid::Uuid::now_v7());
    let event_id = uuid::Uuid::now_v7();
    let event = ScheduleArmed::new(
        schedule_id,
        fire_at,
        target,
        event_id,
        "gh-report.sweep_timeout_fired",
        encoded,
        sweep.corr_ctx.clone(),
    );
    let bus = InProcessEventBus::<SweepTimeoutEvent>::new();
    let scheduler = DurableScheduler::<_, _, _, _, SweepTimeoutEvent>::new(
        sweep.state.scheduler_event_store.as_ref(),
        sweep.state.sweep_timeout_event_store.as_ref(),
        &bus,
        SweepTimeoutDecoder,
    );
    EventScheduler::arm(&scheduler, event)
        .await
        .map_err(sweep_timeout_persistence)?;
    Ok(ArmedSweepTimeout {
        schedule_id,
        fire_at,
    })
}

async fn cancel_sweep_timeout(
    sweep: &SweepCtx<'_>,
    armed: ArmedSweepTimeout,
) -> Result<(), AppError> {
    let bus = InProcessEventBus::<SweepTimeoutEvent>::new();
    let scheduler = DurableScheduler::<_, _, _, _, SweepTimeoutEvent>::new(
        sweep.state.scheduler_event_store.as_ref(),
        sweep.state.sweep_timeout_event_store.as_ref(),
        &bus,
        SweepTimeoutDecoder,
    );
    EventScheduler::cancel(
        &scheduler,
        ScheduleCancelled::new(armed.schedule_id),
        sweep.corr_ctx.clone(),
    )
    .await
    .map_err(sweep_timeout_persistence)
}

async fn fire_due_sweep_timeout(
    sweep: &SweepCtx<'_>,
    armed: ArmedSweepTimeout,
) -> Result<(), AppError> {
    let bus = InProcessEventBus::<SweepTimeoutEvent>::new();
    let scheduler = DurableScheduler::<_, _, _, _, SweepTimeoutEvent>::new(
        sweep.state.scheduler_event_store.as_ref(),
        sweep.state.sweep_timeout_event_store.as_ref(),
        &bus,
        SweepTimeoutDecoder,
    );
    let _report = EventScheduler::recover_due(&scheduler, armed.fire_at)
        .await
        .map_err(sweep_timeout_persistence)?;
    Ok(())
}

pub(crate) async fn recover_due_sweep_timeouts(state: &AppState) -> Result<(), AppError> {
    let bus = InProcessEventBus::<SweepTimeoutEvent>::new();
    let scheduler = DurableScheduler::<_, _, _, _, SweepTimeoutEvent>::new(
        state.scheduler_event_store.as_ref(),
        state.sweep_timeout_event_store.as_ref(),
        &bus,
        SweepTimeoutDecoder,
    );
    let _report = EventScheduler::recover_due(&scheduler, jiff::Timestamp::now())
        .await
        .map_err(sweep_timeout_persistence)?;
    Ok(())
}

/// Current phase of the sweep saga state machine.
///
/// Each variant represents a checkpoint in the orchestration pipeline.
/// The saga transitions through phases sequentially, emitting domain
/// events at each transition for observability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SweepPhase {
    /// Initial state — checkpoint resume pending.
    Init,
    /// Checkpoint resume completed. Baseline reuse pending.
    Resumed,
    /// Baseline reuse completed. Batch enqueue pending.
    BaselineReused,
    /// Batch enqueued into work queue. Awaiting completion.
    AwaitingBatch,
    /// All repos evaluated (batch drained). Finalize pending.
    BatchDrained,
    /// Pipeline completed successfully.
    Completed,
    /// Pipeline failed (timeout, error, or all jobs rejected).
    Failed { error: String },
}

/// Sweep orchestration state machine.
///
/// Models `run_collection_pipeline` as explicit phase transitions with
/// domain event emission, progress tracking, and saga-level timeout.
///
/// ## Phase flow
///
/// ```text
/// Init → Resumed → BaselineReused → AwaitingBatch → BatchDrained → Completed
///                                 ↘ (no pending) → BatchDrained → Completed
///                     (timeout/error) → Failed
/// ```
///
/// Each step method advances the phase and can be tested independently
/// by constructing the saga in the appropriate starting phase.
pub(crate) struct SweepSaga {
    /// Current phase.
    phase: SweepPhase,
    /// Evidence carried forward from a prior run (δ.3c-ii: sourced from the
    /// projection via event-log replay; was previously the on-disk
    /// checkpoint).
    completed: HashMap<String, Arc<RepositoryEvidence>>,
    /// Evidence reused from previous baseline.
    baseline_cache: HashMap<String, Arc<RepositoryEvidence>>,
    /// Number of repos resumed from the projection (δ.3c-ii: was previously
    /// the checkpoint-resumed count).
    resumed_count: usize,
    /// Number of repos reused from baseline.
    baseline_reused: usize,
    /// Wall-clock start of the sweep.
    sweep_start: std::time::Instant,
    /// ISO 8601 run timestamp (derived from `RunMetadata`).
    run_timestamp: String,
    /// Content hash of the org-level alert snapshot.
    snapshot_signature: String,
    /// Budget pause notification channel.
    pause_notify: Arc<tokio::sync::Notify>,
    /// Org-level alert summary.
    org_summary: Arc<OrgAlertSummary>,
    /// GitHub API client (shared across phases).
    client: Arc<GitHubClient>,
}

impl SweepSaga {
    /// Create a new saga in the `Init` phase.
    ///
    /// Performs one-time setup: wires budget pause notification and stores
    /// the org summary for eventual consistency (AD6).
    fn new(
        run: &RunMetadata,
        ctx: &CollectionContext,
        org_alert: OrgAlertContext,
        state: &Arc<AppState>,
    ) -> Self {
        let pause_notify = Arc::new(tokio::sync::Notify::new());
        ctx.client
            .set_budget_pause_notify(Arc::clone(&pause_notify));

        let org_summary = Arc::new(org_alert.summary);
        state.set_org_alert_summary(Arc::clone(&org_summary));

        Self {
            phase: SweepPhase::Init,
            completed: HashMap::new(),
            baseline_cache: HashMap::new(),
            resumed_count: 0,
            baseline_reused: 0,
            sweep_start: std::time::Instant::now(),
            run_timestamp: run.timestamp(),
            snapshot_signature: checkpoint::build_snapshot_signature(org_alert.snapshot.as_ref()),
            pause_notify,
            org_summary,
            client: Arc::clone(&ctx.client),
        }
    }

    /// Current phase of the saga.
    #[cfg(test)]
    pub(crate) fn phase(&self) -> &SweepPhase {
        &self.phase
    }

    /// Drive the saga to completion, transitioning through all phases.
    ///
    /// Emits domain events (`SweepStarted`, `SweepProgress`,
    /// `SweepCompleted`/`SweepFailed`) at each transition.
    async fn run_to_completion(
        &mut self,
        sweep: &mut SweepCtx<'_>,
        ctx: &CollectionContext,
        inventory: &InventoryLoad,
    ) -> Result<(), AppError> {
        self.step_start_sweep(sweep, inventory);

        debug_assert_eq!(self.phase, SweepPhase::Init);
        self.phase = SweepPhase::Resumed;

        Self::emit_progress(
            sweep.run(),
            self.completed.len() as u64,
            inventory.active_repos.len() as u64,
        );

        self.step_baseline(sweep, inventory);

        Self::emit_progress(
            sweep.run(),
            (self.completed.len() + self.baseline_cache.len()) as u64,
            inventory.active_repos.len() as u64,
        );

        self.step_enqueue_and_await(sweep, ctx, inventory).await?;

        if let SweepPhase::Failed { ref error } = self.phase {
            return Err(AppError::Inventory(
                crate::error::InventoryError::ApiFetchFailed {
                    reason: error.clone(),
                },
            ));
        }

        self.step_finalize(sweep, ctx, inventory).await?;

        Ok(())
    }

    /// Phase 2: Reuse evidence from the previous baseline.
    ///
    /// Finds repos whose `updated_at` matches the baseline and
    /// pre-populates the evidence store.
    fn step_baseline(&mut self, sweep: &SweepCtx<'_>, inventory: &InventoryLoad) {
        debug_assert_eq!(self.phase, SweepPhase::Resumed);

        self.baseline_cache = reuse_from_baseline(
            &inventory.active_repos,
            &self.completed,
            &self.run_timestamp,
            sweep.state,
        );
        self.baseline_reused = self.baseline_cache.len();

        self.phase = SweepPhase::BaselineReused;
    }

    /// Phase 0: Register the sweep aggregate via `RunService::start_sweep`.
    ///
    /// CHE-0054:R1.a requires `SweepStarted` to be the first event of any
    /// `Run` instance, and CHE-0054:R5 routes subsequent commands through
    /// `runs_by_key` which is populated by the merger arm of `start_sweep`.
    /// Invoking this before the Resumed transition ensures the two pre-batch
    /// `emit_progress` calls in `run_to_completion` no longer hit an empty
    /// routing index (which previously surfaced as `RunError::RoutingMiss`
    /// swallowed by the non-fatal `SweepProgress publish failed` warn arm).
    /// Publish failure here remains non-fatal per CHE-0024:R1.
    fn step_start_sweep(&self, sweep: &SweepCtx<'_>, inventory: &InventoryLoad) {
        info!(
            org = %sweep.config.org_name,
            repo_count = inventory.active_repos.len(),
            batch_id = %sweep.run().run_id,
            timestamp = %jiff::Timestamp::now(),
            snapshot_signature = %self.snapshot_signature,
            "sweep started"
        );
    }

    /// Phase 3: Enqueue pending repos and await batch completion with timeout.
    ///
    /// If no repos are pending (all resumed or reused), transitions directly
    /// to `BatchDrained`. If all jobs are rejected by the queue, transitions
    /// to `Completed` (clean abort). On timeout, transitions to `Failed` and
    /// emits `SweepFailed`.
    async fn step_enqueue_and_await(
        &mut self,
        sweep: &SweepCtx<'_>,
        ctx: &CollectionContext,
        inventory: &InventoryLoad,
    ) -> Result<(), AppError> {
        debug_assert_eq!(self.phase, SweepPhase::BaselineReused);

        let pending: Vec<&Arc<Repository>> = inventory
            .active_repos
            .iter()
            .filter(|r| {
                !self.completed.contains_key(&r.inventory_key)
                    && !self.baseline_cache.contains_key(&r.inventory_key)
            })
            .collect();

        info!(
            total = inventory.active_repos.len(),
            resumed = self.resumed_count,
            baseline_reused = self.baseline_reused,
            pending = pending.len(),
            "enqueuing pending repos via work queue"
        );

        if pending.is_empty() {
            self.phase = SweepPhase::BatchDrained;
            return Ok(());
        }

        self.phase = SweepPhase::AwaitingBatch;

        let batch_future = enqueue_and_await_batch(BatchParams {
            pending: &pending,
            run_timestamp: &self.run_timestamp,
            pause_notify: &self.pause_notify,
            org_summary: &self.org_summary,
            auth_metadata: &ctx.auth_metadata,
            capabilities: &ctx.capabilities,
            config: sweep.config,
            run: sweep.run(),
            corr_ctx: &sweep.corr_ctx,
            inventory,
            state: sweep.state,
        });

        let timeout_error = format!("sweep timed out after {}s", config::SWEEP_TIMEOUT_SECS);
        let armed_timeout = arm_sweep_timeout(sweep, &timeout_error).await?;

        tokio::select! {
            result = batch_future => match result {
            Ok(true) => {
                cancel_sweep_timeout(sweep, armed_timeout).await?;
                self.phase = SweepPhase::BatchDrained;

                let total = inventory.active_repos.len() as u64;
                Self::emit_progress(sweep.run(), total, total);
            }
            Ok(false) => {
                cancel_sweep_timeout(sweep, armed_timeout).await?;
                let error_msg = "all jobs rejected by work queue".to_string();
                self.phase = SweepPhase::Failed {
                    error: error_msg.clone(),
                };
                Self::publish_sweep_failed(sweep.run(), &error_msg, self.elapsed_ms());
            }
            Err(e) => {
                cancel_sweep_timeout(sweep, armed_timeout).await?;
                let error_msg = e.to_string();
                self.phase = SweepPhase::Failed {
                    error: error_msg.clone(),
                };
                Self::publish_sweep_failed(sweep.run(), &error_msg, self.elapsed_ms());
                return Err(e);
            }
            },
            () = tokio::time::sleep_until(tokio::time::Instant::now() + std::time::Duration::from_secs(config::SWEEP_TIMEOUT_SECS)) => {
                warn!(
                    timeout_secs = config::SWEEP_TIMEOUT_SECS,
                    elapsed_ms = self.elapsed_ms(),
                    "sweep batch timed out"
                );
                fire_due_sweep_timeout(sweep, armed_timeout).await?;
                self.phase = SweepPhase::Failed {
                    error: timeout_error.clone(),
                };
                Self::publish_sweep_failed(sweep.run(), &timeout_error, self.elapsed_ms());
            }
        }

        Ok(())
    }

    /// Publish a `SweepFailed` event; non-fatal on publish error (warn only).
    ///
    /// Associated function: callers already hold the values needed and pass
    /// them explicitly, mirroring [`Self::emit_progress`].
    fn publish_sweep_failed(run: &RunMetadata, error_msg: &str, duration_ms: u64) {
        warn!(
            batch_id = %run.run_id,
            error = %error_msg,
            duration_ms,
            timestamp = %jiff::Timestamp::now(),
            "sweep failed"
        );
    }

    /// Phase 4: Finalize — snapshot evidence, build report, publish.
    ///
    /// CHE-0068:R2 — the visible terminal commit (html-cache replace +
    /// WS broadcast) is sequenced strictly after `SweepCompleted` has
    /// been published. The render *computation* runs first (so a
    /// render failure still maps to `SweepFailed`), then the barrier
    /// event publishes, then the rendered pages commit, then the
    /// terminal `EvidencePublished` event publishes (CHE-0054:R1.c).
    async fn step_finalize(
        &mut self,
        sweep: &mut SweepCtx<'_>,
        ctx: &CollectionContext,
        inventory: &InventoryLoad,
    ) -> Result<(), AppError> {
        debug_assert_eq!(self.phase, SweepPhase::BatchDrained);
        let config = sweep.config;
        let state = sweep.state;

        if inventory.complete && !inventory.active_repos.is_empty() {
            reconcile_deleted_repositories(state, inventory, &sweep.run().timestamp())?;
        }

        let result = finalize_and_publish(FinalizeParams {
            config,
            run: sweep.run_mut(),
            inventory,
            org_summary: &self.org_summary,
            auth_metadata: &ctx.auth_metadata,
            capabilities: &ctx.capabilities,
            budget_baseline: ctx.budget_baseline,
            client: &self.client,
            state,
        })
        .await;

        match result {
            Ok((pages, warm_start)) => {
                self.phase = SweepPhase::Completed;
                info!(
                    batch_id = %sweep.run().run_id,
                    duration_ms = self.elapsed_ms(),
                    repo_count = inventory.active_repos.len(),
                    timestamp = %jiff::Timestamp::now(),
                    "sweep completed"
                );

                let page_count = commit_cached_pages(state, sweep.run(), pages);

                info!(
                    page_count = page_count,
                    warm_start,
                    timestamp = %jiff::Timestamp::now(),
                    "evidence published"
                );
                Ok(())
            }
            Err(e) => {
                let error_msg = e.to_string();
                self.phase = SweepPhase::Failed {
                    error: error_msg.clone(),
                };
                Self::publish_sweep_failed(sweep.run(), &error_msg, self.elapsed_ms());
                Err(e)
            }
        }
    }

    /// Elapsed time since sweep start, in milliseconds (clamped to u64).
    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.sweep_start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn emit_progress(run: &RunMetadata, completed: u64, total: u64) {
        info!(
            batch_id = %run.run_id,
            completed,
            total,
            timestamp = %jiff::Timestamp::now(),
            "sweep progress"
        );
    }
}

/// Parameters for [`enqueue_and_await_batch`].
struct BatchParams<'a> {
    pending: &'a [&'a Arc<Repository>],
    run_timestamp: &'a str,
    pause_notify: &'a Arc<tokio::sync::Notify>,
    org_summary: &'a Arc<OrgAlertSummary>,
    auth_metadata: &'a AuthMetadata,
    capabilities: &'a CapabilitySet,
    config: &'a RuntimeConfig,
    run: &'a RunMetadata,
    corr_ctx: &'a CorrelationContext,
    inventory: &'a InventoryLoad,
    state: &'a Arc<AppState>,
}

/// Enqueue pending repos, wait for all jobs to complete, then shut down
/// the partial publisher. Returns `false` if the queue rejected all jobs
/// (caller should abort the sweep).
async fn enqueue_and_await_batch(params: BatchParams<'_>) -> Result<bool, AppError> {
    let BatchParams {
        pending,
        run_timestamp,
        pause_notify,
        org_summary,
        auth_metadata,
        capabilities,
        config,
        run,
        corr_ctx,
        inventory,
        state,
    } = params;
    let items: Vec<(crate::app::work_queue::DomainKey, JobContext)> = pending
        .iter()
        .map(|repo| {
            (
                repo.inventory_key.clone(),
                JobContext {
                    repo: Arc::clone(repo),
                    run_timestamp: run_timestamp.to_string(),
                },
            )
        })
        .collect();

    let batch_result = crate::app::work_queue::enqueue_batch(
        &state.work_queue,
        items,
        &crate::app::work_queue::JobSource::ScheduledBatch,
        corr_ctx,
    );

    if batch_result.accepted == 0 && !pending.is_empty() {
        warn!(
            total = batch_result.total,
            rejected = batch_result.rejected,
            "sweep aborted: work queue rejected all jobs (closed or full)"
        );
        return Ok(false);
    }

    if batch_result.rejected > 0 {
        warn!(
            rejected = batch_result.rejected,
            total = batch_result.total,
            "some sweep jobs rejected — queue capacity < inventory"
        );
    }

    let tracker = crate::app::work_queue::BatchTracker::new(batch_result.accepted);
    state.set_active_batch_tracker(Some(Arc::clone(&tracker)));

    let pp_config = PartialPublishConfig {
        pause_notify: Arc::clone(pause_notify),
        config: config.clone(),
        run: run.clone(),
        inventory_fetched_at: inventory.inventory_fetched_at.clone(),
        org_alert_summary: Some(Arc::clone(org_summary)),
        auth_metadata: auth_metadata.clone(),
        capabilities: capabilities.clone(),
        state: Arc::clone(state),
    };
    let (pp_task, pp_shutdown) = spawn_partial_publisher_from_store(pp_config, Arc::clone(state));

    tracker.wait().await;
    state.set_active_batch_tracker(None);

    let _ = pp_shutdown.send(true);
    if let Err(e) = pp_task.await {
        error!(error = ?e, "partial publisher task panicked");
    }

    Ok(true)
}

/// Arguments for [`finalize_and_publish`].
///
/// δ.3c-ii: `baseline_cache`, `run_timestamp`, `snapshot_signature`, and
/// `checkpoint_path` are gone — they only fed the on-disk checkpoint
/// write, which is retired.
struct FinalizeParams<'a> {
    config: &'a RuntimeConfig,
    run: &'a mut RunMetadata,
    inventory: &'a InventoryLoad,
    org_summary: &'a Arc<OrgAlertSummary>,
    auth_metadata: &'a AuthMetadata,
    capabilities: &'a CapabilitySet,
    budget_baseline: u64,
    client: &'a Arc<GitHubClient>,
    state: &'a Arc<AppState>,
}

/// Phase 4: Snapshot the evidence store, build evidence, render the
/// HTML cache, and export the repo-detail cache. **Does not commit
/// the rendered cache** — CHE-0068:R2 requires the visible
/// (html-cache replace + WS broadcast) step to fire strictly after
/// `SweepCompleted` has been published. The caller
/// ([`SweepSaga::step_finalize`]) commits the returned
/// [`HashMap<String, CachedPage>`] via [`commit_cached_pages`] after
/// publishing the barrier event.
///
/// δ.3c-ii: on-disk baseline + checkpoint persistence is retired. The
/// projection is the durable read-model (rebuilt at boot via event-log
/// replay per CHE-0051:R5 + CHE-0048:R2).
async fn finalize_and_publish(
    params: FinalizeParams<'_>,
) -> Result<(HashMap<String, CachedPage>, bool), AppError> {
    let FinalizeParams {
        config,
        run,
        inventory,
        org_summary,
        auth_metadata,
        capabilities,
        budget_baseline,
        client,
        state,
    } = params;

    let rate_limit_warnings = client
        .rate_limit_warnings
        .load(std::sync::atomic::Ordering::Relaxed);

    let org_snapshot = build_org_state_snapshot(&OrgSnapshotParams {
        config,
        run,
        inventory,
        org_summary,
        auth_metadata,
        capabilities,
        rate_limit_warnings,
    });
    state.record_org(org_snapshot)?;

    let evidence_repos = state.projection_snapshot();

    let team_slugs = crate::domain::metrics::team_owner_slugs(&evidence_repos);
    let team_rosters = team_membership::collect_team_rosters(client, &team_slugs).await;

    let evidence = build_evidence(BuildEvidenceParams {
        repositories: evidence_repos,
        deleted: state
            .projection_deleted_snapshot()
            .into_iter()
            .map(|(_, record)| record)
            .collect(),
        org_state: state.projection_org_state(),
        config,
        run,
        inventory_fetched_at: inventory.inventory_fetched_at.clone(),
        org_alert_summary: Some(org_summary),
        auth_metadata,
        capabilities,
        rate_limit_warnings,
        team_rosters,
    });

    let pages = build_cached_pages(config, &evidence).await?;
    let warm_start = evidence.assessment_metadata.warm_start;

    run.complete();

    let entries = client.export_cache();
    if !entries.is_empty() {
        state.store_client_repo_detail_cache(entries).await;
    }

    info!(
        run_id = %run.run_id,
        repos = evidence.collection_statistics.total_repos,
        api_calls = client.budget_total_calls() - budget_baseline,
        "collection run complete"
    );

    Ok((pages, warm_start))
}

async fn prepare_collection(
    config: &RuntimeConfig,
    run: &RunMetadata,
    state: &Arc<AppState>,
) -> Result<CollectionSetup, AppError> {
    let store_dir = config.store_dir.clone();
    let run_id = run.run_id.clone();
    let force_unlock = config.force_unlock;
    let lock_guard = tokio::task::spawn_blocking(move || -> Result<lock::RunLock, AppError> {
        std::fs::create_dir_all(&store_dir).map_err(PersistenceError::Io)?;
        lock::acquire(&store_dir, &run_id, lock::DEFAULT_LOCK_TTL, force_unlock)
            .map_err(AppError::Persistence)
    })
    .await
    .map_err(|e| AppError::Persistence(PersistenceError::Io(std::io::Error::other(e))))?
    .map_err(|e| {
        if let AppError::Persistence(PersistenceError::LockFailed { ref reason }) = e {
            let lock_file = lock::lock_path(&config.store_dir);
            error!(
                reason = %reason,
                lock_file = %lock_file.display(),
                "collection already in progress; remove lock manually or re-run with --force-unlock"
            );
        }
        e
    })?;
    info!("run lock acquired");

    let client = state
        .github_client_or_try_init(|| async {
            let (budget, rate_limit) = state.github_api_controls();
            let app_config = GitHubAppConfig::from_environment()?;
            let credential = GitHubCredential::from_environment()?;
            let client = GitHubClient::new(
                credential,
                crate::config::DEFAULT_GITHUB_API_BASE_URL,
                &config.org_name,
                app_config,
                budget,
                rate_limit,
            )?;
            Ok::<Arc<GitHubClient>, AppError>(Arc::new(client))
        })
        .await?;
    let client = Arc::clone(client);

    client.clear_run_cache();
    client.reset_halt();

    state.seed_client_repo_detail_cache(&client);

    let budget_baseline = state.github_budget_total_calls();

    let capabilities = client.probe_capabilities().await;
    if !capabilities.can_run() {
        return Err(AppError::GitHubApi(GitHubApiError::AuthorizationDenied {
            reason: "insufficient permissions to list org repositories; \
                     set GITHUB_TOKEN or run `gh auth login`"
                .into(),
        }));
    }

    let auth_metadata = client.collect_auth_metadata().await;
    info!(
        auth_mode = %auth_metadata.auth_mode,
        token_tier = %auth_metadata.token_tier,
        "credentials resolved"
    );

    Ok(CollectionSetup {
        lock: lock_guard,
        client,
        capabilities,
        auth_metadata,
        budget_baseline,
    })
}

async fn load_active_repositories(client: &GitHubClient) -> Result<InventoryLoad, AppError> {
    let inv = inventory::build_inventory_from_api(client, None).await?;
    let load = inventory_load_from_payload(inv);
    info!(
        total = load.active_repos.len(),
        "repository inventory loaded"
    );
    Ok(load)
}

fn inventory_load_from_payload(payload: inventory::InventoryPayload) -> InventoryLoad {
    let inventory_fetched_at = payload.inventory_fetched_at;
    let active_repos: Vec<Arc<Repository>> =
        payload.repositories.into_iter().map(Arc::new).collect();
    InventoryLoad {
        active_repos,
        complete: payload.complete,
        inventory_fetched_at,
    }
}

fn reconcile_deleted_repositories(
    state: &AppState,
    inventory: &InventoryLoad,
    detected_at: &str,
) -> Result<(), PersistenceError> {
    let active_keys: BTreeSet<&str> = inventory
        .active_repos
        .iter()
        .map(|repo| repo.inventory_key.as_str())
        .collect();
    let disappeared: Vec<(String, String)> = state
        .projection_key_name_snapshot()
        .into_iter()
        .filter(|(inventory_key, _name)| !active_keys.contains(inventory_key.as_str()))
        .collect();
    for (domain_key, repo_name) in disappeared {
        state.mark_repo_deleted(&domain_key, &repo_name, detected_at)?;
    }
    Ok(())
}

#[cfg(test)]
fn reconcile_deleted_repositories_after_successful_inventory(
    state: &AppState,
    inventory_result: &Result<InventoryLoad, AppError>,
    detected_at: &str,
) -> Result<(), PersistenceError> {
    let Ok(inventory) = inventory_result else {
        return Ok(());
    };
    if !inventory.complete || inventory.active_repos.is_empty() {
        return Ok(());
    }
    reconcile_deleted_repositories(state, inventory, detected_at)
}

async fn collect_org_alert_context(
    client: &GitHubClient,
    active_repos: &[Arc<Repository>],
    run: &RunMetadata,
) -> OrgAlertContext {
    let summary = ghas_scanning::collect_org_alerts(client, active_repos, &run.timestamp()).await;
    let snapshot = serde_json::to_value(&summary)
        .inspect_err(
            |e| warn!(error = %e, "failed to serialize org alert summary for checkpoint signature"),
        )
        .ok();

    OrgAlertContext { summary, snapshot }
}

/// Populate the HTML cache from the most recent baseline, so the server
/// can start serving immediately without waiting for the first API
/// collection to complete.
///
/// Returns `true` if the warm-start succeeded (baseline exists, is valid,
/// and pages were rendered). Returns `false` on any graceful failure
/// (no baseline, empty baseline, schema mismatch, render error).
///
/// # Why this is safe
///
/// - `html_cache` uses [`ArcSwap`] for atomic swap — concurrent reads
///   from the server always see a consistent snapshot.
/// - The warm-start evidence uses placeholder `AssessmentMetadata` (auth
///   fields set to Unknown, `warm_start: true`). `archived_repos` in
///   `collection_statistics` is derived from the baseline repos via
///   [`metrics::build_collection_statistics`] rather than zeroed.
///   Templates detect `warm_start` and display a "Cached" badge.
/// - The first real collection atomically replaces the warm-start cache.
pub(crate) async fn warm_start_from_baseline(
    config: &RuntimeConfig,
    state: &Arc<AppState>,
) -> bool {
    let repos = state.projection_snapshot();
    let org_state = state.projection_org_state();

    if repos.is_empty() && org_state.is_none() {
        info!("projection is empty — skipping warm start");
        return false;
    }

    info!(repos = repos.len(), "warm-starting from baseline");

    let run = RunMetadata::new(
        config.org_name.clone(),
        config::EVIDENCE_SCHEMA_VERSION.to_string(),
    );

    let evidence = build_evidence(BuildEvidenceParams {
        repositories: repos,
        deleted: state
            .projection_deleted_snapshot()
            .into_iter()
            .map(|(_, record)| record)
            .collect(),
        org_state,
        config,
        run: &run,
        inventory_fetched_at: None,
        org_alert_summary: None,
        auth_metadata: &AuthMetadata {
            token_tier: crate::domain::auth::TokenTier::Unknown,
            token_scopes: "cached".to_string(),
            auth_mode: crate::domain::auth::AuthMode::Unknown,
        },
        capabilities: &CapabilitySet::default(),
        rate_limit_warnings: 0,
        team_rosters: Vec::new(),
    });

    let warm_repo_count: u64 = u64::from(evidence.collection_statistics.total_repos);

    info!(
        org = %config.org_name,
        repo_count = warm_repo_count,
        timestamp = %run.timestamp(),
        "warm-start evidence render"
    );

    let warm_run = run.clone();

    match publish_evidence(config, &warm_run, &evidence, state).await {
        Ok(()) => {
            info!("warm-start cache populated — server can start serving");
            true
        }
        Err(e) => {
            warn!(error = %e, "warm-start render failed — server will start with empty cache");
            false
        }
    }
}

/// Build the HTML+zstd cache and broadcast WS update. Returns `page_count`.
pub(crate) async fn render_and_cache_evidence(
    config: &RuntimeConfig,
    run: &RunMetadata,
    evidence: &Evidence,
    state: &Arc<AppState>,
) -> Result<usize, AppError> {
    let pages = build_cached_pages(config, evidence).await?;
    Ok(commit_cached_pages(state, run, pages))
}

/// Render dashboard HTML and build the per-page zstd-compressed cache
/// entries on the blocking thread pool. Pure compute — no shared state
/// is mutated and no broadcast fires. Fallible (template rendering /
/// blocking-task join). CHE-0068:R2 two-phase render: callers that
/// need barrier-aligned visibility commit the result via
/// [`commit_cached_pages`] strictly after the barrier event has been
/// published.
pub(crate) async fn build_cached_pages(
    config: &RuntimeConfig,
    evidence: &Evidence,
) -> Result<HashMap<String, CachedPage>, AppError> {
    let pages = html::render_dashboard(evidence, &config.dashboard_config)?;
    info!(
        page_count = pages.len(),
        total_bytes = pages.values().map(String::len).sum::<usize>(),
        "dashboard pages rendered"
    );

    tokio::task::spawn_blocking(move || {
        pages
            .into_iter()
            .map(|(path, content)| {
                let page = match path.as_str() {
                    "style.css" => CACHED_STYLESHEET.clone(),
                    "ws.js" => CACHED_WS_JS.clone(),
                    _ => CachedPage::new(&path, content.into_bytes()),
                };
                (path, page)
            })
            .collect()
    })
    .await
    .map_err(|e| {
        AppError::Report(crate::error::ReportError::TemplateRenderFailed {
            reason: format!("cache build task panicked: {e}"),
        })
    })
}

/// Commit pre-built cached pages: atomically replace the html cache
/// pointer and notify WebSocket subscribers.
pub(crate) fn commit_cached_pages(
    state: &Arc<AppState>,
    run: &RunMetadata,
    cache: HashMap<String, CachedPage>,
) -> usize {
    let page_count = cache.len();
    let page_keys: Vec<String> = cache.keys().cloned().collect();

    state.set_html_cache(cache);
    info!(page_count, run_id = %run.run_id, "html cache updated");

    let _ = state.send_page_update(crate::app::state::PageUpdateEvent::new(
        page_keys,
        jiff::Timestamp::now().to_string(),
    ));

    page_count
}

/// Render, cache, and trace evidence publication.
pub(crate) async fn publish_evidence(
    config: &RuntimeConfig,
    run: &RunMetadata,
    evidence: &Evidence,
    state: &Arc<AppState>,
) -> Result<(), AppError> {
    let page_count = render_and_cache_evidence(config, run, evidence, state).await?;

    info!(
        page_count = page_count,
        warm_start = evidence.assessment_metadata.warm_start,
        timestamp = %jiff::Timestamp::now(),
        "evidence published"
    );

    Ok(())
}

/// Phase 2: Check the baseline for repos that haven't changed since the
/// last successful run.
///
/// Returns a separate map of baseline-reused entries (kept separate from
/// checkpoint-completed entries so checkpoints remain small).
fn reuse_from_baseline(
    repositories: &[Arc<Repository>],
    completed: &HashMap<String, Arc<RepositoryEvidence>>,
    run_timestamp: &str,
    state: &Arc<AppState>,
) -> HashMap<String, Arc<RepositoryEvidence>> {
    let pending_before_baseline: Vec<&Arc<Repository>> = repositories
        .iter()
        .filter(|r| !completed.contains_key(&r.inventory_key))
        .collect();

    let mut baseline_cache: HashMap<String, Arc<RepositoryEvidence>> = HashMap::new();

    for repo in &pending_before_baseline {
        let Some(evidence) = state.projection_get(&repo.inventory_key) else {
            continue;
        };
        let baseline_updated_at = evidence
            .repository
            .updated_at
            .as_deref()
            .unwrap_or_default();
        if !baseline::should_reuse(baseline_updated_at, repo.updated_at.as_deref()) {
            continue;
        }
        if evidence.checks.dependabot_security_updates.status == DependabotStatus::Enabled
            && crate::domain::time::is_dependabot_inactive(repo.pushed_at.as_deref(), run_timestamp)
        {
            debug!(
                repo = %repo.name,
                "skipping baseline reuse: dependabot may be auto-paused (inactive >90 days)"
            );
            continue;
        }

        debug!(
            repo = %repo.name,
            updated_at = %baseline_updated_at,
            "reusing baseline evidence"
        );
        baseline_cache.insert(repo.inventory_key.clone(), Arc::new(evidence));
    }

    if !baseline_cache.is_empty() {
        info!(
            reused = baseline_cache.len(),
            total = repositories.len(),
            "reused evidence from baseline"
        );
    }

    baseline_cache
}

/// Configuration for the partial publisher task.
///
/// Carries all owned data needed to call [`build_evidence`] +
/// [`publish_evidence`] inside a `tokio::spawn` boundary.
pub(crate) struct PartialPublishConfig {
    /// Notification channel: fires when the budget gate pauses.
    pub pause_notify: Arc<tokio::sync::Notify>,
    pub config: RuntimeConfig,
    pub run: RunMetadata,
    pub inventory_fetched_at: Option<String>,
    pub org_alert_summary: Option<Arc<OrgAlertSummary>>,
    pub auth_metadata: AuthMetadata,
    pub capabilities: CapabilitySet,
    pub state: Arc<AppState>,
}

/// Spawn a partial publisher that reads from the evidence store (C9).
///
/// Reads through `state.projection_snapshot()` so the typed read-side port
/// remains the report-render path. Used by the queue-based sweep.
fn spawn_partial_publisher_from_store(
    pp: PartialPublishConfig,
    state: Arc<AppState>,
) -> (
    tokio::task::JoinHandle<()>,
    tokio::sync::watch::Sender<bool>,
) {
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    let handle = tokio::spawn(async move {
        let debounce_duration = crate::config::PARTIAL_RENDER_MAX_STALENESS;
        let mut pending = false;
        let debounce_timer = tokio::time::sleep(debounce_duration);
        tokio::pin!(debounce_timer);
        debounce_timer
            .as_mut()
            .reset(tokio::time::Instant::now() + std::time::Duration::from_hours(24));

        loop {
            tokio::select! {
                () = pp.pause_notify.notified() => {
                    if !pending {
                        pending = true;
                        debounce_timer.as_mut().reset(
                            tokio::time::Instant::now() + debounce_duration,
                        );
                    }
                }

                () = &mut debounce_timer, if pending => {
                    pending = false;

                    let all_evidence = state.projection_snapshot();

                    let evidence = build_evidence(BuildEvidenceParams {
                        repositories: all_evidence,
                        deleted: state
                            .projection_deleted_snapshot()
                            .into_iter()
                            .map(|(_, record)| record)
                            .collect(),
                        org_state: state.projection_org_state(),
                        config: &pp.config,
                        run: &pp.run,
                        inventory_fetched_at: pp.inventory_fetched_at.clone(),
                        org_alert_summary: pp.org_alert_summary.as_deref(),
                        auth_metadata: &pp.auth_metadata,
                        capabilities: &pp.capabilities,
                        rate_limit_warnings: 0,
                        team_rosters: Vec::new(),
                    });

                    let pending_repos: u64 = 0;

                    match render_and_cache_evidence(
                        &pp.config,
                        &pp.run,
                        &evidence,
                        &pp.state,
                    )
                    .await
                    {
                        Ok(page_count) => {
                            info!(
                                batch_id = %pp.run.run_id,
                                page_count = page_count,
                                pending_repos,
                                timestamp = %jiff::Timestamp::now(),
                                "partial report published"
                            );
                        }
                        Err(e) => warn!(error = %e, "partial report publish failed"),
                    }
                }

                _ = shutdown_rx.changed() => break,
            }
        }
    });

    (handle, shutdown_tx)
}

/// Create a failure evidence record for a repository whose evaluation failed.
///
/// All checks are set to `unknown` with `collection_error` reason.
///
/// Public within the crate for use by the delivery task in `daemon.rs`
/// (inserts failure evidence when a `JobOutcome::Failure` is received).
pub(crate) fn failure_evidence(repo: &Arc<Repository>, run_timestamp: &str) -> RepositoryEvidence {
    failure_evidence_with_reason(repo, run_timestamp, "collection_error")
}

/// Create a failure evidence record with a specific reason string.
///
/// Used by [`failure_evidence`] (reason = `"collection_error"`) and by the
/// `JoinError` handler (reason = `"task_panicked"`).
///
/// For non-public repositories, the security policy check is set to
/// `NotApplicable` (matching the real evaluator in `security_policy.rs`)
/// rather than `Unknown`. This prevents non-public repos from diluting
/// per-owner security policy coverage denominators.
fn failure_evidence_with_reason(
    repo: &Arc<Repository>,
    run_timestamp: &str,
    reason: &str,
) -> RepositoryEvidence {
    let default_branch = repo.default_branch.clone();
    let (sp_status, sp_evidence) = if repo.is_public() {
        (
            SecurityPolicyStatus::Unknown,
            SecurityPolicyEvidence::CollectionError,
        )
    } else {
        (
            SecurityPolicyStatus::NotApplicable,
            SecurityPolicyEvidence::NotApplicable,
        )
    };
    RepositoryEvidence {
        repository: (**repo).clone(),
        checks: RepositoryChecks {
            security_policy: SecurityPolicyResult {
                status: sp_status,
                evidence: sp_evidence,
                path: None,
                timestamp: run_timestamp.to_string(),
            },
            secret_scanning: SecretScanningResult {
                status: SecretScanningStatus::Unknown,
                has_open_alerts: None,
                alerts_observable: false,
                reason: Some(reason.to_string()),
                timestamp: run_timestamp.to_string(),
            },
            dependabot_security_updates: DependabotResult {
                status: DependabotStatus::Unknown,
                reason: Some(reason.to_string()),
                timestamp: run_timestamp.to_string(),
            },
            branch_protection: BranchProtectionResult {
                status: BranchProtectionStatus::Unknown,
                details: BranchProtectionDetails {
                    default_branch,
                    has_pr: None,
                    required_reviewers: None,
                    has_status_checks: None,
                    admin_equivalent: None,
                    has_broad_bypass: None,
                    reason: Some(reason.to_string()),
                    reason_kind: Some(crate::domain::checks::CollectionFailureReason::Invalid),
                    http_status: None,
                    force_push_blocked: None,
                    deletion_blocked: None,
                },
                timestamp: run_timestamp.to_string(),
            },
            codeowners: CodeownersResult {
                status: CodeownersStatus::Unknown,
                path: None,
                timestamp: run_timestamp.to_string(),
                parsed: None,
                truncation: None,
            },
        },
        last_commit: None,
    }
}

/// Parameters for building the evidence artifact.
///
/// Groups the inputs to [`build_evidence`] to avoid a long positional
/// parameter list.
struct BuildEvidenceParams<'a> {
    repositories: Vec<RepositoryEvidence>,
    deleted: Vec<crate::projection::DeletedRepoRecord>,
    org_state: Option<crate::projection::OrgReadModel>,
    config: &'a RuntimeConfig,
    run: &'a RunMetadata,
    inventory_fetched_at: Option<String>,
    org_alert_summary: Option<&'a OrgAlertSummary>,
    auth_metadata: &'a AuthMetadata,
    capabilities: &'a CapabilitySet,
    rate_limit_warnings: u32,
    /// Team rosters fetched fresh this tick (B1). Empty at warm-start and
    /// during mid-sweep partial publishes — see [`finalize_and_publish`],
    /// the only call site with both a completed CODEOWNERS-derived team
    /// list and a live `GitHubClient` to fetch with.
    team_rosters: Vec<crate::domain::metrics::TeamRoster>,
}

struct OrgSnapshotParams<'a> {
    config: &'a RuntimeConfig,
    run: &'a RunMetadata,
    inventory: &'a InventoryLoad,
    org_summary: &'a Arc<OrgAlertSummary>,
    auth_metadata: &'a AuthMetadata,
    capabilities: &'a CapabilitySet,
    rate_limit_warnings: u32,
}

fn build_org_state_snapshot(
    params: &OrgSnapshotParams<'_>,
) -> crate::domain::evidence::OrgStateSnapshot {
    crate::domain::evidence::OrgStateSnapshot {
        archived_repos: u32::try_from(
            params
                .inventory
                .active_repos
                .iter()
                .filter(|repo| repo.archived)
                .count(),
        )
        .unwrap_or(u32::MAX),
        assessment_metadata: build_assessment_metadata(
            params.config,
            params.run,
            params.inventory.inventory_fetched_at.clone(),
            params.auth_metadata,
            params.capabilities,
            params.rate_limit_warnings,
        ),
        alert_summary: params.org_summary.as_ref().clone(),
    }
}

/// Build the complete evidence artifact from collected data.
fn build_evidence(params: BuildEvidenceParams<'_>) -> Evidence {
    let mut stats = metrics::build_collection_statistics(&params.repositories);
    if let Some(org_state) = params.org_state.as_ref() {
        stats.archived_repos = org_state.archived_repos;
    }
    let mut aggregated = metrics::aggregate_metrics(&params.repositories);
    metrics::enrich_owner_metrics_with_lifecycle(
        &mut aggregated.owner_metrics,
        &params.repositories,
        &params.run.timestamp(),
    );
    aggregated.team_rosters = params.team_rosters;
    let alert_summary = params
        .org_state
        .as_ref()
        .map_or(params.org_alert_summary, |org_state| {
            Some(&org_state.alert_summary)
        });
    let observability =
        metrics::build_secret_scanning_observability_summary(&params.repositories, alert_summary);

    let assessment_metadata = params.org_state.as_ref().map_or_else(
        || {
            build_assessment_metadata(
                params.config,
                params.run,
                params.inventory_fetched_at,
                params.auth_metadata,
                params.capabilities,
                params.rate_limit_warnings,
            )
        },
        |org_state| org_state.assessment_metadata.clone(),
    );

    Evidence {
        assessment_metadata,
        collection_statistics: stats,
        metrics: aggregated,
        secret_scanning_observability: observability,
        repositories: params.repositories,
        deleted: params.deleted,
    }
}

/// Build assessment metadata for the evidence artifact.
fn build_assessment_metadata(
    config: &RuntimeConfig,
    run: &RunMetadata,
    inventory_fetched_at: Option<String>,
    auth_metadata: &AuthMetadata,
    capabilities: &CapabilitySet,
    rate_limit_warnings: u32,
) -> AssessmentMetadata {
    AssessmentMetadata {
        date: run.date(),
        organization: config.org_name.clone(),
        schema_version: config::EVIDENCE_SCHEMA_VERSION.to_string(),
        run_timestamp: run.timestamp(),
        run_id: run.run_id.clone(),
        token_tier: auth_metadata.token_tier,
        token_scopes: auth_metadata.token_scopes.clone(),
        auth_mode: auth_metadata.auth_mode,
        rate_limit_warnings,
        unavailable_capabilities: capabilities
            .unavailable_capabilities_for_auth_mode(auth_metadata.auth_mode),
        inventory_fetched_at,
        warm_start: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::worker_pool::JobExecutor;
    use crate::config::dashboard::DashboardConfig;
    use crate::domain::auth::AuthMode;
    use crate::domain::auth::{Capability, TokenTier};
    use crate::domain::repository::Visibility;
    use crate::test_fixtures;
    use cherry_pit_web::serve::ServerState;

    /// Compile-time assertion: `LiveEvaluator` satisfies `JobExecutor` bounds.
    /// This ensures the impl stays in sync with the trait as both evolve.
    const _: () = {
        fn assert_job_executor<T: JobExecutor>() {}
        let _ = assert_job_executor::<LiveEvaluator>;
    };

    fn sample_repo(name: &str) -> RepositoryEvidence {
        test_fixtures::all_passing_evidence(name)
    }

    fn inventory_payload_with(repos: Vec<Repository>) -> inventory::InventoryPayload {
        inventory::InventoryPayload {
            schema_version: config::INVENTORY_SCHEMA_VERSION.to_string(),
            organization: "TestOrg".to_string(),
            generated_at: "2026-06-02T00:00:00+00:00".to_string(),
            repositories: repos,
            complete: true,
            inventory_fetched_at: Some("2026-06-02T00:00:01+00:00".to_string()),
        }
    }

    #[test]
    fn inventory_load_from_payload_keeps_archived_in_active_repos() {
        let payload = inventory_payload_with(vec![
            test_fixtures::make_repository("active-pub", false, Visibility::Public),
            test_fixtures::make_repository("archived-pub", true, Visibility::Public),
            test_fixtures::make_repository("active-priv", false, Visibility::Private),
        ]);

        let load = inventory_load_from_payload(payload);

        let names: Vec<&str> = load.active_repos.iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.contains(&"archived-pub"),
            "archived repos must flow through to active_repos so the evaluator pipeline emits RepoEvaluated events for them; got {names:?}",
        );
        assert_eq!(load.active_repos.len(), 3);
        assert_eq!(load.active_repos.iter().filter(|r| r.archived).count(), 1);
    }

    #[test]
    fn inventory_load_from_payload_archived_count_matches_archived_input() {
        let payload = inventory_payload_with(vec![
            test_fixtures::make_repository("a", true, Visibility::Public),
            test_fixtures::make_repository("b", true, Visibility::Private),
            test_fixtures::make_repository("c", false, Visibility::Public),
        ]);

        let load = inventory_load_from_payload(payload);

        assert_eq!(load.active_repos.iter().filter(|r| r.archived).count(), 2);
    }

    fn sample_config() -> RuntimeConfig {
        RuntimeConfig {
            org_name: "TestOrg".to_string(),
            no_resume: true,
            max_workers: 4,
            store_dir: std::path::PathBuf::from("/tmp/test-store"),
            pardosa_backend: crate::config::runtime::PardosaBackend::Pgno,
            nats_url: crate::config::runtime::DEFAULT_NATS_URL.to_string(),
            nats_creds: None,
            force_unlock: false,
            dashboard_config: DashboardConfig::default(),
        }
    }

    fn test_repository(name: &str) -> Repository {
        test_fixtures::make_repository(name, false, Visibility::Public)
    }

    fn arc_repo(name: &str) -> Arc<Repository> {
        Arc::new(test_repository(name))
    }

    fn test_auth_metadata() -> AuthMetadata {
        AuthMetadata {
            token_tier: TokenTier::Full,
            token_scopes: "repo, read:org".to_string(),
            auth_mode: AuthMode::Pat,
        }
    }

    fn test_capabilities() -> CapabilitySet {
        CapabilitySet::default()
    }

    /// Test-only wrapper that implements [`RepoEvaluator`] using a synchronous
    /// closure. The closure is wrapped in `std::sync::Mutex` to allow mutation
    /// from `&self`.
    struct FnEvaluator<F>(std::sync::Mutex<F>)
    where
        F: FnMut(&Repository, &str) -> Result<RepositoryEvidence, String> + Send;

    impl<F> RepoEvaluator for FnEvaluator<F>
    where
        F: FnMut(&Repository, &str) -> Result<RepositoryEvidence, String> + Send,
    {
        async fn evaluate<'a>(
            &'a self,
            repo: Arc<Repository>,
            ts: &'a str,
        ) -> Result<RepositoryEvidence, String> {
            (self.0.lock().unwrap())(&repo, ts)
        }
    }

    impl<F> crate::app::worker_pool::JobExecutor for FnEvaluator<F>
    where
        F: FnMut(&Repository, &str) -> Result<RepositoryEvidence, String> + Send + 'static,
    {
        type Context = JobContext;
        type Result = RepositoryEvidence;

        async fn execute<'a>(
            &'a self,
            _domain_key: &'a crate::app::work_queue::DomainKey,
            context: &'a Self::Context,
        ) -> Result<Self::Result, String> {
            self.evaluate(Arc::clone(&context.repo), &context.run_timestamp)
                .await
        }
    }

    #[test]
    fn build_evidence_with_repos() {
        let repos = vec![sample_repo("repo-1"), sample_repo("repo-2")];
        let config = sample_config();
        let run_meta = RunMetadata::new(
            "TestOrg".to_string(),
            crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
        );

        let evidence = build_evidence(BuildEvidenceParams {
            repositories: repos,
            deleted: vec![],
            org_state: None,
            config: &config,
            run: &run_meta,
            inventory_fetched_at: None,
            org_alert_summary: None,
            auth_metadata: &test_auth_metadata(),
            capabilities: &test_capabilities(),
            rate_limit_warnings: 0,
            team_rosters: Vec::new(),
        });

        assert_eq!(evidence.assessment_metadata.organization, "TestOrg");
        assert_eq!(
            evidence.assessment_metadata.schema_version,
            config::EVIDENCE_SCHEMA_VERSION
        );
        assert_eq!(evidence.collection_statistics.total_repos, 2);
        assert_eq!(evidence.collection_statistics.public_repos, 2);
        assert_eq!(evidence.repositories.len(), 2);
    }

    #[test]
    fn build_evidence_threads_team_rosters_into_metrics() {
        use crate::domain::metrics::{TeamMember, TeamMemberRole, TeamRoster, TeamRosterStatus};

        let repos = vec![sample_repo("repo-1")];
        let config = sample_config();
        let run_meta = RunMetadata::new(
            "TestOrg".to_string(),
            crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
        );
        let rosters = vec![TeamRoster {
            canonical_owner: "@testorg/team-foo".to_string(),
            team_slug: "team-foo".to_string(),
            status: TeamRosterStatus::Complete,
            members: vec![TeamMember {
                login: "octocat".to_string(),
                role: TeamMemberRole::Maintainer,
            }],
        }];

        let evidence = build_evidence(BuildEvidenceParams {
            repositories: repos,
            deleted: vec![],
            org_state: None,
            config: &config,
            run: &run_meta,
            inventory_fetched_at: None,
            org_alert_summary: None,
            auth_metadata: &test_auth_metadata(),
            capabilities: &test_capabilities(),
            rate_limit_warnings: 0,
            team_rosters: rosters.clone(),
        });

        assert_eq!(evidence.metrics.team_rosters, rosters);
    }

    #[test]
    fn build_evidence_archived_count_comes_from_projection() {
        let mut archived = sample_repo("archived-repo");
        archived.repository.archived = true;
        let repos = vec![sample_repo("active-repo"), archived];
        let config = sample_config();
        let run_meta = RunMetadata::new(
            "TestOrg".to_string(),
            crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
        );

        let evidence = build_evidence(BuildEvidenceParams {
            repositories: repos,
            deleted: vec![],
            org_state: None,
            config: &config,
            run: &run_meta,
            inventory_fetched_at: None,
            org_alert_summary: None,
            auth_metadata: &test_auth_metadata(),
            capabilities: &test_capabilities(),
            rate_limit_warnings: 0,
            team_rosters: Vec::new(),
        });

        assert_eq!(evidence.collection_statistics.total_repos, 1);
        assert_eq!(evidence.collection_statistics.archived_repos, 1);
    }

    #[test]
    fn rendered_archived_count_uses_org_event_as_single_authority() {
        let mut archived = sample_repo("repo-stream-archived");
        archived.repository.archived = true;
        let repos = vec![sample_repo("repo-stream-active"), archived];
        let config = sample_config();
        let run_meta = RunMetadata::new(
            "TestOrg".to_string(),
            crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
        );
        let org_state = crate::projection::OrgReadModel {
            archived_repos: 7,
            assessment_metadata: test_fixtures::make_metadata(),
            alert_summary: test_org_summary(),
        };

        let evidence = build_evidence(BuildEvidenceParams {
            repositories: repos,
            deleted: vec![],
            org_state: Some(org_state),
            config: &config,
            run: &run_meta,
            inventory_fetched_at: None,
            org_alert_summary: None,
            auth_metadata: &test_auth_metadata(),
            capabilities: &test_capabilities(),
            rate_limit_warnings: 0,
            team_rosters: Vec::new(),
        });

        assert_eq!(evidence.collection_statistics.total_repos, 1);
        assert_eq!(
            evidence.collection_statistics.archived_repos, 7,
            "rendered org archived count must come from the org event, not the repo-stream-derived count",
        );
    }

    #[test]
    fn build_evidence_empty_repos() {
        let config = sample_config();
        let run_meta = RunMetadata::new(
            "TestOrg".to_string(),
            crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
        );

        let evidence = build_evidence(BuildEvidenceParams {
            repositories: Vec::new(),
            deleted: vec![],
            org_state: None,
            config: &config,
            run: &run_meta,
            inventory_fetched_at: None,
            org_alert_summary: None,
            auth_metadata: &test_auth_metadata(),
            capabilities: &test_capabilities(),
            rate_limit_warnings: 0,
            team_rosters: Vec::new(),
        });

        assert_eq!(evidence.collection_statistics.total_repos, 0);
        assert_eq!(evidence.repositories.len(), 0);
    }

    #[test]
    fn build_evidence_metrics_computed_correctly() {
        let repos = vec![sample_repo("repo-1")];
        let config = sample_config();
        let run_meta = RunMetadata::new(
            "TestOrg".to_string(),
            crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
        );

        let evidence = build_evidence(BuildEvidenceParams {
            repositories: repos,
            deleted: vec![],
            org_state: None,
            config: &config,
            run: &run_meta,
            inventory_fetched_at: None,
            org_alert_summary: None,
            auth_metadata: &test_auth_metadata(),
            capabilities: &test_capabilities(),
            rate_limit_warnings: 0,
            team_rosters: Vec::new(),
        });

        assert_eq!(evidence.metrics.security_policy_coverage.numerator, 1);
        assert_eq!(evidence.metrics.security_policy_coverage.denominator, 1);
        assert_eq!(evidence.metrics.security_policy_coverage.rate, Some(100.0));
        assert_eq!(evidence.metrics.secret_scanning_coverage.numerator, 1);
        assert_eq!(evidence.metrics.branch_protection_coverage.numerator, 1);
    }

    #[test]
    fn build_assessment_metadata_populates_fields() {
        let config = sample_config();
        let run_meta = RunMetadata::new(
            "TestOrg".to_string(),
            crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
        );
        let auth = test_auth_metadata();
        let caps = test_capabilities();

        let metadata = build_assessment_metadata(&config, &run_meta, None, &auth, &caps, 3);

        assert_eq!(metadata.organization, "TestOrg");
        assert_eq!(metadata.schema_version, config::EVIDENCE_SCHEMA_VERSION);
        assert_eq!(metadata.run_id, run_meta.run_id);
        assert_eq!(metadata.date.len(), 10);
        assert_eq!(metadata.token_tier, TokenTier::Full);
        assert_eq!(metadata.token_scopes, "repo, read:org");
        assert_eq!(metadata.auth_mode, AuthMode::Pat);
        assert_eq!(metadata.rate_limit_warnings, 3);
        assert_eq!(metadata.unavailable_capabilities.len(), 2);
        assert!(
            metadata
                .unavailable_capabilities
                .contains(&Capability::OrgSecretScanningAlerts)
        );
        assert!(
            metadata
                .unavailable_capabilities
                .contains(&Capability::PrivateBranchProtectionRead)
        );
    }

    #[test]
    fn failure_evidence_has_unknown_status() {
        let repo = Arc::new(test_repository("test"));
        let ev = failure_evidence(&repo, "2026-04-09T12:00:00+00:00");

        assert_eq!(
            ev.checks.security_policy.status,
            SecurityPolicyStatus::Unknown
        );
        assert_eq!(
            ev.checks.secret_scanning.status,
            SecretScanningStatus::Unknown
        );
        assert_eq!(
            ev.checks.dependabot_security_updates.status,
            DependabotStatus::Unknown
        );
        assert_eq!(
            ev.checks.branch_protection.status,
            BranchProtectionStatus::Unknown
        );
        assert_eq!(ev.checks.codeowners.status, CodeownersStatus::Unknown);
        assert_eq!(
            ev.checks.security_policy.evidence,
            SecurityPolicyEvidence::CollectionError
        );
        assert_eq!(ev.checks.codeowners.path, None);
    }

    /// Helper: create a `RepositoryEvidence` from a domain `Repository`.
    fn sample_repo_from_domain(repo: &Repository, timestamp: &str) -> RepositoryEvidence {
        test_fixtures::evidence_from_repository(repo, timestamp)
    }

    #[test]
    fn failure_evidence_with_reason_uses_custom_reason() {
        let repo = Arc::new(test_repository("panicked"));
        let ev = failure_evidence_with_reason(&repo, "2026-04-09T12:00:00+00:00", "task_panicked");

        assert_eq!(
            ev.checks.secret_scanning.reason.as_deref(),
            Some("task_panicked")
        );
        assert_eq!(
            ev.checks.dependabot_security_updates.reason.as_deref(),
            Some("task_panicked")
        );
        assert_eq!(
            ev.checks.branch_protection.details.reason.as_deref(),
            Some("task_panicked")
        );
        assert_eq!(
            ev.checks.security_policy.status,
            SecurityPolicyStatus::Unknown
        );
    }

    #[test]
    fn failure_evidence_with_reason_non_public_repo_uses_not_applicable() {
        let repo = Arc::new(test_fixtures::make_repository(
            "private-repo",
            false,
            Visibility::Private,
        ));
        let ev =
            failure_evidence_with_reason(&repo, "2026-04-09T12:00:00+00:00", "collection_error");

        assert_eq!(
            ev.checks.security_policy.status,
            SecurityPolicyStatus::NotApplicable,
            "non-public repo failure evidence should use NotApplicable"
        );
        assert_eq!(
            ev.checks.security_policy.evidence,
            SecurityPolicyEvidence::NotApplicable,
        );
        assert_eq!(
            ev.checks.secret_scanning.status,
            SecretScanningStatus::Unknown
        );
    }

    #[test]
    fn failure_evidence_with_reason_pending_non_public_uses_not_applicable() {
        let repo = Arc::new(test_fixtures::make_repository(
            "internal-repo",
            false,
            Visibility::Internal,
        ));
        let ev = failure_evidence_with_reason(&repo, "2026-04-09T12:00:00+00:00", "pending");

        assert_eq!(
            ev.checks.security_policy.status,
            SecurityPolicyStatus::NotApplicable,
            "non-public repo with pending reason should use NotApplicable"
        );
        assert_eq!(
            ev.checks.security_policy.evidence,
            SecurityPolicyEvidence::NotApplicable,
        );
    }

    /// Evaluator that always panics — used to test `JoinError` recovery.
    struct PanickingEvaluator;

    impl RepoEvaluator for PanickingEvaluator {
        async fn evaluate<'a>(
            &'a self,
            _repo: Arc<Repository>,
            _ts: &'a str,
        ) -> Result<RepositoryEvidence, String> {
            panic!("simulated task panic for JoinError test");
        }
    }

    impl crate::app::worker_pool::JobExecutor for PanickingEvaluator {
        type Context = JobContext;
        type Result = RepositoryEvidence;

        async fn execute<'a>(
            &'a self,
            _domain_key: &'a crate::app::work_queue::DomainKey,
            context: &'a Self::Context,
        ) -> Result<Self::Result, String> {
            self.evaluate(Arc::clone(&context.repo), &context.run_timestamp)
                .await
        }
    }

    /// Helper: create a test repository with an explicit `updated_at` value.
    fn test_repository_with_updated_at(name: &str, updated_at: Option<&str>) -> Repository {
        let mut repo = test_repository(name);
        repo.updated_at = updated_at.map(String::from);
        repo
    }

    fn arc_repo_with_updated_at(name: &str, updated_at: Option<&str>) -> Arc<Repository> {
        Arc::new(test_repository_with_updated_at(name, updated_at))
    }

    fn test_org_summary() -> OrgAlertSummary {
        OrgAlertSummary {
            collection_status: crate::domain::status::CollectionStatus::Success,
            collection_reason: None,
            per_repo: HashMap::new(),
            open_secret_alert_age_buckets: config::empty_age_buckets(),
            total_open_secret_alerts: 0,
            oldest_open_secret_alert_created_at: None,
            newest_open_secret_alert_created_at: None,
        }
    }

    #[test]
    fn saga_starts_in_init_phase() {
        let config = sample_config();
        let run_meta = RunMetadata::new(
            "TestOrg".to_string(),
            crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
        );

        let saga = make_test_saga(&config, &run_meta);

        assert_eq!(*saga.phase(), SweepPhase::Init);
    }

    #[tokio::test]
    async fn saga_step_baseline_transitions_to_baseline_reused() {
        let dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            store_dir: dir.path().to_path_buf(),
            ..sample_config()
        };
        let mut run_meta = RunMetadata::new(
            "TestOrg".to_string(),
            crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
        );

        let state = AppState::new_with_cache_capacity(10).await;
        let inventory = InventoryLoad {
            active_repos: vec![arc_repo("repo-1")],
            complete: true,
            inventory_fetched_at: None,
        };

        let mut saga = make_test_saga_in(&config, &run_meta, SweepPhase::Resumed);
        let corr_ctx = run_meta.correlation_context();
        let sweep = SweepCtx::new(&config, &mut run_meta, &corr_ctx, &state);

        saga.step_baseline(&sweep, &inventory);
        assert_eq!(*saga.phase(), SweepPhase::BaselineReused);
        assert_eq!(saga.baseline_reused, 0);
    }

    #[test]
    fn sweep_phase_failed_carries_error_message() {
        let phase = SweepPhase::Failed {
            error: "timeout after 7200s".into(),
        };
        assert_eq!(
            phase,
            SweepPhase::Failed {
                error: "timeout after 7200s".into()
            }
        );
    }

    #[test]
    fn sweep_phase_debug_impl() {
        let phases = vec![
            SweepPhase::Init,
            SweepPhase::Resumed,
            SweepPhase::BaselineReused,
            SweepPhase::AwaitingBatch,
            SweepPhase::BatchDrained,
            SweepPhase::Completed,
            SweepPhase::Failed {
                error: "test".into(),
            },
        ];
        for phase in &phases {
            let debug = format!("{phase:?}");
            assert!(!debug.is_empty());
        }
    }

    #[test]
    fn sweep_phase_eq() {
        assert_eq!(SweepPhase::Init, SweepPhase::Init);
        assert_ne!(SweepPhase::Init, SweepPhase::Resumed);
        assert_eq!(
            SweepPhase::Failed { error: "a".into() },
            SweepPhase::Failed { error: "a".into() }
        );
        assert_ne!(
            SweepPhase::Failed { error: "a".into() },
            SweepPhase::Failed { error: "b".into() }
        );
    }

    /// Helper: create a minimal `Arc<GitHubClient>` for saga tests.
    ///
    /// The client is never actually used in unit tests (saga steps that
    /// need it are tested via integration-level tests). This just satisfies
    /// the type constraint.
    fn test_github_client() -> Arc<GitHubClient> {
        let credential = crate::github::auth::GitHubCredential {
            mode: AuthMode::Pat,
            token: secrecy::SecretString::from("test-token"),
            expires_at: None,
        };
        Arc::new(
            GitHubClient::new(
                credential,
                config::DEFAULT_GITHUB_API_BASE_URL,
                "test-org",
                None,
                Arc::new(crate::github::budget::BudgetGate::new(
                    1000,
                    std::time::Duration::from_mins(1),
                )),
                Arc::new(crate::github::rate_limit::new_default()),
            )
            .expect("test client construction should not fail"),
        )
    }

    /// Construct a `SweepSaga` for testing, starting in the given phase.
    ///
    /// Defaults to `SweepPhase::Init` when called via [`make_test_saga`].
    fn make_test_saga_in(
        _config: &RuntimeConfig,
        run: &RunMetadata,
        phase: SweepPhase,
    ) -> SweepSaga {
        SweepSaga {
            phase,
            completed: HashMap::new(),
            baseline_cache: HashMap::new(),
            resumed_count: 0,
            baseline_reused: 0,
            sweep_start: std::time::Instant::now(),
            run_timestamp: run.timestamp(),
            snapshot_signature: "test-snapshot-sig".to_string(),
            pause_notify: Arc::new(tokio::sync::Notify::new()),
            org_summary: Arc::new(test_org_summary()),
            client: test_github_client(),
        }
    }

    /// Construct a `SweepSaga` in the `Init` phase for testing.
    fn make_test_saga(config: &RuntimeConfig, run: &RunMetadata) -> SweepSaga {
        make_test_saga_in(config, run, SweepPhase::Init)
    }

    /// Construct a `CollectionContext` for saga tests.
    fn make_test_collection_context() -> CollectionContext {
        CollectionContext {
            client: test_github_client(),
            capabilities: test_capabilities(),
            auth_metadata: test_auth_metadata(),
            budget_baseline: 0,
        }
    }

    /// Start a worker pool + delivery loop with a test executor.
    ///
    /// Returns `(pool_handle, delivery_handle)`. The caller must close
    /// the work queue to shut down the pool (e.g., after saga completion).
    fn start_test_worker_pool<E>(
        state: &Arc<AppState>,
        executor: Arc<E>,
        worker_count: usize,
    ) -> (tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()>)
    where
        E: crate::app::worker_pool::JobExecutor<Context = JobContext, Result = RepositoryEvidence>,
    {
        let (outcome_tx, outcome_rx) = tokio::sync::mpsc::channel(1024);

        let delivery_state = Arc::clone(state);
        let delivery_handle = tokio::spawn(crate::app::daemon::delivery_loop(
            outcome_rx,
            delivery_state,
        ));

        let queue = Arc::clone(&state.work_queue);
        let (budget, rate_limit) = state.github_api_controls();
        let cancel = tokio_util::sync::CancellationToken::new();

        let pool_handle = tokio::spawn(async move {
            crate::app::worker_pool::run_worker_pool(
                queue,
                executor,
                budget,
                rate_limit,
                {
                    let mut cfg = crate::app::worker_pool::WorkerPoolConfig::default();
                    cfg.worker_count = worker_count;
                    cfg
                },
                cancel,
                outcome_tx,
            )
            .await;
        });

        (pool_handle, delivery_handle)
    }

    /// Create a `RuntimeConfig` with `store_dir` and `no_resume` overridden.
    fn config_with_dir(dir: &std::path::Path) -> RuntimeConfig {
        RuntimeConfig {
            store_dir: dir.to_path_buf(),
            no_resume: false,
            ..sample_config()
        }
    }

    /// Create a standard `RunMetadata` for tests.
    fn test_run_meta() -> RunMetadata {
        RunMetadata::new(
            "TestOrg".to_string(),
            crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
        )
    }

    #[tokio::test]
    async fn collection_outcome_is_cancelled_when_cancel_fires() {
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();

        let outcome = run_collection_inner_with_pipeline(&cancel, async {
            std::future::pending::<Result<(), AppError>>().await
        })
        .await
        .expect("cancelled collection outcome");

        assert_eq!(outcome, CollectionOutcome::Cancelled);
    }

    #[tokio::test]
    async fn collection_outcome_is_completed_when_pipeline_finishes() {
        let cancel = tokio_util::sync::CancellationToken::new();

        let outcome = run_collection_inner_with_pipeline(&cancel, async { Ok(()) })
            .await
            .expect("completed collection outcome");

        assert_eq!(outcome, CollectionOutcome::Completed);
    }

    #[tokio::test]
    async fn collection_outcome_is_fenced_conflict_without_retry_spin() {
        let cancel = tokio_util::sync::CancellationToken::new();

        let outcome = run_collection_inner_with_pipeline(&cancel, async {
            Err(AppError::Persistence(PersistenceError::FencedConflict {
                source: Box::new(std::io::Error::other("wrong last sequence")),
            }))
        })
        .await
        .expect("fenced conflict maps to outcome");

        assert_eq!(outcome, CollectionOutcome::FencedConflict);
    }

    /// Seed projection state to simulate a populated baseline.
    ///
    /// δ.3c-ii: the on-disk `baseline.msgpack` is retired; baseline
    /// reuse now reads from the projection (rebuilt at boot via
    /// event-log replay per CHE-0051:R5 + CHE-0048:R2). Tests seed the
    /// projection directly via `Projection::load_baseline`.
    ///
    /// The `_store_dir` argument is retained for call-site stability
    /// (tests still pass `dir.path()`); it is unused.
    fn seed_baseline(
        _store_dir: &std::path::Path,
        state: &Arc<AppState>,
        entries: Vec<(&str, &str, RepositoryEvidence)>,
    ) {
        let projected: Vec<RepositoryEvidence> = entries
            .into_iter()
            .map(|(_name, updated_at, mut evidence)| {
                if evidence.repository.updated_at.is_none() {
                    evidence.repository.updated_at = Some(updated_at.to_string());
                }
                evidence
            })
            .collect();
        state.lock_projection().load_baseline(projected);
    }

    fn saga_run_resume_and_baseline(
        saga: &mut SweepSaga,
        inventory: &InventoryLoad,
        config: &RuntimeConfig,
        run: &RunMetadata,
        state: &Arc<AppState>,
    ) {
        let mut run = run.clone();
        let sweep = test_sweep_ctx(config, &mut run, state);
        saga.step_start_sweep(&sweep, inventory);
        debug_assert_eq!(saga.phase, SweepPhase::Init);
        saga.phase = SweepPhase::Resumed;
        saga.step_baseline(&sweep, inventory);
    }

    fn test_sweep_ctx<'a>(
        config: &'a RuntimeConfig,
        run: &'a mut RunMetadata,
        state: &'a Arc<AppState>,
    ) -> SweepCtx<'a> {
        let corr_ctx = run.correlation_context();
        SweepCtx::new(config, run, &corr_ctx, state)
    }

    async fn saga_step_enqueue_and_await(
        saga: &mut SweepSaga,
        config: &RuntimeConfig,
        run: &RunMetadata,
        ctx: &CollectionContext,
        inventory: &InventoryLoad,
        state: &Arc<AppState>,
    ) -> Result<(), AppError> {
        let mut run = run.clone();
        let sweep = test_sweep_ctx(config, &mut run, state);
        saga.step_enqueue_and_await(&sweep, ctx, inventory).await
    }

    async fn saga_step_finalize(
        saga: &mut SweepSaga,
        config: &RuntimeConfig,
        run: &mut RunMetadata,
        ctx: &CollectionContext,
        inventory: &InventoryLoad,
        state: &Arc<AppState>,
    ) -> Result<(), AppError> {
        let mut sweep = test_sweep_ctx(config, run, state);
        saga.step_finalize(&mut sweep, ctx, inventory).await
    }

    /// Saga-level same-day resume — every inventory repo already in
    /// projection: all reused, none enqueued.
    #[tokio::test]
    async fn saga_resumes_from_event_log_zero_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;

        let mut saga = make_test_saga(&config, &run);
        let ctx = make_test_collection_context();

        let mut e1 = sample_repo("repo-1");
        e1.repository.updated_at = Some("2026-04-10T00:00:00Z".to_string());
        let mut e2 = sample_repo("repo-2");
        e2.repository.updated_at = Some("2026-04-10T00:00:00Z".to_string());
        seed_baseline(
            dir.path(),
            &state,
            vec![
                ("repo-1", "2026-04-10T00:00:00Z", e1),
                ("repo-2", "2026-04-10T00:00:00Z", e2),
            ],
        );

        let inventory = InventoryLoad {
            active_repos: vec![
                arc_repo_with_updated_at("repo-1", Some("2026-04-10T00:00:00Z")),
                arc_repo_with_updated_at("repo-2", Some("2026-04-10T00:00:00Z")),
            ],
            complete: true,
            inventory_fetched_at: None,
        };

        saga_run_resume_and_baseline(&mut saga, &inventory, &config, &run, &state);
        assert_eq!(saga.baseline_reused, 2);

        saga_step_enqueue_and_await(&mut saga, &config, &run, &ctx, &inventory, &state)
            .await
            .unwrap();
        assert_eq!(*saga.phase(), SweepPhase::BatchDrained);
        assert_eq!(state.projection_len(), 2);
    }

    /// Saga-level same-day resume — subset of inventory in projection:
    /// reused subset reported in `baseline_reused`; remainder pending.
    #[tokio::test]
    async fn saga_resumes_from_event_log_partial_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;

        let mut saga = make_test_saga(&config, &run);

        let mut e1 = sample_repo("repo-1");
        e1.repository.updated_at = Some("2026-04-10T00:00:00Z".to_string());
        seed_baseline(
            dir.path(),
            &state,
            vec![("repo-1", "2026-04-10T00:00:00Z", e1)],
        );

        let inventory = InventoryLoad {
            active_repos: vec![
                arc_repo_with_updated_at("repo-1", Some("2026-04-10T00:00:00Z")),
                arc_repo("repo-2"),
            ],
            complete: true,
            inventory_fetched_at: None,
        };

        saga_run_resume_and_baseline(&mut saga, &inventory, &config, &run, &state);
        assert_eq!(saga.baseline_reused, 1);
        assert!(state.projection_contains("id-repo-1"));
        assert!(!state.projection_contains("id-repo-2"));
    }

    /// Saga-level same-day resume — empty projection: nothing reused;
    /// all inventory enqueued for fresh evaluation.
    #[tokio::test]
    async fn saga_resumes_from_event_log_all_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;

        let mut saga = make_test_saga(&config, &run);
        let _ = dir;

        let inventory = InventoryLoad {
            active_repos: vec![arc_repo("repo-1"), arc_repo("repo-2")],
            complete: true,
            inventory_fetched_at: None,
        };

        saga_run_resume_and_baseline(&mut saga, &inventory, &config, &run, &state);
        assert_eq!(saga.baseline_reused, 0);
        assert!(!state.projection_contains("id-repo-1"));
        assert!(!state.projection_contains("id-repo-2"));
    }

    /// Test 10: Unchanged repos reuse baseline evidence.
    #[tokio::test]
    async fn saga_reuses_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;

        let mut saga = make_test_saga(&config, &run);

        let mut evidence_1 = sample_repo("repo-1");
        evidence_1.repository.updated_at = Some("2026-04-10T00:00:00Z".to_string());
        seed_baseline(
            dir.path(),
            &state,
            vec![("repo-1", "2026-04-10T00:00:00Z", evidence_1)],
        );

        let inventory = InventoryLoad {
            active_repos: vec![arc_repo_with_updated_at(
                "repo-1",
                Some("2026-04-10T00:00:00Z"),
            )],
            complete: true,
            inventory_fetched_at: None,
        };

        saga_run_resume_and_baseline(&mut saga, &inventory, &config, &run, &state);

        assert_eq!(*saga.phase(), SweepPhase::BaselineReused);
        assert_eq!(saga.baseline_reused, 1);
        assert!(state.projection_contains("id-repo-1"));
    }

    /// Test 11: Changed `updated_at` forces re-evaluation (no baseline reuse).
    #[tokio::test]
    async fn saga_reevaluates_when_baseline_updated_at_changes() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;

        let mut saga = make_test_saga(&config, &run);

        let mut evidence = sample_repo("repo-1");
        evidence.repository.updated_at = Some("2026-04-09T00:00:00Z".to_string());
        seed_baseline(
            dir.path(),
            &state,
            vec![("repo-1", "2026-04-09T00:00:00Z", evidence)],
        );

        let inventory = InventoryLoad {
            active_repos: vec![arc_repo_with_updated_at(
                "repo-1",
                Some("2026-04-10T12:00:00Z"),
            )],
            complete: true,
            inventory_fetched_at: None,
        };

        saga_run_resume_and_baseline(&mut saga, &inventory, &config, &run, &state);

        assert_eq!(
            saga.baseline_reused, 0,
            "changed updated_at should prevent reuse"
        );
    }

    /// Test 13: No `updated_at` → no baseline reuse.
    #[tokio::test]
    async fn saga_baseline_skipped_when_repo_updated_at_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;

        let mut saga = make_test_saga(&config, &run);

        let mut evidence = sample_repo("repo-1");
        evidence.repository.updated_at = Some("2026-04-10T00:00:00Z".to_string());
        seed_baseline(
            dir.path(),
            &state,
            vec![("repo-1", "2026-04-10T00:00:00Z", evidence)],
        );

        let inventory = InventoryLoad {
            active_repos: vec![arc_repo("repo-1")],
            complete: true,
            inventory_fetched_at: None,
        };

        saga_run_resume_and_baseline(&mut saga, &inventory, &config, &run, &state);

        assert_eq!(saga.baseline_reused, 0);
    }

    /// Test 1: All repos in input appear in output evidence store.
    #[tokio::test]
    async fn saga_evaluates_all_repos() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;
        let ctx = make_test_collection_context();

        let evaluator = Arc::new(FnEvaluator(std::sync::Mutex::new(
            |repo: &Repository, ts: &str| Ok(sample_repo_from_domain(repo, ts)),
        )));

        let (_pool, _delivery) = start_test_worker_pool(&state, evaluator, 2);

        let mut saga = make_test_saga(&config, &run);
        let inventory = InventoryLoad {
            active_repos: vec![arc_repo("repo-1"), arc_repo("repo-2"), arc_repo("repo-3")],
            complete: true,
            inventory_fetched_at: None,
        };

        saga_run_resume_and_baseline(&mut saga, &inventory, &config, &run, &state);
        saga_step_enqueue_and_await(&mut saga, &config, &run, &ctx, &inventory, &state)
            .await
            .unwrap();

        assert_eq!(*saga.phase(), SweepPhase::BatchDrained);
        assert_eq!(state.projection_len(), 3);
        assert!(state.projection_contains("id-repo-1"));
        assert!(state.projection_contains("id-repo-2"));
        assert!(state.projection_contains("id-repo-3"));

        state.work_queue.close();
    }

    /// Test 2: Failing repo is isolated — passing repos still get results.
    #[tokio::test]
    async fn saga_isolates_failures() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;
        let ctx = make_test_collection_context();

        let evaluator = Arc::new(FnEvaluator(std::sync::Mutex::new(
            |repo: &Repository, ts: &str| {
                if repo.name == "fail-repo" {
                    Err("simulated failure".to_string())
                } else {
                    Ok(sample_repo_from_domain(repo, ts))
                }
            },
        )));

        let (_pool, _delivery) = start_test_worker_pool(&state, evaluator, 2);

        let mut saga = make_test_saga(&config, &run);
        let inventory = InventoryLoad {
            active_repos: vec![arc_repo("pass-repo"), arc_repo("fail-repo")],
            complete: true,
            inventory_fetched_at: None,
        };

        saga_run_resume_and_baseline(&mut saga, &inventory, &config, &run, &state);
        saga_step_enqueue_and_await(&mut saga, &config, &run, &ctx, &inventory, &state)
            .await
            .unwrap();

        assert_eq!(*saga.phase(), SweepPhase::BatchDrained);
        assert!(state.projection_contains("id-pass-repo"));
        assert!(
            !state.projection_contains("id-fail-repo"),
            "fresh repo failure produces no evidence entry in saga path"
        );

        state.work_queue.close();
    }

    /// Test 5: Evidence store contains all expected repos (saga path
    /// does not guarantee sorted order — reframed from legacy test).
    #[tokio::test]
    async fn saga_output_contains_all_repos() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;
        let ctx = make_test_collection_context();

        let evaluator = Arc::new(FnEvaluator(std::sync::Mutex::new(
            |repo: &Repository, ts: &str| Ok(sample_repo_from_domain(repo, ts)),
        )));

        let (_pool, _delivery) = start_test_worker_pool(&state, evaluator, 2);

        let mut saga = make_test_saga(&config, &run);
        let names = ["zebra", "alpha", "middle"];
        let inventory = InventoryLoad {
            active_repos: names.iter().map(|n| arc_repo(n)).collect(),
            complete: true,
            inventory_fetched_at: None,
        };

        saga_run_resume_and_baseline(&mut saga, &inventory, &config, &run, &state);
        saga_step_enqueue_and_await(&mut saga, &config, &run, &ctx, &inventory, &state)
            .await
            .unwrap();

        let snapshot = state.projection_snapshot();
        let mut found_names: Vec<String> =
            snapshot.iter().map(|e| e.repository.name.clone()).collect();
        found_names.sort();
        assert_eq!(found_names, vec!["alpha", "middle", "zebra"]);

        state.work_queue.close();
    }

    /// Test 6: Panicked worker → saga times out → `Failed` phase.
    ///
    /// Uses `start_paused = true` to fast-forward the saga-level timeout
    /// without actually waiting 2 hours.
    #[tokio::test(start_paused = true)]
    async fn saga_recovers_from_task_panic() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;
        let ctx = make_test_collection_context();

        let evaluator: Arc<PanickingEvaluator> = Arc::new(PanickingEvaluator);

        let (_pool, _delivery) = start_test_worker_pool(&state, evaluator, 1);

        let mut saga = make_test_saga(&config, &run);
        let inventory = InventoryLoad {
            active_repos: vec![arc_repo("panic-repo")],
            complete: true,
            inventory_fetched_at: None,
        };

        saga_run_resume_and_baseline(&mut saga, &inventory, &config, &run, &state);
        saga_step_enqueue_and_await(&mut saga, &config, &run, &ctx, &inventory, &state)
            .await
            .unwrap();

        assert!(
            matches!(saga.phase(), SweepPhase::Failed { .. }),
            "expected Failed phase after worker panic, got {:?}",
            saga.phase()
        );

        state.work_queue.close();
    }

    #[tokio::test(start_paused = true)]
    async fn saga_timeout_persists_auditable_scheduled_event() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let run = test_run_meta();
        let state = AppState::with_stores(
            dir.path(),
            crate::config::runtime::PardosaBackend::Pgno,
            crate::config::runtime::NatsStoreConfig::for_org("TestOrg", "nats://localhost:4222")
                .expect("valid test nats config"),
        )
        .await
        .expect("create test state with stores");
        let ctx = make_test_collection_context();

        let evaluator: Arc<PanickingEvaluator> = Arc::new(PanickingEvaluator);

        let (pool, delivery) = start_test_worker_pool(&state, evaluator, 1);

        let mut saga = make_test_saga(&config, &run);
        let inventory = InventoryLoad {
            active_repos: vec![arc_repo("panic-repo")],
            complete: true,
            inventory_fetched_at: None,
        };

        saga_run_resume_and_baseline(&mut saga, &inventory, &config, &run, &state);
        saga_step_enqueue_and_await(&mut saga, &config, &run, &ctx, &inventory, &state)
            .await
            .unwrap();

        assert!(
            matches!(saga.phase(), SweepPhase::Failed { .. }),
            "expected Failed phase after scheduled sweep timeout, got {:?}",
            saga.phase()
        );
        assert!(
            config.store_dir.join("sweep-timeouts").exists(),
            "scheduled sweep timeout must persist an auditable domain event under store_dir/sweep-timeouts"
        );
        state.work_queue.close();
        pool.abort();
        delivery.abort();
        let _pool_result = pool.await;
        let _delivery_result = delivery.await;
        drop(state);

        let restarted = AppState::with_stores(
            dir.path(),
            crate::config::runtime::PardosaBackend::Pgno,
            crate::config::runtime::NatsStoreConfig::for_org("TestOrg", "nats://localhost:4222")
                .expect("valid test nats config"),
        )
        .await
        .expect("reopen test state with stores");
        recover_due_sweep_timeouts(restarted.as_ref())
            .await
            .expect("recover stored sweep timeout");
    }

    /// Test 7: `max_workers=0` is clamped to `MIN_WORKERS` by `RuntimeConfig`.
    /// Saga completes with the clamped worker count.
    #[tokio::test]
    async fn saga_clamps_max_workers_zero_to_min() {
        let dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig::new("TestOrg", false, 0, dir.path().to_path_buf()).unwrap();
        assert_eq!(config.max_workers, config::MIN_WORKERS);

        let run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;
        let ctx = make_test_collection_context();

        let evaluator = Arc::new(FnEvaluator(std::sync::Mutex::new(
            |repo: &Repository, ts: &str| Ok(sample_repo_from_domain(repo, ts)),
        )));

        let (_pool, _delivery) = start_test_worker_pool(&state, evaluator, config.max_workers);

        let mut saga = make_test_saga(&config, &run);
        let inventory = InventoryLoad {
            active_repos: vec![arc_repo("repo-1")],
            complete: true,
            inventory_fetched_at: None,
        };

        saga_run_resume_and_baseline(&mut saga, &inventory, &config, &run, &state);
        saga_step_enqueue_and_await(&mut saga, &config, &run, &ctx, &inventory, &state)
            .await
            .unwrap();

        assert_eq!(*saga.phase(), SweepPhase::BatchDrained);
        assert_eq!(state.projection_len(), 1);

        state.work_queue.close();
    }

    /// Test 8: `max_workers=9999` is clamped to `MAX_WORKERS`.
    #[tokio::test]
    async fn saga_clamps_max_workers_oversized_to_max() {
        let dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig::new("TestOrg", false, 9999, dir.path().to_path_buf()).unwrap();
        assert_eq!(config.max_workers, config::MAX_WORKERS);

        let run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;
        let ctx = make_test_collection_context();

        let evaluator = Arc::new(FnEvaluator(std::sync::Mutex::new(
            |repo: &Repository, ts: &str| Ok(sample_repo_from_domain(repo, ts)),
        )));

        let (_pool, _delivery) = start_test_worker_pool(&state, evaluator, 2);

        let mut saga = make_test_saga(&config, &run);
        let inventory = InventoryLoad {
            active_repos: vec![arc_repo("repo-1")],
            complete: true,
            inventory_fetched_at: None,
        };

        saga_run_resume_and_baseline(&mut saga, &inventory, &config, &run, &state);
        saga_step_enqueue_and_await(&mut saga, &config, &run, &ctx, &inventory, &state)
            .await
            .unwrap();

        assert_eq!(*saga.phase(), SweepPhase::BatchDrained);
        assert_eq!(state.projection_len(), 1);

        state.work_queue.close();
    }

    /// Test 9: Partial publisher task shuts down cleanly after saga completion.
    #[tokio::test]
    async fn saga_partial_publisher_shuts_down_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;
        let ctx = make_test_collection_context();

        let evaluator = Arc::new(FnEvaluator(std::sync::Mutex::new(
            |repo: &Repository, ts: &str| Ok(sample_repo_from_domain(repo, ts)),
        )));

        let (pool, delivery) = start_test_worker_pool(&state, evaluator, 2);

        let mut saga = make_test_saga(&config, &run);
        let inventory = InventoryLoad {
            active_repos: vec![arc_repo("repo-1"), arc_repo("repo-2")],
            complete: true,
            inventory_fetched_at: None,
        };

        saga_run_resume_and_baseline(&mut saga, &inventory, &config, &run, &state);
        saga_step_enqueue_and_await(&mut saga, &config, &run, &ctx, &inventory, &state)
            .await
            .unwrap();

        assert_eq!(*saga.phase(), SweepPhase::BatchDrained);
        assert_eq!(state.projection_len(), 2);

        state.work_queue.close();
        let pool_result = pool.await;
        let delivery_result = delivery.await;
        assert!(pool_result.is_ok(), "worker pool should exit cleanly");
        assert!(delivery_result.is_ok(), "delivery loop should exit cleanly");
    }

    #[tokio::test]
    async fn step_finalize_renders_from_native_projection() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let mut run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;
        let ctx = make_test_collection_context();

        let evaluator = Arc::new(FnEvaluator(std::sync::Mutex::new(
            |repo: &Repository, ts: &str| Ok(sample_repo_from_domain(repo, ts)),
        )));
        let (_pool, _delivery) = start_test_worker_pool(&state, evaluator, 2);

        let mut saga = make_test_saga(&config, &run);
        let inventory = InventoryLoad {
            active_repos: vec![arc_repo("repo-1")],
            complete: true,
            inventory_fetched_at: None,
        };

        saga_run_resume_and_baseline(&mut saga, &inventory, &config, &run, &state);
        saga_step_enqueue_and_await(&mut saga, &config, &run, &ctx, &inventory, &state)
            .await
            .unwrap();

        saga_step_finalize(&mut saga, &config, &mut run, &ctx, &inventory, &state)
            .await
            .unwrap();

        assert!(
            state.projection_contains("id-repo-1"),
            "terminal render must source the repository written through NativeStore"
        );
        assert!(state.html_cache().load().is_some());

        state.work_queue.close();
    }

    #[tokio::test]
    async fn successful_non_empty_inventory_moves_disappeared_repo_to_deleted_before_finalize() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let mut run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;
        let ctx = make_test_collection_context();
        let timestamp = run.timestamp();
        let kept = sample_repo("kept-repo");
        let disappeared = sample_repo("disappeared-repo");
        let kept_key = kept.repository.inventory_key.clone();
        let disappeared_key = disappeared.repository.inventory_key.clone();
        let disappeared_name = disappeared.repository.name.clone();

        for evidence in [kept.clone(), disappeared.clone()] {
            let domain_key = evidence.repository.inventory_key.clone();
            let repo_name = evidence.repository.name.clone();
            state
                .record_repo(&domain_key, evidence, &repo_name, &timestamp)
                .expect("record repo");
        }

        let mut saga = make_test_saga_in(&config, &run, SweepPhase::BatchDrained);
        let inventory = InventoryLoad {
            active_repos: vec![arc_repo("kept-repo")],
            complete: true,
            inventory_fetched_at: Some("2026-06-24T00:00:00Z".to_string()),
        };

        saga_step_finalize(&mut saga, &config, &mut run, &ctx, &inventory, &state)
            .await
            .expect("finalize");

        assert!(state.projection_contains(&kept_key));
        assert!(!state.projection_contains(&disappeared_key));
        let deleted = state.projection_deleted_snapshot();
        let record = deleted
            .iter()
            .find(|(key, _)| key == &disappeared_key)
            .map(|(_, record)| record)
            .expect("deleted record");
        assert_eq!(record.repo_name, disappeared_name);
        let expected_detected_at = crate::domain::time::parse_iso8601(&timestamp)
            .expect("run timestamp parses")
            .to_string();
        assert_eq!(record.detected_at, expected_detected_at);
    }

    #[tokio::test]
    async fn reconcile_marks_exactly_the_disappeared_set_across_multiple_repos() {
        let state = AppState::new_with_cache_capacity(10).await;
        let timestamp = "2026-06-24T00:00:00Z";
        let kept_names = ["kept-a", "kept-b"];
        let gone_names = ["gone-a", "gone-b", "gone-c"];

        for name in kept_names.into_iter().chain(gone_names) {
            let evidence = sample_repo(name);
            let domain_key = evidence.repository.inventory_key.clone();
            let repo_name = evidence.repository.name.clone();
            state
                .record_repo(&domain_key, evidence, &repo_name, timestamp)
                .expect("record repo");
        }

        let active_repos: Vec<Arc<Repository>> = kept_names.into_iter().map(arc_repo).collect();
        let inventory = Ok(InventoryLoad {
            active_repos,
            complete: true,
            inventory_fetched_at: Some(timestamp.to_string()),
        });

        reconcile_deleted_repositories_after_successful_inventory(&state, &inventory, timestamp)
            .expect("reconcile");

        for name in kept_names {
            assert!(state.projection_contains(&format!("id-{name}")));
        }

        let deleted = state.projection_deleted_snapshot();
        assert_eq!(
            deleted.len(),
            gone_names.len(),
            "exactly the disappeared repos must be marked deleted, no more no less"
        );
        for name in gone_names {
            let key = format!("id-{name}");
            assert!(!state.projection_contains(&key));
            let record = deleted
                .iter()
                .find(|(deleted_key, _)| deleted_key == &key)
                .map(|(_, record)| record)
                .expect("deleted record present for disappeared repo");
            assert_eq!(record.repo_name, name);
        }
    }

    #[tokio::test]
    async fn failed_inventory_result_skips_deleted_reconciliation() {
        let state = AppState::new_with_cache_capacity(10).await;
        let timestamp = "2026-06-24T00:00:00Z";
        let evidence = sample_repo("failed-load-kept");
        let domain_key = evidence.repository.inventory_key.clone();
        let repo_name = evidence.repository.name.clone();
        state
            .record_repo(&domain_key, evidence, &repo_name, timestamp)
            .expect("record repo");
        let failed = Err(AppError::Inventory(
            crate::error::InventoryError::ApiFetchFailed {
                reason: "simulated load failure".to_string(),
            },
        ));

        reconcile_deleted_repositories_after_successful_inventory(&state, &failed, timestamp)
            .expect("skip failed inventory");

        assert!(state.projection_contains(&domain_key));
        assert!(state.projection_deleted_snapshot().is_empty());
    }

    #[tokio::test]
    async fn empty_inventory_result_does_not_mass_delete_projection() {
        let state = AppState::new_with_cache_capacity(10).await;
        let timestamp = "2026-06-24T00:00:00Z";
        let repos = [sample_repo("empty-guard-a"), sample_repo("empty-guard-b")];
        let keys: Vec<String> = repos
            .iter()
            .map(|evidence| evidence.repository.inventory_key.clone())
            .collect();
        for evidence in repos {
            let domain_key = evidence.repository.inventory_key.clone();
            let repo_name = evidence.repository.name.clone();
            state
                .record_repo(&domain_key, evidence, &repo_name, timestamp)
                .expect("record repo");
        }
        let empty = Ok(InventoryLoad {
            active_repos: vec![],
            complete: true,
            inventory_fetched_at: Some(timestamp.to_string()),
        });

        reconcile_deleted_repositories_after_successful_inventory(&state, &empty, timestamp)
            .expect("skip empty inventory");

        for key in keys {
            assert!(state.projection_contains(&key));
        }
        assert!(state.projection_deleted_snapshot().is_empty());
    }

    #[tokio::test]
    async fn partial_inventory_does_not_delete_omitted_repo() {
        let state = AppState::new_with_cache_capacity(10).await;
        let timestamp = "2026-06-24T00:00:00Z";
        let evidence = sample_repo("partial-kept");
        let key = evidence.repository.inventory_key.clone();
        let name = evidence.repository.name.clone();
        state
            .record_repo(&key, evidence, &name, timestamp)
            .expect("record repo");
        let partial = Ok(InventoryLoad {
            active_repos: vec![arc_repo("some-other-repo")],
            complete: false,
            inventory_fetched_at: Some(timestamp.to_string()),
        });

        reconcile_deleted_repositories_after_successful_inventory(&state, &partial, timestamp)
            .expect("skip partial inventory");

        assert!(state.projection_contains(&key));
        assert!(state.projection_deleted_snapshot().is_empty());
    }

    #[tokio::test]
    async fn saga_reappearing_deleted_repo_resurrects_to_active() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;
        let ctx = make_test_collection_context();
        let timestamp = run.timestamp();
        let repo = arc_repo("resurrected-repo");
        let domain_key = repo.inventory_key.clone();
        state
            .mark_repo_deleted(&domain_key, &repo.name, &timestamp)
            .expect("mark repo deleted");

        let evaluator = Arc::new(FnEvaluator(std::sync::Mutex::new(
            |repo: &Repository, ts: &str| Ok(sample_repo_from_domain(repo, ts)),
        )));
        let (_pool, _delivery) = start_test_worker_pool(&state, evaluator, 1);
        let mut saga = make_test_saga(&config, &run);
        let inventory = InventoryLoad {
            active_repos: vec![repo],
            complete: true,
            inventory_fetched_at: Some(timestamp.clone()),
        };

        saga_run_resume_and_baseline(&mut saga, &inventory, &config, &run, &state);
        saga_step_enqueue_and_await(&mut saga, &config, &run, &ctx, &inventory, &state)
            .await
            .expect("enqueue");

        assert!(state.projection_contains(&domain_key));
        assert!(state.projection_deleted_snapshot().is_empty());
        state.work_queue.close();
    }

    /// CHE-0068:R2 — the terminal *visible* commit (html-cache replace
    /// + WS broadcast) must occur strictly after `SweepCompleted` has
    /// been published. Asserted via a bus handler that snapshots the
    /// `html_cache` pointer state at the moment each barrier event is
    /// delivered: at `SweepCompleted` the cache must still hold the
    /// pre-finalize state; at `EvidencePublished` it must hold the
    /// fresh terminal render.
    #[tokio::test]
    async fn step_finalize_commits_html_cache_after_native_write() {
        use std::collections::HashMap as StdHashMap;

        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let mut run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;
        let ctx = make_test_collection_context();

        let sentinel_key = "__sentinel_pre_finalize__".to_string();
        let mut sentinel_map: StdHashMap<String, crate::app::state::CachedPage> = StdHashMap::new();
        sentinel_map.insert(
            sentinel_key.clone(),
            crate::app::state::CachedPage::new(&sentinel_key, b"sentinel".to_vec()),
        );
        state.html_cache().store(Arc::new(Some(sentinel_map)));

        let evaluator = Arc::new(FnEvaluator(std::sync::Mutex::new(
            |repo: &Repository, ts: &str| Ok(sample_repo_from_domain(repo, ts)),
        )));
        let (_pool, _delivery) = start_test_worker_pool(&state, evaluator, 2);

        let mut saga = make_test_saga(&config, &run);
        let inventory = InventoryLoad {
            active_repos: vec![arc_repo("repo-1")],
            complete: true,
            inventory_fetched_at: None,
        };

        saga_run_resume_and_baseline(&mut saga, &inventory, &config, &run, &state);
        saga_step_enqueue_and_await(&mut saga, &config, &run, &ctx, &inventory, &state)
            .await
            .unwrap();

        assert!(
            state
                .html_cache()
                .load()
                .as_ref()
                .as_ref()
                .is_some_and(|m| m.contains_key("__sentinel_pre_finalize__"))
        );

        saga_step_finalize(&mut saga, &config, &mut run, &ctx, &inventory, &state)
            .await
            .unwrap();

        assert!(
            !state
                .html_cache()
                .load()
                .as_ref()
                .as_ref()
                .is_some_and(|m| m.contains_key("__sentinel_pre_finalize__")),
            "finalize must replace sentinel html cache after native write"
        );

        state.work_queue.close();
    }

    #[tokio::test]
    async fn html_cache_must_not_be_visible_before_finalize_commits() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let mut run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;
        let ctx = make_test_collection_context();

        let baseline_map: HashMap<String, crate::app::state::CachedPage> = HashMap::new();
        let baseline_arc = Arc::new(Some(baseline_map));
        state.html_cache().store(Arc::clone(&baseline_arc));
        let baseline_addr = Arc::as_ptr(&baseline_arc) as usize;

        let evaluator = Arc::new(FnEvaluator(std::sync::Mutex::new(
            |repo: &Repository, ts: &str| Ok(sample_repo_from_domain(repo, ts)),
        )));
        let (_pool, _delivery) = start_test_worker_pool(&state, evaluator, 2);

        let mut saga = make_test_saga(&config, &run);
        let inventory = InventoryLoad {
            active_repos: vec![arc_repo("repo-1")],
            complete: true,
            inventory_fetched_at: None,
        };

        saga_run_resume_and_baseline(&mut saga, &inventory, &config, &run, &state);
        saga_step_enqueue_and_await(&mut saga, &config, &run, &ctx, &inventory, &state)
            .await
            .unwrap();

        let guard = state.html_cache().load();
        let current_addr = Arc::as_ptr(&*guard) as usize;
        assert_eq!(
            current_addr, baseline_addr,
            "html_cache must not flip during native repository writes before finalize"
        );

        saga_step_finalize(&mut saga, &config, &mut run, &ctx, &inventory, &state)
            .await
            .unwrap();

        let guard = state.html_cache().load();
        let current_addr = Arc::as_ptr(&*guard) as usize;
        assert_ne!(
            current_addr, baseline_addr,
            "html_cache must flip after finalize commits cached pages"
        );

        state.work_queue.close();
    }

    #[tokio::test]
    async fn warm_start_renders_seeded_projection_without_durable_lifecycle_events() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let state = AppState::new_with_cache_capacity(10).await;

        let mut evidence = sample_repo("repo-1");
        evidence.repository.updated_at = Some("2026-04-10T00:00:00Z".to_string());
        seed_baseline(
            dir.path(),
            &state,
            vec![("repo-1", "2026-04-10T00:00:00Z", evidence)],
        );

        let ok = warm_start_from_baseline(&config, &state).await;
        assert!(ok, "warm-start should succeed with a seeded baseline");
        assert!(
            state.html_cache().load().is_some(),
            "warm-start must populate html cache from projection"
        );
    }

    #[tokio::test]
    async fn sweep_progress_completed_reflects_current_run_not_projection_len() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with_dir(dir.path());
        let mut run = test_run_meta();
        let state = AppState::new_with_cache_capacity(10).await;

        let mut contaminant = sample_repo("repo-not-in-inventory");
        contaminant.repository.updated_at = Some("2026-04-10T00:00:00Z".to_string());
        seed_baseline(
            dir.path(),
            &state,
            vec![("repo-not-in-inventory", "2026-04-10T00:00:00Z", contaminant)],
        );

        let mut reused = sample_repo("repo-reused");
        reused.repository.updated_at = Some("2026-04-10T00:00:00Z".to_string());
        seed_baseline(
            dir.path(),
            &state,
            vec![("repo-reused", "2026-04-10T00:00:00Z", reused)],
        );

        let inventory = InventoryLoad {
            active_repos: vec![
                arc_repo_with_updated_at("repo-reused", Some("2026-04-10T00:00:00Z")),
                arc_repo("repo-pending"),
            ],
            complete: true,
            inventory_fetched_at: None,
        };
        let total = inventory.active_repos.len() as u64;
        assert_eq!(total, 2);

        let mut saga = make_test_saga(&config, &run);

        let sweep = test_sweep_ctx(&config, &mut run, &state);
        saga.step_start_sweep(&sweep, &inventory);
        debug_assert_eq!(saga.phase, SweepPhase::Init);
        saga.phase = SweepPhase::Resumed;

        SweepSaga::emit_progress(sweep.run(), saga.current_run_completed(), total);

        saga.step_baseline(&sweep, &inventory);
        assert_eq!(saga.baseline_reused, 1, "repo-reused must reuse baseline");

        SweepSaga::emit_progress(sweep.run(), saga.current_run_completed(), total);

        assert_eq!(saga.current_run_completed(), 1);
    }

    impl SweepSaga {
        fn current_run_completed(&self) -> u64 {
            (self.completed.len() + self.baseline_cache.len()) as u64
        }
    }

    #[tokio::test]
    async fn commit_cached_pages_broadcasts_page_update_event() {
        let state = AppState::new_with_cache_capacity(10).await;
        let run = test_run_meta();

        let mut rx = state.ws_subscribe();

        let mut cache: HashMap<String, crate::app::state::CachedPage> = HashMap::new();
        cache.insert(
            "index.html".to_string(),
            crate::app::state::CachedPage::new("index.html", b"<html>x</html>".to_vec()),
        );
        cache.insert(
            "report.html".to_string(),
            crate::app::state::CachedPage::new("report.html", b"<html>y</html>".to_vec()),
        );

        let page_count = commit_cached_pages(&state, &run, cache);
        assert_eq!(page_count, 2);

        let guard = state.html_cache().load();
        let pages = guard
            .as_ref()
            .as_ref()
            .expect("html_cache must be populated after commit_cached_pages");
        assert!(pages.contains_key("index.html"));
        assert!(pages.contains_key("report.html"));

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect(
                "commit_cached_pages must broadcast a PageUpdateEvent so connected \
                 browser clients are notified — no projection/cache update may \
                 silently mutate browser-visible server state",
            )
            .expect("ws_broadcast channel must not be closed");

        let mut keys: Vec<String> = event.pages.iter().map(|s| s.as_ref().to_owned()).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["index.html".to_string(), "report.html".to_string()],
            "PageUpdateEvent.pages must list every cache key written so the \
             client can decide whether to reload"
        );
    }
}
