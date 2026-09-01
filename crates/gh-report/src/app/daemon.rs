//! The daemon: scheduled collection + serving.
//!
//! Runs the web server while collecting on a fixed interval; the only
//! operational mode. Source-of-truth, snapshot fast-path: CHE-0048.
//!
//! ## Startup order
//!
//! 1. **Port bind** — duplicate-instance guard, before store handles,
//!    projection replay, warm-start, run-lock, credentials.
//! 2. **Projection init** — `snapshot_fast_path_init` (CHE-0048).
//! 3. **Warm-start** — render cache from the projection; 503 until first
//!    sweep if empty.
//! 4. **Web server starts** — warm-start data or 503.
//! 5. **Background collection** — scheduled runs, each success updates
//!    the cache atomically.
//! 6. **Worker pool** — lazy via `AppState::ensure_worker_pool()`,
//!    persists across runs.
//!
//! Shuts down gracefully on `Ctrl-C` / `SIGTERM`.
//!
//! **`--force-unlock`** / **`--force-refresh`** are one-shot: apply only to
//! the initial run (skip run-lock / bypass baseline reuse). Later runs
//! behave normally.

use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::app::collect;
use crate::app::state::{AppState, log_error_chain, read_rss_kb};
use crate::app::work_queue::JobSource;
use crate::app::worker_pool::JobOutcome;
use crate::app::write_policy::{WriteFailure, source_chain, write_with_policy_sync};
use crate::config;
use crate::config::runtime::RuntimeConfig;
use crate::domain::evidence::RepositoryEvidence;
use crate::error::{AppError, ConfigError, PersistenceError, persist_error_variant};
use tokio::net::TcpListener;
use tracing::{error, info, warn};

/// Shared cooperative drain budget for worker-pool, delivery-task, and
/// scheduled collection task shutdown. All drain phases start together
/// after cancellation is signalled; the total daemon-side drain budget is
/// this value rather than the sum of per-phase budgets.
const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(3);
const PHASE_READY: &str = "ready";
const PHASE_SHUTDOWN_BEGIN: &str = "shutdown_begin";
const PHASE_DRAIN_POOL: &str = "drain_pool";
const PHASE_DRAIN_DELIVERY: &str = "drain_delivery";
const PHASE_DRAIN_COLLECTION: &str = "drain_collection";
const PHASE_STOPPED: &str = "stopped";
const MESSAGE_READY: &str = "daemon ready — serving";
const MESSAGE_SHUTDOWN_BEGIN: &str = "beginning graceful shutdown";
const MESSAGE_STOPPED: &str = "daemon stopped";

fn duration_millis(duration: Duration) -> u128 {
    duration.as_millis()
}

/// Outcome of waiting for the next scheduled collection tick.
#[derive(Debug)]
enum NextTick {
    Run,
    Cancel,
}

/// A boolean flag that applies once: armed at construction, then cleared by
/// the first [`consume`](Self::consume) call. Backs `--force-unlock` and
/// `--force-refresh`, both of which apply only to the daemon's initial
/// collection run (see module docs).
struct OneShotFlag(AtomicBool);

impl OneShotFlag {
    /// Construct a flag in the given initial (armed/disarmed) state.
    fn new(armed: bool) -> Self {
        Self(AtomicBool::new(armed))
    }

    /// Return whether the flag was armed, clearing it in the same
    /// read-modify-write step so a concurrent observer never double-consumes.
    fn consume(&self) -> bool {
        self.0.fetch_and(false, Ordering::AcqRel)
    }

    /// Return whether the flag is currently armed, without clearing it.
    fn peek(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Wait for either the scheduled interval to elapse or a cancellation
/// signal, whichever comes first. The watch channel makes cancellation
/// sticky: a signal sent before the next call is observed immediately,
/// and a signal arriving during the sleep wins the `select!` (biased
/// branch). This guarantees no further `collect::run` is started after
/// shutdown is requested.
async fn next_collection_tick(
    cancel: &mut tokio::sync::watch::Receiver<bool>,
    interval: Duration,
) -> NextTick {
    if *cancel.borrow() {
        return NextTick::Cancel;
    }
    tokio::select! {
        biased;
        _ = cancel.changed() => NextTick::Cancel,
        () = tokio::time::sleep(interval) => {
            if *cancel.borrow() { NextTick::Cancel } else { NextTick::Run }
        }
    }
}

/// Start the daemon (warm-start + web server + background collection).
///
/// 1. Reads the `PORT` env var (default 8080).
/// 2. Attempts warm-start from baseline (fast, no API calls).
/// 3. Starts the web server on `{bind_address}:{port}`.
/// 4. Spawns a background task for the initial collection + scheduled loop.
/// 5. Shuts down gracefully on `Ctrl-C` or `SIGTERM`.
///
/// # Errors
///
/// Returns `AppError` if the server cannot start or the PORT env var is
/// invalid. Initial collection failures are logged but do not prevent
/// the server from continuing (retried on the next scheduled interval).
///
/// # Panics
///
/// Panics if the default `ServerConfig` cannot be built (indicates a
/// programming error in the hardcoded defaults).
#[expect(
    clippy::too_many_lines,
    reason = "daemon startup order is the operator-visible contract"
)]
pub async fn run(config: RuntimeConfig) -> Result<(), AppError> {
    let startup_started = Instant::now();
    let port = resolve_port()?;
    let bind_address = resolve_bind_address()?;
    let addr = parse_serving_addr(&bind_address, port).map_err(|e| server_error_runtime(&e))?;

    info!(
        org = %config.org_name,
        bind = %bind_address,
        port,
        interval_secs = config::COLLECTION_INTERVAL_SECS,
        "daemon starting"
    );

    let listener = bind_serving_port_before_next_step(addr, || ())
        .await
        .map_err(|e| server_error_runtime(&e))?;

    let events_dir = config.store_dir.join("events").join(&config.org_name);
    let nats = config.nats_store_config()?;
    let app_state = AppState::with_stores(&events_dir, config.pardosa_backend, nats)
        .await
        .map_err(|source| {
            log_error_chain("gh_report_open_event_store_failed", &source);
            AppError::Persistence(PersistenceError::LoadFailed {
                reason: format!("open event store at {}: {source}", events_dir.display()),
            })
        })?;

    if let Err(e) = app_state.snapshot_fast_path_init() {
        error!(error = %e, "projection runtime init failed");
        return Err(AppError::Persistence(PersistenceError::LoadFailed {
            reason: format!("projection runtime init failed: {e}"),
        }));
    }

    collect::warm_start_from_baseline(&config, &app_state).await;
    let rehydrated_records = app_state.projection_len();

    let shutdown_signal = Arc::new(Mutex::new(None));
    let shutdown_signal_slot = Arc::clone(&shutdown_signal);
    let shutdown = async move {
        let signal = crate::infra::signal::wait_for_shutdown_signal().await;
        *shutdown_signal_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(signal);
        info!(signal = signal.as_str(), "shutdown signal received");
    };

    let force_flag = Arc::new(OneShotFlag::new(config.force_unlock));
    let force_refresh_flag = Arc::new(OneShotFlag::new(config.force_refresh));
    let (collect_cancel_tx, collect_cancel_rx) = tokio::sync::watch::channel(false);

    let mut extra_routes = crate::server::status_router();
    if app_state.webhook_secret().is_some() {
        info!("webhooks enabled (WEBHOOK_SECRET set)");
        extra_routes = extra_routes.merge(crate::webhook::webhook_router());
    } else {
        info!("webhooks disabled (WEBHOOK_SECRET not set)");
    }
    info!(
        phase = PHASE_READY,
        bind = %bind_address,
        port,
        backend = ?config.pardosa_backend,
        rehydrated_records,
        startup_ms = duration_millis(startup_started.elapsed()),
        MESSAGE_READY,
    );

    let mut collection_loop = spawn_collection_loop(
        config.clone(),
        Arc::clone(&app_state),
        Arc::clone(&force_flag),
        Arc::clone(&force_refresh_flag),
        collect_cancel_rx.clone(),
    );
    spawn_team_refresh_loop(&config, Arc::clone(&app_state), collect_cancel_rx);
    let server_config = crate::server::served_dashboard_server_config();

    let server_result = cherry_pit_web::serve::start(
        port,
        &bind_address,
        Some(listener),
        shutdown,
        Arc::clone(&app_state),
        crate::server::server_layer_limits(),
        crate::server::server_ws_policy(),
        &server_config,
        None,
        Some(extra_routes),
    )
    .await;

    let shutdown_started = Instant::now();
    let observed_shutdown_signal = shutdown_signal
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    let signal = observed_shutdown_signal
        .unwrap_or(crate::infra::signal::ShutdownSignal::Interrupt)
        .as_str();
    info!(
        phase = PHASE_SHUTDOWN_BEGIN,
        signal,
        budget_ms = duration_millis(SHUTDOWN_DRAIN_TIMEOUT),
        MESSAGE_SHUTDOWN_BEGIN,
    );

    drain_shutdown(&app_state, &collect_cancel_tx, &mut collection_loop).await;

    server_result.map_err(|e| crate::error::ServerError::Runtime(e.to_string()))?;

    info!(
        phase = PHASE_STOPPED,
        elapsed_ms = duration_millis(shutdown_started.elapsed()),
        MESSAGE_STOPPED,
    );
    Ok(())
}

/// Drain all daemon-side background tasks on shutdown.
///
/// Cancels the worker-pool token, closes the work queue, and signals the
/// collection loop before starting the shared drain budget. Worker-pool,
/// delivery-task, and collection-loop handles are then awaited concurrently;
/// handles still pending at the budget boundary are aborted.
async fn drain_shutdown(
    app_state: &Arc<AppState>,
    cancel: &tokio::sync::watch::Sender<bool>,
    collection_loop: &mut tokio::task::JoinHandle<()>,
) {
    drain_shutdown_with_timeout(app_state, cancel, collection_loop, SHUTDOWN_DRAIN_TIMEOUT).await;
}

async fn drain_shutdown_with_timeout(
    app_state: &Arc<AppState>,
    cancel: &tokio::sync::watch::Sender<bool>,
    collection_loop: &mut tokio::task::JoinHandle<()>,
    timeout: Duration,
) {
    app_state.cancel_worker_pool();
    app_state.work_queue.close();
    let _ = cancel.send(true);
    let worker_drain = app_state.drain_worker_pool(timeout);
    let collection_drain =
        drain_collection_loop_after_cancel_with_timeout(collection_loop, timeout);
    let ((pool_drained, delivery_drained), collection_drained) =
        tokio::join!(worker_drain, collection_drain);
    if pool_drained {
        info!(
            phase = PHASE_DRAIN_POOL,
            reason = "drained",
            "worker pool drained cooperatively"
        );
    } else {
        warn!(
            phase = PHASE_DRAIN_POOL,
            reason = "timeout",
            budget_ms = duration_millis(timeout),
            "aborting in-flight worker jobs — drain budget exceeded"
        );
    }
    if delivery_drained {
        info!(
            phase = PHASE_DRAIN_DELIVERY,
            reason = "drained",
            "delivery task drained cooperatively"
        );
    } else {
        warn!(
            phase = PHASE_DRAIN_DELIVERY,
            reason = "timeout",
            budget_ms = duration_millis(timeout),
            "aborting in-flight delivery work — drain budget exceeded"
        );
    }
    match collection_drained {
        Ok(()) => info!(
            phase = PHASE_DRAIN_COLLECTION,
            reason = "drained",
            "collection task drained cooperatively"
        ),
        Err(CollectionDrainError::Join(join_err)) => warn!(
            phase = PHASE_DRAIN_COLLECTION,
            reason = "join_error",
            error = %join_err,
            "collection task ended abnormally during drain",
        ),
        Err(CollectionDrainError::Timeout) => warn!(
            phase = PHASE_DRAIN_COLLECTION,
            reason = "timeout",
            budget_ms = duration_millis(timeout),
            "aborting in-flight collection work — persist or publish outcome is unknown; EventStore boot replay will reconcile on next startup",
        ),
    }
}

/// A durable-write failure proven to be
/// [`WritePolicyCategory::Conflict`] — the OCC fence rejecting a
/// superseded writer (PGN-0016:R2).
///
/// Correct by construction: the only constructor is
/// [`FenceSignal::from_failure`], which rejects every other category, so
/// a non-fence failure cannot be carried as a fence signal and cannot
/// abort a run that should merely have been logged. The typed
/// [`PersistenceError`] is carried whole — never string-flattened
/// (PGN-0016:R9) — so the run boundary can re-raise it as
/// [`AppError::Persistence`] and converge through the single sanctioned
/// [`converge_on_fence`] sink (CHE-0088:R9) rather than re-arming at the
/// detection site.
#[derive(Debug)]
pub(crate) struct FenceSignal {
    error: PersistenceError,
}

impl FenceSignal {
    pub(crate) fn from_failure(
        failure: crate::app::write_policy::WriteFailure,
    ) -> Result<Self, crate::app::write_policy::WriteFailure> {
        if failure.category == crate::app::write_policy::WritePolicyCategory::Conflict {
            Ok(Self {
                error: failure.error,
            })
        } else {
            Err(failure)
        }
    }

    pub(crate) fn into_error(self) -> PersistenceError {
        self.error
    }
}

/// The outcome of delivering one job outcome to durable storage.
///
/// `Fenced` is the only variant that aborts the run; every other
/// persist failure stays `Delivered` and keeps its existing
/// severity-based logging and batch accounting, so propagation is not
/// widened from the fence to all durable-write failures.
enum DeliveryStep {
    Delivered,
    Fenced(FenceSignal),
}

/// The durable-write side of the delivery task, as the delivery task
/// sees it: one repository record in, a classified [`WriteFailure`] out.
///
/// Production is [`AppState`] itself, wrapping the inherent
/// `record_repo` in the single `write_with_policy_sync` chokepoint
/// (CHE-0088:R2). The seam exists so a fenced repository persist can be
/// arranged at the real delivery/collection boundary, where the OCC
/// fence itself needs a second live writer to raise.
pub(crate) trait RepoRecorder: Send + Sync + 'static {
    fn record_repo(
        &self,
        domain_key: &str,
        evidence: RepositoryEvidence,
        repo_name: &str,
        timestamp: &str,
    ) -> Result<(), WriteFailure>;
}

impl RepoRecorder for AppState {
    fn record_repo(
        &self,
        domain_key: &str,
        evidence: RepositoryEvidence,
        repo_name: &str,
        timestamp: &str,
    ) -> Result<(), WriteFailure> {
        write_with_policy_sync(|| {
            AppState::record_repo(self, domain_key, evidence.clone(), repo_name, timestamp)
        })
    }
}

/// Bounded re-arm policy applied after `CollectionOutcome::FencedConflict`.
///
/// Design-Y consumer-owned re-arm (ghr-fea8b799): on a typed fence
/// conflict the daemon forces a fresh authoritative read (re-seeding the
/// long-lived store handle via [`AppState::resync_event_store`]) before
/// retrying the run, instead of the prior non-converging warn-and-wait
/// loop that resent the same stale cached sequence every tick forever.
/// Bounded by `max_attempts`; exhausting the cap surfaces a typed
/// [`RearmError`] rather than looping forever.
struct RearmPolicy {
    max_attempts: u32,
    backoff_base: Duration,
}

impl RearmPolicy {
    const DEFAULT: Self = Self {
        max_attempts: 3,
        backoff_base: Duration::from_secs(2),
    };
}

/// Terminal give-up surface for [`converge_on_fence`] / [`rearm_after_fenced_conflict`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
enum RearmError<E: std::error::Error + 'static> {
    #[error("fence-conflict re-arm exhausted after {attempts} attempt(s): resync failed: {source}")]
    ResyncFailed {
        attempts: u32,
        #[source]
        source: std::io::Error,
    },
    #[error("fence-conflict re-arm exhausted after {attempts} attempt(s): still fenced")]
    StillFenced { attempts: u32 },
    #[error("fence-conflict re-arm exhausted after {attempts} attempt(s): run failed: {source}")]
    RunFailed {
        attempts: u32,
        #[source]
        source: E,
    },
}

/// One resync+run attempt's classification, fed to [`converge_on_fence`]
/// by each sanctioned-sink caller (collection loop, team-refresh loop).
enum ConvergeStep<T, E> {
    Converged(T),
    Fenced,
    Failed(E),
}

/// The single sanctioned resync+bounded-retry converge sink
/// (ghr-c905de05, CHE-0088 amendment): force a fresh authoritative read
/// via `resync`, then retry via `run`, bounded by `policy`. Both
/// gh-report durable-write loops (collection, team-refresh) route
/// through this combinator rather than hand-rolling their own converge
/// dance per call site.
///
/// Does NOT patch a cached sequence and redrive the same append
/// (R10-forbidden) — `resync` re-owns/re-reads a fresh authoritative view
/// before each retry, so a superseded writer identity cannot win an
/// append against stale state.
async fn converge_on_fence<Resync, ResyncFut, Run, RunFut, T, E>(
    policy: &RearmPolicy,
    mut resync: Resync,
    mut run: Run,
) -> Result<T, RearmError<E>>
where
    Resync: FnMut() -> ResyncFut,
    ResyncFut: Future<Output = Result<(), std::io::Error>>,
    Run: FnMut() -> RunFut,
    RunFut: Future<Output = ConvergeStep<T, E>>,
    E: std::error::Error + 'static,
{
    for attempt in 1..=policy.max_attempts {
        if let Err(source) = resync().await {
            if attempt == policy.max_attempts {
                return Err(RearmError::ResyncFailed {
                    attempts: attempt,
                    source,
                });
            }
            tokio::time::sleep(policy.backoff_base * attempt).await;
            continue;
        }
        match run().await {
            ConvergeStep::Converged(value) => return Ok(value),
            ConvergeStep::Fenced => {
                if attempt == policy.max_attempts {
                    return Err(RearmError::StillFenced { attempts: attempt });
                }
                tokio::time::sleep(policy.backoff_base * attempt).await;
            }
            ConvergeStep::Failed(source) => {
                if attempt == policy.max_attempts {
                    return Err(RearmError::RunFailed {
                        attempts: attempt,
                        source,
                    });
                }
                tokio::time::sleep(policy.backoff_base * attempt).await;
            }
        }
    }
    Err(RearmError::StillFenced {
        attempts: policy.max_attempts,
    })
}

/// Re-arm a fenced collection run: force a fresh authoritative read via
/// `resync`, then retry via `run`, bounded by `policy`. Thin
/// [`collect::CollectionOutcome`]-shaped wrapper over the shared
/// [`converge_on_fence`] sink.
async fn rearm_after_fenced_conflict<Resync, ResyncFut, Run, RunFut>(
    policy: &RearmPolicy,
    resync: Resync,
    mut run: Run,
) -> Result<collect::CollectionOutcome, RearmError<AppError>>
where
    Resync: FnMut() -> ResyncFut,
    ResyncFut: Future<Output = Result<(), std::io::Error>>,
    Run: FnMut() -> RunFut,
    RunFut: Future<Output = Result<collect::CollectionOutcome, AppError>>,
{
    converge_on_fence(policy, resync, move || {
        let fut = Box::pin(run());
        async move {
            match fut.await {
                Ok(
                    outcome @ (collect::CollectionOutcome::Completed
                    | collect::CollectionOutcome::Cancelled),
                ) => ConvergeStep::Converged(outcome),
                Ok(collect::CollectionOutcome::FencedConflict) => ConvergeStep::Fenced,
                Err(source) => ConvergeStep::Failed(source),
            }
        }
    })
    .await
}

/// Drive [`rearm_after_fenced_conflict`] for one fenced collection tick,
/// logging the terminal outcome. `run_cfg` builds a fresh
/// [`RuntimeConfig`] for each retry (mirrors the caller's
/// `initial_run_config` / `scheduled_run_config` choice).
async fn rearm_fenced_run(
    events_dir: &Path,
    backend: crate::config::runtime::PardosaBackend,
    nats: Result<&crate::config::runtime::NatsStoreConfig, &ConfigError>,
    state: &Arc<AppState>,
    mut run_cfg: impl FnMut() -> RuntimeConfig,
) {
    let Ok(nats) = nats else {
        error!(
            owner_id = %state.owner_id,
            "fence-conflict re-arm skipped: NATS store config invalid — falling back to next scheduled tick"
        );
        return;
    };
    let outcome = rearm_after_fenced_conflict(
        &RearmPolicy::DEFAULT,
        || state.resync_event_store(events_dir, backend, nats.clone()),
        || collect::run_with_outcome(run_cfg(), Arc::clone(state)),
    )
    .await;
    match outcome {
        Ok(collect::CollectionOutcome::Completed) => {
            info!(owner_id = %state.owner_id, "fence-conflict re-arm converged");
        }
        Ok(collect::CollectionOutcome::Cancelled) => {
            info!(owner_id = %state.owner_id, "fence-conflict re-arm aborted on shutdown");
        }
        Ok(collect::CollectionOutcome::FencedConflict) => {
            unreachable!("rearm_after_fenced_conflict never returns Ok(FencedConflict)")
        }
        Err(error) => {
            error!(
                owner_id = %state.owner_id,
                error = %error,
                "fence-conflict re-arm exhausted — reverting to next scheduled tick"
            );
        }
    }
}

/// Zero-sized give-up marker for [`rearm_after_fenced_team_refresh`]'s
/// non-fence run failures. The real [`team_refresh::TickFailure`]
/// (error and logging context) is returned alongside the
/// [`RearmError`] outcome rather than carried as its generic source,
/// since `TickFailure` is not itself a `std::error::Error` impl and the
/// caller needs the failure for exactly one terminal
/// [`team_refresh::log_tick_failure`] call, not for error-chain
/// rendering.
#[derive(Debug, thiserror::Error)]
#[error("team-refresh tick failed (see logged context)")]
struct TeamRefreshFailureLogged;

/// Converge a team-refresh tick after a `FencedConflict`: force a fresh
/// authoritative read via `resync`, then retry via `run`, bounded by
/// `policy` — the team-refresh analogue of
/// [`rearm_after_fenced_conflict`], routed through the same shared
/// [`converge_on_fence`] sink (ghr-c905de05) rather than hand-rolling a
/// second converge dance. Returns the terminal [`RearmError`] outcome
/// alongside the last observed [`team_refresh::TickFailure`] (if any run
/// attempt failed), so the caller can log it exactly once via
/// [`team_refresh::log_tick_failure`] on give-up.
async fn rearm_after_fenced_team_refresh<Resync, ResyncFut, Run, RunFut>(
    policy: &RearmPolicy,
    resync: Resync,
    mut run: Run,
) -> (
    Result<(), RearmError<TeamRefreshFailureLogged>>,
    Option<crate::app::team_refresh::TickFailure>,
)
where
    Resync: FnMut() -> ResyncFut,
    ResyncFut: Future<Output = Result<(), std::io::Error>>,
    Run: FnMut() -> RunFut,
    RunFut: Future<Output = Result<(), crate::app::team_refresh::TickFailure>>,
{
    let last_failure = Arc::new(Mutex::new(None));
    let last_failure_ref = Arc::clone(&last_failure);
    let outcome = converge_on_fence(policy, resync, move || {
        let fut = Box::pin(run());
        let last_failure_ref = Arc::clone(&last_failure_ref);
        async move {
            match fut.await {
                Ok(()) => ConvergeStep::Converged(()),
                Err(failure) => {
                    let is_fenced = matches!(
                        failure.error,
                        AppError::Persistence(PersistenceError::FencedConflict { .. })
                    );
                    *last_failure_ref
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(failure);
                    if is_fenced {
                        ConvergeStep::Fenced
                    } else {
                        ConvergeStep::Failed(TeamRefreshFailureLogged)
                    }
                }
            }
        }
    })
    .await;
    let last_failure = Arc::try_unwrap(last_failure).map_or(None, |mutex| {
        mutex
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    });
    (outcome, last_failure)
}

/// Drive [`rearm_after_fenced_team_refresh`] for one fenced team-refresh
/// tick, logging the terminal outcome. Mirrors [`rearm_fenced_run`]'s
/// shape: on give-up, logs the last observed failure's typed context via
/// [`team_refresh::log_tick_failure`] exactly once (not on every
/// intermediate retry), then a terminal `error!` matching
/// `rearm_fenced_run`'s own give-up log.
async fn rearm_fenced_team_refresh_tick(
    events_dir: &Path,
    backend: crate::config::runtime::PardosaBackend,
    nats: Result<&crate::config::runtime::NatsStoreConfig, &ConfigError>,
    state: &Arc<AppState>,
    client: &crate::github::client::GitHubClient,
    fetched_at: &str,
) {
    let Ok(nats) = nats else {
        error!(
            owner_id = %state.owner_id,
            "team-refresh fence-conflict re-arm skipped: NATS store config invalid — falling back to next scheduled tick"
        );
        return;
    };
    let (outcome, last_failure) = rearm_after_fenced_team_refresh(
        &RearmPolicy::DEFAULT,
        || state.resync_event_store(events_dir, backend, nats.clone()),
        || crate::app::team_refresh::run_team_refresh_tick(state, client, fetched_at),
    )
    .await;
    match outcome {
        Ok(()) => {
            info!(owner_id = %state.owner_id, "team-refresh fence-conflict re-arm converged");
        }
        Err(error) => {
            if let Some(failure) = last_failure {
                crate::app::team_refresh::log_tick_failure(&failure.error, &failure.context);
            }
            error!(
                owner_id = %state.owner_id,
                error = %error,
                "team-refresh fence-conflict re-arm exhausted — reverting to next scheduled tick"
            );
        }
    }
}

/// Spawn the background collection task: one initial run with the
/// caller-supplied `force_unlock` flag, then a scheduled loop that
/// honours a cooperative cancellation signal between iterations. The
/// loop never cancels an in-flight `collect::run`; persist→publish
/// runs to completion before the next tick is considered.
fn spawn_collection_loop(
    config: RuntimeConfig,
    state: Arc<AppState>,
    force_flag: Arc<OneShotFlag>,
    force_refresh_flag: Arc<OneShotFlag>,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    let events_dir = config.store_dir.join("events").join(&config.org_name);
    let nats = config.nats_store_config();
    tokio::spawn(async move {
        {
            let cfg = initial_run_config(&config, &force_flag, &force_refresh_flag);
            match collect::run_with_outcome(cfg, Arc::clone(&state)).await {
                Ok(collect::CollectionOutcome::Completed) => info!("initial collection complete"),
                Ok(collect::CollectionOutcome::Cancelled) => {
                    info!("initial collection aborted on shutdown — no report published");
                }
                Ok(collect::CollectionOutcome::FencedConflict) => {
                    warn!(
                        owner_id = %state.owner_id,
                        expected = "rollover",
                        "initial collection fenced by active single-writer guard — expected Cloud-Run-rollover OCC churn (PGN-0016:R7); re-arming with fresh authoritative read"
                    );
                    rearm_fenced_run(
                        &events_dir,
                        config.pardosa_backend,
                        nats.as_ref(),
                        &state,
                        || initial_run_config(&config, &force_flag, &force_refresh_flag),
                    )
                    .await;
                }
                Err(AppError::Persistence(error @ PersistenceError::LockFailed { .. })) => {
                    let failure = WriteFailure::classify(error);
                    error!(
                        persist_error_variant = persist_error_variant(&failure.error),
                        category = ?failure.category,
                        response = ?failure.response,
                        owner_id = %state.owner_id,
                        source_chain = source_chain(&failure.error).as_str(),
                        "initial collection skipped: lock held"
                    );
                }
                Err(e) => log_initial_collection_failure(&e),
            }
        }

        loop {
            match next_collection_tick(
                &mut cancel,
                Duration::from_secs(crate::config::COLLECTION_INTERVAL_SECS),
            )
            .await
            {
                NextTick::Cancel => {
                    info!("collection loop cancelled — exiting");
                    return;
                }
                NextTick::Run => {}
            }
            let cfg = scheduled_run_config(&config, &force_flag, &force_refresh_flag);
            match collect::run_with_outcome(cfg, Arc::clone(&state)).await {
                Ok(collect::CollectionOutcome::Completed) => {
                    info!(
                        rss_kb = ?read_rss_kb(),
                        projection_repo_count = state.projection_len(),
                        projection_bytes_deep = ?state.projection_bytes_deep(),
                        "scheduled collection complete"
                    );
                }
                Ok(collect::CollectionOutcome::Cancelled) => {
                    info!("scheduled collection aborted on shutdown — no report published");
                }
                Ok(collect::CollectionOutcome::FencedConflict) => {
                    warn!(
                        owner_id = %state.owner_id,
                        expected = "rollover",
                        "scheduled collection fenced by active single-writer guard — expected Cloud-Run-rollover OCC churn (PGN-0016:R7); re-arming with fresh authoritative read"
                    );
                    rearm_fenced_run(
                        &events_dir,
                        config.pardosa_backend,
                        nats.as_ref(),
                        &state,
                        || scheduled_run_config(&config, &force_flag, &force_refresh_flag),
                    )
                    .await;
                }
                Err(AppError::Persistence(error @ PersistenceError::LockFailed { .. })) => {
                    let failure = WriteFailure::classify(error);
                    warn!(
                        persist_error_variant = persist_error_variant(&failure.error),
                        category = ?failure.category,
                        response = ?failure.response,
                        owner_id = %state.owner_id,
                        source_chain = source_chain(&failure.error).as_str(),
                        "collection skipped: lock held"
                    );
                }
                Err(e) => error!(error = %e, "scheduled collection failed"),
            }
        }
    })
}

fn log_initial_collection_failure(error: &AppError) {
    log_error_chain("gh_report_initial_collection_failed", error);
    error!(error = %error, "initial collection failed — will retry");
}

/// Spawn the team-refresh collector loop: a periodic tick, decoupled
/// from [`spawn_collection_loop`]'s repo collect-cycle timer, that
/// persists `TeamStateCaptured` events on its own cadence
/// ([`crate::config::TEAM_REFRESH_INTERVAL_SECS`]). This severs the
/// repo-snapshot↔roster-fetch coupling that was the raciness root
/// (ghr-3fda2878, roadmap ghr-b562fe02 §E Phase 3).
///
/// Reuses the same cooperative cancellation signal as the collection
/// loop; a tick in flight is not interrupted (matching the collection
/// loop's own drain semantics — see module docs), but the wait between
/// ticks observes cancellation immediately.
///
/// Ticks wait for the GitHub client rather than skipping when it is
/// absent: it is created lazily on the first repo collection, so a tick
/// racing that initialisation must block on it. Skipping instead would
/// forfeit a full [`crate::config::TEAM_REFRESH_INTERVAL_SECS`] of
/// roster data — 24 hours after every Cloud Run revision.
///
/// A refresh runs at STARTUP, before the first interval wait, so a
/// freshly-started revision reaches a populated roster in seconds
/// rather than a day. The wait is deliberately on client availability
/// rather than on the initial collection's completion: taking a
/// completion signal from [`spawn_collection_loop`] would reintroduce
/// the repo-collect-cycle coupling CHE-0089:R5 severs.
fn spawn_team_refresh_loop(
    config: &RuntimeConfig,
    state: Arc<AppState>,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    let events_dir = config.store_dir.join("events").join(&config.org_name);
    let nats = config.nats_store_config();
    let backend = config.pardosa_backend;
    tokio::spawn(async move {
        let ClientReady::Client(client) = await_github_client(&state, &mut cancel).await else {
            info!("team-refresh loop cancelled while awaiting GitHub client — exiting");
            return;
        };
        info!("team-refresh startup tick running (does not wait a full interval)");
        run_one_team_refresh_tick(&state, &client, &events_dir, backend, nats.as_ref()).await;

        loop {
            match next_collection_tick(
                &mut cancel,
                Duration::from_secs(crate::config::TEAM_REFRESH_INTERVAL_SECS),
            )
            .await
            {
                NextTick::Cancel => {
                    info!("team-refresh loop cancelled — exiting");
                    return;
                }
                NextTick::Run => {}
            }
            let ClientReady::Client(client) = await_github_client(&state, &mut cancel).await else {
                info!("team-refresh loop cancelled while awaiting GitHub client — exiting");
                return;
            };
            run_one_team_refresh_tick(&state, &client, &events_dir, backend, nats.as_ref()).await;
        }
    })
}

/// Outcome of waiting for the lazily-initialised GitHub client.
///
/// Deliberately two variants with no "not ready, skip anyway" arm: a
/// skipped tick is indistinguishable from a lost refresh period, which
/// at a 24h cadence is the regression this wait exists to prevent. The
/// only way out without a client is cancellation.
enum ClientReady {
    Client(Arc<crate::github::client::GitHubClient>),
    Cancelled,
}

/// Wait until the GitHub client exists, or until shutdown is requested.
///
/// Returns immediately when the client is already initialised (the
/// steady-state case), so the poll cost is paid only during the startup
/// race against the first repo collection.
async fn await_github_client(
    state: &Arc<AppState>,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
) -> ClientReady {
    let poll = Duration::from_secs(crate::config::TEAM_REFRESH_CLIENT_POLL_SECS);
    loop {
        if *cancel.borrow() {
            return ClientReady::Cancelled;
        }
        if let Some(client) = state.github_client() {
            return ClientReady::Client(client);
        }
        tokio::select! {
            biased;
            _ = cancel.changed() => return ClientReady::Cancelled,
            () = tokio::time::sleep(poll) => {}
        }
    }
}

/// Run one team-refresh tick, re-arming once on a single-writer fence
/// conflict and warn-logging any other failure without propagating it.
async fn run_one_team_refresh_tick(
    state: &Arc<AppState>,
    client: &crate::github::client::GitHubClient,
    events_dir: &std::path::Path,
    backend: crate::config::runtime::PardosaBackend,
    nats: Result<&crate::config::runtime::NatsStoreConfig, &ConfigError>,
) {
    let fetched_at = jiff::Timestamp::now().to_string();
    if let Err(failure) =
        crate::app::team_refresh::run_team_refresh_tick(state, client, &fetched_at).await
    {
        if matches!(
            failure.error,
            AppError::Persistence(PersistenceError::FencedConflict { .. })
        ) {
            warn!(
                owner_id = %state.owner_id,
                expected = "rollover",
                "team-refresh tick fenced by active single-writer guard — expected Cloud-Run-rollover OCC churn (PGN-0016:R7); re-arming with fresh authoritative read"
            );
            rearm_fenced_team_refresh_tick(events_dir, backend, nats, state, client, &fetched_at)
                .await;
        } else {
            crate::app::team_refresh::log_tick_failure(&failure.error, &failure.context);
        }
    }
}

/// Resolve the config for the daemon's initial collection run: consumes
/// both one-shot force flags, so subsequent scheduled runs see them cleared.
fn initial_run_config(
    config: &RuntimeConfig,
    force_flag: &OneShotFlag,
    force_refresh_flag: &OneShotFlag,
) -> RuntimeConfig {
    let mut cfg = config.clone();
    cfg.force_unlock = force_flag.consume();
    cfg.force_refresh = force_refresh_flag.consume();
    cfg
}

/// Resolve the config for a scheduled (non-initial) collection run: reads
/// the one-shot force flags without clearing them — they were already
/// consumed by the initial run and stay disarmed thereafter.
fn scheduled_run_config(
    config: &RuntimeConfig,
    force_flag: &OneShotFlag,
    force_refresh_flag: &OneShotFlag,
) -> RuntimeConfig {
    let mut cfg = config.clone();
    cfg.force_unlock = force_flag.peek();
    cfg.force_refresh = force_refresh_flag.peek();
    cfg
}

async fn bind_serving_port_before_next_step<F>(
    addr: SocketAddr,
    next_step: F,
) -> Result<TcpListener, cherry_pit_web::serve::ServerError>
where
    F: FnOnce(),
{
    let listener = cherry_pit_web::serve::bind_serving_port(addr).await?;
    next_step();
    Ok(listener)
}

enum CollectionDrainError {
    Join(tokio::task::JoinError),
    Timeout,
}

async fn drain_collection_loop_after_cancel_with_timeout(
    handle: &mut tokio::task::JoinHandle<()>,
    timeout: Duration,
) -> Result<(), CollectionDrainError> {
    match tokio::time::timeout(timeout, &mut *handle).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(join_err)) => Err(CollectionDrainError::Join(join_err)),
        Err(_) => {
            handle.abort();
            let _ = (&mut *handle).await;
            Err(CollectionDrainError::Timeout)
        }
    }
}

/// Delivery task: consumes job outcomes from the worker pool channel.
///
/// Responsibilities:
/// 1. Record `RepositoryStateCaptured` success events on each repo's fiber
///    via `AppState::record_repo`, then refold `EvidenceProjection` from the
///    written event stream (projection is a pure fold over `NativeStore`).
/// 2. Record failure events carrying synthesised `Unknown`-status evidence
///    (so the dashboard shows error state rather than stale passing data).
/// 3. `batch_tracker.complete_one()` for `ScheduledBatch` outcomes
///    (countdown so the sweep knows when all jobs are done)
///
/// Log lines include repo-name enrichment: the repository name is extracted
/// from evidence so operators can identify repos without looking up numeric IDs.
///
/// Phase E adds: incremental rendering, WS broadcast.
pub(crate) async fn delivery_loop(
    rx: tokio::sync::mpsc::Receiver<JobOutcome<RepositoryEvidence>>,
    state: Arc<AppState>,
) {
    let recorder = Arc::clone(&state);
    delivery_loop_with_recorder(rx, state, recorder).await;
}

/// [`delivery_loop`] over an explicit durable-write side.
pub(crate) async fn delivery_loop_with_recorder<R: RepoRecorder>(
    mut rx: tokio::sync::mpsc::Receiver<JobOutcome<RepositoryEvidence>>,
    state: Arc<AppState>,
    recorder: Arc<R>,
) {
    while let Some(outcome) = rx.recv().await {
        let (source, duration) = match &outcome {
            JobOutcome::Success {
                source, duration, ..
            }
            | JobOutcome::Failure {
                source, duration, ..
            } => (source.clone(), *duration),
            _ => {
                warn!("delivery_loop: unhandled JobOutcome variant, skipping");
                continue;
            }
        };

        if state.run_is_fenced() {
            warn!(
                source = ?source,
                "discarding delivery outcome: run aborted by the single-writer fence — a \
                 superseded writer must not append (PGN-0016:R2); the converged run re-collects it"
            );
            continue;
        }

        match outcome {
            JobOutcome::Success {
                domain_key, result, ..
            } => {
                let step = handle_success_outcome(
                    recorder.as_ref(),
                    &domain_key,
                    &result,
                    &source,
                    duration,
                );
                apply_delivery_step(&state, step, &source);
            }
            JobOutcome::Failure {
                domain_key, error, ..
            } => {
                let step = handle_failure_outcome(
                    &state,
                    recorder.as_ref(),
                    &domain_key,
                    &error,
                    &source,
                    duration,
                );
                apply_delivery_step(&state, step, &source);
            }
            _ => {
                warn!("delivery_loop: unhandled JobOutcome variant, skipping");
            }
        }
    }
    info!("delivery task exiting — outcome channel closed");
}

/// Apply one delivery outcome to run-level accounting.
///
/// `Fenced` aborts the run through [`AppState::fence_active_run`] and
/// deliberately does NOT complete a batch slot for the fenced record: a
/// write the fence rejected did not happen, and the batch is abandoned
/// as a unit rather than counted down member by member. Every other
/// outcome — success or a non-fence persist failure — keeps the
/// pre-existing per-record countdown.
fn apply_delivery_step(state: &Arc<AppState>, step: DeliveryStep, source: &JobSource) {
    match step {
        DeliveryStep::Fenced(signal) => state.fence_active_run(signal),
        DeliveryStep::Delivered => {
            if matches!(source, JobSource::ScheduledBatch) {
                state.complete_active_batch();
            }
        }
    }
}

/// Publish a successful repo evaluation and log completion.
///
/// Extracted from [`delivery_loop`] for cohesion; no behavioural change.
fn handle_success_outcome<R: RepoRecorder + ?Sized>(
    recorder: &R,
    domain_key: &str,
    result: &RepositoryEvidence,
    source: &JobSource,
    duration: Duration,
) -> DeliveryStep {
    let repo_name = result.repository.name.clone();
    let timestamp = jiff::Timestamp::now().to_string();
    match recorder.record_repo(domain_key, result.clone(), &repo_name, &timestamp) {
        Ok(()) => {
            info!(
                key = %domain_key,
                repo = %repo_name,
                source = ?source,
                duration_ms = duration.as_millis(),
                "job completed"
            );
            DeliveryStep::Delivered
        }
        Err(failure) => classify_persist_failure(failure, domain_key, &repo_name, source, duration),
    }
}

/// Log a durable-write failure, then decide whether it aborts the run.
///
/// A `Conflict` is the OCC fence rejecting this writer: it must reach a
/// run boundary as a typed error (CHE-0088:R3 no-swallow) so the run
/// aborts (PGN-0016:R2). Every other category keeps its existing
/// log-and-continue handling — the propagation is scoped to the fence,
/// not widened to all persist failures.
fn classify_persist_failure(
    failure: WriteFailure,
    domain_key: &str,
    repo_name: &str,
    source: &JobSource,
    duration: Duration,
) -> DeliveryStep {
    log_job_persist_failure(&failure, domain_key, repo_name, source, duration);
    match FenceSignal::from_failure(failure) {
        Ok(signal) => DeliveryStep::Fenced(signal),
        Err(_) => DeliveryStep::Delivered,
    }
}

fn log_job_persist_failure(
    failure: &WriteFailure,
    domain_key: &str,
    repo_name: &str,
    source: &JobSource,
    duration: Duration,
) {
    let persist_error_variant = persist_error_variant(&failure.error);
    let category = failure.category;
    let response = failure.response;
    let source_chain = source_chain(&failure.error);
    if crate::app::write_policy::severity_for(category) == tracing::Level::WARN {
        warn!(
            key = %domain_key,
            repo = %repo_name,
            source = ?source,
            duration_ms = duration.as_millis(),
            persist_error_variant,
            category = ?category,
            response = ?response,
            source_chain = source_chain.as_str(),
            "job outcome downgraded to failed: durable record write did not succeed"
        );
    } else {
        error!(
            key = %domain_key,
            repo = %repo_name,
            source = ?source,
            duration_ms = duration.as_millis(),
            persist_error_variant,
            category = ?category,
            response = ?response,
            source_chain = source_chain.as_str(),
            "job outcome downgraded to failed: durable record write did not succeed"
        );
    }
}

/// Publish a failed repo evaluation and log the failure.
///
/// The synthesised failure-state record is a second repository write on
/// the same fiber, so it can be rejected by the same OCC fence: its
/// persist failure is classified through the same
/// [`classify_persist_failure`] path, and a `Conflict` aborts the run
/// rather than being logged and accounted as delivered (CHE-0088:R3,
/// PGN-0016:R2). Non-conflict categories keep their existing
/// log-and-continue handling.
fn handle_failure_outcome<R: RepoRecorder + ?Sized>(
    state: &Arc<AppState>,
    recorder: &R,
    domain_key: &str,
    error: &str,
    source: &JobSource,
    duration: Duration,
) -> DeliveryStep {
    let existing = state.projection_get(domain_key);
    let (repo_name, step) = if let Some(existing) = existing {
        let name = existing.repository.name.clone();
        let failure = collect::failure_evidence(
            &std::sync::Arc::new(existing.repository.clone()),
            &jiff::Timestamp::now().to_string(),
        );
        let timestamp = jiff::Timestamp::now().to_string();
        let step = match recorder.record_repo(domain_key, failure.clone(), &name, &timestamp) {
            Ok(()) => DeliveryStep::Delivered,
            Err(write_failure) => {
                classify_failure_state_persist_failure(write_failure, domain_key, &name)
            }
        };
        (name, step)
    } else {
        (domain_key.to_string(), DeliveryStep::Delivered)
    };
    error!(
        key = %domain_key,
        repo = %repo_name,
        source = ?source,
        error = %error,
        duration_ms = duration.as_millis(),
        "job failed"
    );
    step
}

/// Log a failed failure-state write, then decide whether it aborts the
/// run. Same fence gate as [`classify_persist_failure`]; only the log
/// line differs, because this write is the synthesised failure record
/// rather than the collected evidence.
fn classify_failure_state_persist_failure(
    write_failure: WriteFailure,
    domain_key: &str,
    repo_name: &str,
) -> DeliveryStep {
    tracing::error!(
        key = %domain_key,
        repo = %repo_name,
        persist_error_variant = persist_error_variant(&write_failure.error),
        category = ?write_failure.category,
        response = ?write_failure.response,
        source_chain = source_chain(&write_failure.error).as_str(),
        "repository failure state record failed"
    );
    match FenceSignal::from_failure(write_failure) {
        Ok(signal) => DeliveryStep::Fenced(signal),
        Err(_) => DeliveryStep::Delivered,
    }
}

/// Resolve the port number from the `PORT` env var, defaulting to 8080.
fn resolve_port() -> Result<u16, ConfigError> {
    resolve_port_with(|key| std::env::var(key).ok())
}

/// Resolve port from a configurable env-var lookup, defaulting to 8080.
fn resolve_port_with<F>(env_var: F) -> Result<u16, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match env_var("PORT") {
        Some(val) => val.parse::<u16>().map_err(|e| ConfigError::InvalidValue {
            field: "PORT".into(),
            reason: format!("invalid port: {e}"),
        }),
        None => Ok(8080),
    }
}

/// Resolve the bind address from the `BIND_ADDRESS` env var, defaulting to
/// [`config::DEFAULT_BIND_ADDRESS`] (`127.0.0.1`).
fn resolve_bind_address() -> Result<String, ConfigError> {
    resolve_bind_address_with(|key| std::env::var(key).ok())
}

/// Resolve bind address from a configurable env-var lookup.
///
/// Empty values are rejected — set `BIND_ADDRESS=0.0.0.0` explicitly for
/// container deployments that need all-interface binding.
fn resolve_bind_address_with<F>(env_var: F) -> Result<String, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match env_var("BIND_ADDRESS") {
        Some(val) => {
            let trimmed = val.trim();
            if trimmed.is_empty() {
                return Err(ConfigError::InvalidValue {
                    field: "BIND_ADDRESS".into(),
                    reason: "empty bind address; set to an IP like 127.0.0.1 or 0.0.0.0".into(),
                });
            }
            Ok(trimmed.to_string())
        }
        None => Ok(config::DEFAULT_BIND_ADDRESS.to_string()),
    }
}

fn parse_serving_addr(
    bind_address: &str,
    port: u16,
) -> Result<SocketAddr, cherry_pit_web::serve::ServerError> {
    let address = format!("{bind_address}:{port}");
    address
        .parse()
        .map_err(|source| cherry_pit_web::serve::ServerError::InvalidAddress { address, source })
}

fn server_error_runtime(error: &cherry_pit_web::serve::ServerError) -> crate::error::ServerError {
    crate::error::ServerError::Runtime(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::SERVED_CSP_WITH_WASM_UNSAFE_EVAL;
    use std::io::Write;
    use tracing_subscriber::fmt::MakeWriter;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[derive(Clone, Default)]
    struct VecWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl VecWriter {
        fn snapshot(&self) -> String {
            String::from_utf8(self.buf.lock().expect("buffer mutex").clone()).expect("utf-8")
        }
    }

    impl Write for VecWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.buf
                .lock()
                .expect("buffer mutex")
                .extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for VecWriter {
        type Writer = VecWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn capture_tracing(f: impl FnOnce()) -> String {
        let writer = VecWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_writer(writer.clone())
            .with_ansi(false)
            .with_target(false)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::callsite::rebuild_interest_cache();
            f();
            tracing::callsite::rebuild_interest_cache();
        });
        writer.snapshot()
    }

    fn nats_connect_app_error(source: impl std::error::Error + Send + Sync + 'static) -> AppError {
        let runtime = pardosa_nats::JetStreamRuntimeError::Connect {
            source: Box::new(source),
        };
        let backend = pardosa::store::BackendError::Connect {
            op: pardosa::store::BackendOp::Sync,
            source: Box::new(runtime),
        };
        let store = crate::store::StoreError::BackendInfrastructure {
            op: pardosa::store::BackendOp::Sync,
            source: Box::new(backend),
        };
        AppError::Persistence(PersistenceError::Io(std::io::Error::other(store)))
    }

    fn captured_error_chain(output: &str) -> String {
        output
            .lines()
            .find_map(|line| {
                let event = serde_json::from_str::<serde_json::Value>(line).ok()?;
                event
                    .get("fields")?
                    .get("error_chain")?
                    .as_str()
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| {
                panic!("initial collection failure log must include error_chain field: {output}")
            })
    }

    /// L1: a durable-write failure on the success path must not be
    /// silently swallowed by loop-continuation reporting the job as
    /// "completed" (CHE-0088/jxma5 no-Swallow guarantee). Reuses the
    /// empty-repo-name trick from
    /// `handle_success_outcome_escalates_swallowed_persist_failure_to_error`
    /// to deterministically force a classified (`Unrecoverable` ->
    /// `Fatal`) persist failure without real store infra.
    #[tokio::test]
    async fn handle_success_outcome_does_not_report_job_completed_on_persist_failure() {
        let state = AppState::new().await;
        let evidence = crate::test_fixtures::all_passing_evidence("");

        let output = capture_tracing(|| {
            handle_success_outcome(
                state.as_ref(),
                "no-swallow-test-key",
                &evidence,
                &JobSource::InitialLoad,
                Duration::from_millis(1),
            );
        });

        let completed = output.lines().any(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|event| {
                    event
                        .get("fields")?
                        .get("message")?
                        .as_str()
                        .map(String::from)
                })
                .is_some_and(|message| message == "job completed")
        });
        assert!(
            !completed,
            "a Fatal persist failure must not be reported as job completed: {output}"
        );
    }

    #[tokio::test]
    async fn handle_success_outcome_escalates_swallowed_persist_failure_to_error() {
        let state = AppState::new().await;
        let evidence = crate::test_fixtures::all_passing_evidence("");

        let output = capture_tracing(|| {
            handle_success_outcome(
                state.as_ref(),
                "escalation-test-key",
                &evidence,
                &JobSource::InitialLoad,
                Duration::from_millis(1),
            );
        });

        let event = output
            .lines()
            .find_map(|line| {
                let event = serde_json::from_str::<serde_json::Value>(line).ok()?;
                event.get("fields")?.get("persist_error_variant")?;
                Some(event)
            })
            .unwrap_or_else(|| {
                panic!(
                    "swallowed persist failure must emit a persist_error_variant field: {output}"
                )
            });

        assert_eq!(
            event.get("level").and_then(serde_json::Value::as_str),
            Some("ERROR"),
            "escalated persist failure must log at ERROR, not WARN: {event}"
        );
        assert_eq!(
            event["fields"]["persist_error_variant"].as_str(),
            Some("LoadFailed"),
            "empty repo name must surface as a LoadFailed persist error: {event}"
        );
    }

    #[test]
    fn log_job_persist_failure_logs_conflict_at_warn() {
        let failure = WriteFailure::classify(PersistenceError::FencedConflict {
            expected_seq: None,
            actual_seq: None,
            source: Box::new(std::io::Error::other("fence")),
        });
        let output = capture_tracing(|| {
            log_job_persist_failure(
                &failure,
                "acme/widgets",
                "widgets",
                &JobSource::InitialLoad,
                Duration::from_millis(1),
            );
        });
        let event = output
            .lines()
            .next()
            .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .expect("one log line");
        assert_eq!(
            event["level"].as_str(),
            Some("WARN"),
            "recognised fence conflict must log at WARN, not ERROR: {event}"
        );
    }

    #[test]
    fn log_job_persist_failure_logs_non_conflict_at_error() {
        let failure = WriteFailure::classify(PersistenceError::InvariantViolation {
            reason: "x".to_string(),
        });
        let output = capture_tracing(|| {
            log_job_persist_failure(
                &failure,
                "acme/widgets",
                "widgets",
                &JobSource::InitialLoad,
                Duration::from_millis(1),
            );
        });
        let event = output
            .lines()
            .next()
            .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .expect("one log line");
        assert_eq!(event["level"].as_str(), Some("ERROR"));
    }

    #[test]
    fn resolve_port_defaults_to_8080() {
        assert_eq!(resolve_port_with(|_| None).unwrap(), 8080);
    }

    #[test]
    fn resolve_port_reads_env_var() {
        assert_eq!(
            resolve_port_with(|_| Some("9090".to_string())).unwrap(),
            9090
        );
    }

    #[test]
    fn resolve_port_rejects_invalid_value() {
        let result = resolve_port_with(|_| Some("not_a_number".to_string()));
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn resolve_port_rejects_out_of_range() {
        let result = resolve_port_with(|_| Some("99999".to_string()));
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn resolve_bind_address_defaults_to_127_0_0_1() {
        assert_eq!(resolve_bind_address_with(|_| None).unwrap(), "127.0.0.1");
    }

    #[test]
    fn resolve_bind_address_reads_env_var() {
        assert_eq!(
            resolve_bind_address_with(|_| Some("0.0.0.0".to_string())).unwrap(),
            "0.0.0.0"
        );
    }

    #[test]
    fn resolve_bind_address_rejects_empty() {
        let result = resolve_bind_address_with(|_| Some(String::new()));
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn resolve_bind_address_trims_whitespace() {
        assert_eq!(
            resolve_bind_address_with(|_| Some("  0.0.0.0  ".to_string())).unwrap(),
            "0.0.0.0"
        );
    }

    #[test]
    fn resolve_bind_address_rejects_whitespace_only() {
        let result = resolve_bind_address_with(|_| Some("   ".to_string()));
        assert!(matches!(result, Err(ConfigError::InvalidValue { .. })));
    }

    #[test]
    fn lifecycle_log_contract_uses_expected_phase_values() {
        assert_eq!(PHASE_READY, "ready");
        assert_eq!(PHASE_SHUTDOWN_BEGIN, "shutdown_begin");
        assert_eq!(PHASE_DRAIN_POOL, "drain_pool");
        assert_eq!(PHASE_DRAIN_DELIVERY, "drain_delivery");
        assert_eq!(PHASE_DRAIN_COLLECTION, "drain_collection");
        assert_eq!(PHASE_STOPPED, "stopped");
    }

    #[test]
    fn lifecycle_log_contract_uses_static_messages() {
        assert_eq!(MESSAGE_READY, "daemon ready — serving");
        assert_eq!(MESSAGE_SHUTDOWN_BEGIN, "beginning graceful shutdown");
        assert_eq!(MESSAGE_STOPPED, "daemon stopped");
    }

    #[test]
    fn served_csp_adds_only_wasm_unsafe_eval_to_script_src() {
        let default_script_src_token = "script-src 'self';";
        let served_script_src_token = "script-src 'self' 'wasm-unsafe-eval';";
        assert!(!SERVED_CSP_WITH_WASM_UNSAFE_EVAL.contains(default_script_src_token));
        assert!(SERVED_CSP_WITH_WASM_UNSAFE_EVAL.contains(served_script_src_token));
        assert!(SERVED_CSP_WITH_WASM_UNSAFE_EVAL.contains("default-src 'self'"));
        assert!(SERVED_CSP_WITH_WASM_UNSAFE_EVAL.contains("style-src 'self'"));
        assert!(SERVED_CSP_WITH_WASM_UNSAFE_EVAL.contains("connect-src 'self'"));
        assert!(SERVED_CSP_WITH_WASM_UNSAFE_EVAL.contains("base-uri 'none'"));
        assert!(SERVED_CSP_WITH_WASM_UNSAFE_EVAL.contains("form-action 'none'"));
    }

    #[test]
    fn served_csp_is_accepted_by_server_config_builder() {
        let config = crate::server::served_dashboard_server_config();
        assert_eq!(
            config.csp_override(),
            Some(SERVED_CSP_WITH_WASM_UNSAFE_EVAL)
        );
    }

    #[test]
    fn one_shot_flag_yields_true_once_then_false_on_subsequent_runs() {
        let flag = OneShotFlag::new(true);

        let initial_run_value = flag.consume();
        let scheduled_run_value = flag.peek();
        let second_scheduled_run_value = flag.peek();

        assert!(
            initial_run_value,
            "initial collection must observe the flag as armed"
        );
        assert!(
            !scheduled_run_value,
            "first scheduled collection must observe the flag as consumed"
        );
        assert!(
            !second_scheduled_run_value,
            "flag must stay consumed across further scheduled collections"
        );
    }

    #[test]
    fn one_shot_flag_peek_does_not_clear() {
        let flag = OneShotFlag::new(true);

        assert!(flag.peek(), "peek must observe the armed state");
        assert!(flag.peek(), "peek must not clear the flag");
        assert!(flag.consume(), "flag must still be armed for consume");
        assert!(!flag.peek(), "consume must clear the flag");
    }

    #[test]
    fn spawn_collection_loop_integration_path_consumes_force_flags_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = RuntimeConfig {
            org_name: "TestOrg".to_string(),
            no_resume: true,
            max_workers: 1,
            store_dir: dir.path().to_path_buf(),
            pardosa_backend: config::runtime::PardosaBackend::Pgno,
            nats_url: config::runtime::DEFAULT_NATS_URL.to_string(),
            nats_creds: None,
            force_unlock: false,
            force_refresh: false,
            dashboard_config: config::dashboard::DashboardConfig::default(),
            team_roster_read_from_projection: true,
            rate_regulator: crate::config::runtime::RateRegulatorKind::default(),
        };
        let force_flag = OneShotFlag::new(true);
        let force_refresh_flag = OneShotFlag::new(true);

        let initial_cfg = initial_run_config(&config, &force_flag, &force_refresh_flag);
        let first_scheduled_cfg = scheduled_run_config(&config, &force_flag, &force_refresh_flag);
        let second_scheduled_cfg = scheduled_run_config(&config, &force_flag, &force_refresh_flag);

        assert!(
            initial_cfg.force_unlock && initial_cfg.force_refresh,
            "initial run must observe both force flags armed"
        );
        assert!(
            !first_scheduled_cfg.force_unlock && !first_scheduled_cfg.force_refresh,
            "first scheduled run must observe both force flags consumed"
        );
        assert!(
            !second_scheduled_cfg.force_unlock && !second_scheduled_cfg.force_refresh,
            "flags must stay consumed across further scheduled runs"
        );
    }

    #[test]
    fn duration_millis_reports_whole_milliseconds() {
        assert_eq!(duration_millis(Duration::from_millis(1_234)), 1_234);
    }

    #[test]
    fn initial_collection_failure_logs_full_nats_connect_error_chain() {
        let connect = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "connect refused");
        let app_error = nats_connect_app_error(connect);

        let output = capture_tracing(|| log_initial_collection_failure(&app_error));
        let error_chain = captured_error_chain(&output);
        let depth = error_chain.matches("\"level\":").count();

        assert!(
            depth > 1,
            "initial daemon absorption must preserve a non-flattened chain: {error_chain}"
        );
        assert!(
            error_chain.contains("connect")
                || error_chain.contains("Connection")
                || error_chain.contains("refused"),
            "chain must include the underlying async-nats connect source: {error_chain}"
        );
        assert!(
            !error_chain.contains("BEGIN NATS USER JWT"),
            "NATS credential bytes must not appear in connect diagnostics: {error_chain}"
        );
    }

    #[tokio::test]
    async fn bind_first_guard_returns_bind_failed_before_store_construction() {
        let first = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = first.local_addr().unwrap();
        let store_constructed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = Arc::clone(&store_constructed);

        let result = bind_serving_port_before_next_step(addr, || {
            observed.store(true, Ordering::Release);
        })
        .await;

        assert!(
            matches!(
                result,
                Err(cherry_pit_web::serve::ServerError::BindFailed { address, .. })
                    if address == addr
            ),
            "duplicate instance must return BindFailed before store construction, got {result:?}"
        );
        assert!(
            !store_constructed.load(Ordering::Acquire),
            "store construction must not run after duplicate bind"
        );
    }

    async fn team_refresh_test_state() -> (Arc<AppState>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let nats = config::runtime::NatsStoreConfig::for_org(
            "test-org",
            config::runtime::DEFAULT_NATS_URL,
        )
        .expect("nats config");
        let state = AppState::with_stores(&events_dir, config::runtime::PardosaBackend::Pgno, nats)
            .await
            .expect("with stores");
        (state, dir)
    }

    fn team_refresh_test_client(base_url: &str) -> crate::github::client::GitHubClient {
        let credential = crate::github::auth::GitHubCredential {
            mode: crate::domain::auth::AuthMode::Pat,
            token: secrecy::SecretString::from("test-token"),
            expires_at: None,
        };
        let budget = Arc::new(crate::github::budget::BudgetGate::new(
            config::API_BUDGET_LIMIT,
            Duration::from_secs(config::API_BUDGET_WAIT_SECS),
        ));
        let rate_limit = Arc::new(crate::github::rate_limit::new_default());
        crate::github::client::GitHubClient::new(
            credential, base_url, "test-org", None, budget, rate_limit,
        )
        .expect("test client construction should succeed")
    }

    async fn install_team_refresh_client(state: &Arc<AppState>, base_url: &str) {
        let client = Arc::new(team_refresh_test_client(base_url));
        state
            .github_client_or_try_init(|| async move { Ok::<_, std::convert::Infallible>(client) })
            .await
            .expect("client init is infallible");
    }

    /// A not-yet-initialised GitHub client must NOT let the startup tick
    /// silently no-op. `await_github_client` has no "skip" variant, so
    /// the only observable behaviours are "still waiting" and "Client".
    ///
    /// Falsifier: the pre-fix code path read `state.github_client()` once
    /// and `continue`d on `None`, which under this test would resolve
    /// instantly with the roster never fetched. Here the future is proven
    /// still pending after a full 24h of virtual time — longer than the
    /// refresh interval it is protecting — and then proven to resolve the
    /// moment the client appears.
    #[tokio::test(start_paused = true)]
    async fn startup_tick_waits_for_a_lazily_initialised_client_instead_of_skipping() {
        let (state, _dir) = team_refresh_test_state().await;
        let (_tx, mut cancel) = tokio::sync::watch::channel(false);

        assert!(
            state.github_client().is_none(),
            "precondition: the client must be uninitialised"
        );

        let mut waiting = Box::pin(await_github_client(&state, &mut cancel));
        let pending = tokio::time::timeout(
            Duration::from_secs(config::TEAM_REFRESH_INTERVAL_SECS),
            &mut waiting,
        )
        .await;
        assert!(
            pending.is_err(),
            "a not-yet-ready client must leave the startup tick WAITING, never \
             resolve it into a silent skip that forfeits a refresh period"
        );

        let server = MockServer::start().await;
        install_team_refresh_client(&state, &server.uri()).await;

        let resolved = tokio::time::timeout(Duration::from_hours(1), waiting)
            .await
            .expect("wait must resolve once the client is initialised");
        assert!(
            matches!(resolved, ClientReady::Client(_)),
            "the wait must hand back the now-initialised client"
        );
    }

    /// Cancellation is the ONLY exit from the wait without a client, so a
    /// shutdown during the startup race terminates the loop rather than
    /// hanging it.
    #[tokio::test(start_paused = true)]
    async fn startup_tick_wait_exits_on_cancellation_when_no_client_ever_arrives() {
        let (state, _dir) = team_refresh_test_state().await;
        let (tx, mut cancel) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let _ = tx.send(true);
        });
        let outcome = await_github_client(&state, &mut cancel).await;
        assert!(matches!(outcome, ClientReady::Cancelled));
    }

    /// The first roster fetch must happen at STARTUP, not one full
    /// `TEAM_REFRESH_INTERVAL_SECS` later. At 86400s the pre-fix
    /// sleep-first loop meant 24 hours of empty roster state after every
    /// Cloud Run revision.
    ///
    /// Virtual time is paused, so the elapsed figure asserted below is
    /// the scheduler's own accounting of how much of the interval the
    /// loop waited before its first write — not wall-clock noise.
    #[tokio::test(start_paused = true)]
    async fn team_refresh_loop_records_a_roster_at_startup_without_waiting_an_interval() {
        let (state, dir) = team_refresh_test_state().await;
        let evidence = crate::test_fixtures::make_repository_evidence(
            "repo-a",
            crate::domain::repository::Visibility::Public,
            false,
            crate::test_fixtures::make_checks(
                crate::test_fixtures::policy_pass_setting(),
                crate::test_fixtures::secret_enabled_observable(false),
                crate::test_fixtures::dependabot_enabled(),
                crate::test_fixtures::branch_pass(),
                crate::test_fixtures::codeowners_with_owners(&["@test-org/platform"]),
            ),
        );
        let domain_key = evidence.repository.inventory_key.clone();
        let repo_name = evidence.repository.name.clone();
        state
            .record_repo(&domain_key, evidence, &repo_name, "2026-07-23T00:00:00Z")
            .expect("seed repo evidence");

        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::path("/orgs/test-org/members"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
        Mock::given(wiremock::matchers::path(
            "/orgs/test-org/teams/platform/members",
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!([{"login": "octocat"}])),
        )
        .mount(&server)
        .await;
        install_team_refresh_client(&state, &server.uri()).await;

        let team_key =
            crate::event::team_domain_key("test-org", "platform").expect("derive team key");
        let config = RuntimeConfig {
            org_name: "test-org".to_string(),
            no_resume: true,
            max_workers: 1,
            store_dir: dir.path().to_path_buf(),
            pardosa_backend: config::runtime::PardosaBackend::Pgno,
            nats_url: config::runtime::DEFAULT_NATS_URL.to_string(),
            nats_creds: None,
            force_unlock: false,
            force_refresh: false,
            dashboard_config: config::dashboard::DashboardConfig::default(),
            team_roster_read_from_projection: true,
            rate_regulator: crate::config::runtime::RateRegulatorKind::default(),
        };
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);

        let started = tokio::time::Instant::now();
        let handle = spawn_team_refresh_loop(&config, Arc::clone(&state), cancel_rx);

        let recorded = tokio::time::timeout(
            Duration::from_secs(config::TEAM_REFRESH_INTERVAL_SECS),
            async {
                loop {
                    if state.lock_projection().team_rosters.contains_key(&team_key) {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            },
        )
        .await;
        let elapsed = started.elapsed();
        let _ = cancel_tx.send(true);
        handle.abort();

        assert!(
            recorded.is_ok(),
            "the team-refresh loop must record a roster at startup; waiting a \
             full {}s interval first is a 24h information regression",
            config::TEAM_REFRESH_INTERVAL_SECS,
        );
        assert!(
            elapsed < Duration::from_secs(config::TEAM_REFRESH_INTERVAL_SECS),
            "startup roster fetch consumed {elapsed:?} of virtual time, which is \
             not strictly less than one refresh interval — the loop is still \
             sleeping before its first tick"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn next_tick_returns_run_when_interval_elapses_first() {
        let (_tx, mut rx) = tokio::sync::watch::channel(false);
        let outcome = next_collection_tick(&mut rx, Duration::from_secs(10)).await;
        assert!(matches!(outcome, NextTick::Run));
    }

    #[tokio::test(start_paused = true)]
    async fn next_tick_returns_cancel_when_signalled_during_sleep() {
        let (tx, mut rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let _ = tx.send(true);
        });
        let outcome = next_collection_tick(&mut rx, Duration::from_hours(1)).await;
        assert!(matches!(outcome, NextTick::Cancel));
    }

    #[tokio::test(start_paused = true)]
    async fn next_tick_returns_cancel_when_already_signalled_before_call() {
        let (tx, mut rx) = tokio::sync::watch::channel(false);
        let _ = tx.send(true);
        let outcome = next_collection_tick(&mut rx, Duration::from_hours(1)).await;
        assert!(matches!(outcome, NextTick::Cancel));
    }

    #[tokio::test]
    async fn shutdown_workers_cancels_worker_pool_token_before_drain() {
        let state = AppState::new().await;
        let token = state.worker_shutdown_token();
        let observed = token.clone();
        let pool_handle = tokio::spawn(async move {
            observed.cancelled().await;
        });
        let delivery_handle = tokio::spawn(async {});
        assert!(
            state
                .worker_pool_started
                .set(std::sync::Mutex::new(Some((pool_handle, delivery_handle))))
                .is_ok()
        );

        let (cancel_tx, _cancel_rx) = tokio::sync::watch::channel(false);
        let mut collection_loop = tokio::spawn(async {});

        drain_shutdown_with_timeout(
            &state,
            &cancel_tx,
            &mut collection_loop,
            Duration::from_millis(100),
        )
        .await;

        assert!(token.is_cancelled());
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_drains_worker_delivery_and_collection_under_one_budget() {
        let state = AppState::new().await;
        let token = state.worker_shutdown_token();
        let pool_handle = tokio::spawn(std::future::pending::<()>());
        let delivery_handle = tokio::spawn(std::future::pending::<()>());
        assert!(
            state
                .worker_pool_started
                .set(std::sync::Mutex::new(Some((pool_handle, delivery_handle))))
                .is_ok()
        );
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let mut collection_loop = tokio::spawn(std::future::pending::<()>());
        let timeout = Duration::from_secs(3);
        let started = tokio::time::Instant::now();

        drain_shutdown_with_timeout(&state, &cancel_tx, &mut collection_loop, timeout).await;

        let elapsed = started.elapsed();
        assert!(token.is_cancelled());
        assert!(*cancel_rx.borrow());
        assert!(
            elapsed <= timeout + Duration::from_millis(1),
            "shutdown drain must use one shared timeout budget; elapsed={elapsed:?}, budget={timeout:?}"
        );
    }

    fn test_policy(max_attempts: u32) -> RearmPolicy {
        RearmPolicy {
            max_attempts,
            backoff_base: Duration::from_millis(1),
        }
    }

    fn fenced_conflict() -> AppError {
        AppError::Persistence(PersistenceError::FencedConflict {
            expected_seq: Some(1),
            actual_seq: Some(2),
            source: Box::new(std::io::Error::other("wrong last sequence")),
        })
    }

    #[tokio::test(start_paused = true)]
    async fn rearm_converges_after_one_resync_when_second_attempt_succeeds() {
        let resync_calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let run_calls = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let resync_calls_c = Arc::clone(&resync_calls);
        let run_calls_c = Arc::clone(&run_calls);
        let outcome = rearm_after_fenced_conflict(
            &test_policy(3),
            || {
                resync_calls_c.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            },
            || {
                let attempt = run_calls_c.fetch_add(1, Ordering::SeqCst) + 1;
                async move {
                    if attempt == 1 {
                        Ok(collect::CollectionOutcome::FencedConflict)
                    } else {
                        Ok(collect::CollectionOutcome::Completed)
                    }
                }
            },
        )
        .await;

        assert!(
            matches!(outcome, Ok(collect::CollectionOutcome::Completed)),
            "must converge to Completed on the second attempt, got {outcome:?}"
        );
        assert_eq!(
            run_calls.load(Ordering::SeqCst),
            2,
            "converges on second run"
        );
        assert_eq!(
            resync_calls.load(Ordering::SeqCst),
            2,
            "R10 guard: a fresh resync (re-read) must precede every retry — \
             the mechanism re-owns rather than blind-redriving the same op"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn rearm_never_redrives_without_a_preceding_resync_r10_guard() {
        let resync_calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let run_calls = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let resync_calls_c = Arc::clone(&resync_calls);
        let run_calls_c = Arc::clone(&run_calls);
        let _ = rearm_after_fenced_conflict(
            &test_policy(3),
            || {
                resync_calls_c.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            },
            || {
                run_calls_c.fetch_add(1, Ordering::SeqCst);
                async { Ok(collect::CollectionOutcome::FencedConflict) }
            },
        )
        .await;

        assert!(
            resync_calls.load(Ordering::SeqCst) >= run_calls.load(Ordering::SeqCst),
            "resync ({}) must never trail run ({}) — a blind redrive would let \
             run outpace resync",
            resync_calls.load(Ordering::SeqCst),
            run_calls.load(Ordering::SeqCst)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn rearm_gives_up_with_typed_error_after_cap_exhausted_still_fenced() {
        let outcome = rearm_after_fenced_conflict(
            &test_policy(3),
            || async { Ok(()) },
            || async { Ok(collect::CollectionOutcome::FencedConflict) },
        )
        .await;

        match outcome {
            Err(RearmError::StillFenced { attempts }) => assert_eq!(attempts, 3),
            other => panic!("expected StillFenced give-up, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn rearm_gives_up_with_typed_error_when_resync_itself_fails() {
        let outcome = rearm_after_fenced_conflict(
            &test_policy(2),
            || async { Err(std::io::Error::other("backend unreachable")) },
            || async { unreachable!("run must not be attempted without a successful resync") },
        )
        .await;

        match outcome {
            Err(RearmError::ResyncFailed { attempts, .. }) => assert_eq!(attempts, 2),
            other => panic!("expected ResyncFailed give-up, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn rearm_surfaces_run_failure_as_typed_error_after_cap() {
        let outcome = rearm_after_fenced_conflict(
            &test_policy(1),
            || async { Ok(()) },
            || async { Err(fenced_conflict()) },
        )
        .await;

        match outcome {
            Err(RearmError::RunFailed { attempts, .. }) => assert_eq!(attempts, 1),
            other => panic!("expected RunFailed give-up, got {other:?}"),
        }
    }

    fn team_tick_fenced_conflict() -> crate::app::team_refresh::TickFailure {
        crate::app::team_refresh::TickFailure {
            error: fenced_conflict(),
            context: crate::app::write_policy::WriteFailureContextOwned {
                org: Some("acme".to_string()),
                team_slug: Some("platform".to_string()),
                domain_key: None,
                writer_id: None,
            },
        }
    }

    /// Team-refresh converges through the SAME shared sink used by the
    /// collection loop (ghr-c905de05): a `FencedConflict` tick failure
    /// re-arms with a fresh read and converges within the bounded cap,
    /// rather than the prior warn-and-wait-forever shape.
    #[tokio::test(start_paused = true)]
    async fn team_refresh_converges_after_one_resync_when_second_attempt_succeeds() {
        let resync_calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let run_calls = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let resync_calls_c = Arc::clone(&resync_calls);
        let run_calls_c = Arc::clone(&run_calls);
        let (outcome, last_failure) = rearm_after_fenced_team_refresh(
            &test_policy(3),
            || {
                resync_calls_c.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            },
            || {
                let attempt = run_calls_c.fetch_add(1, Ordering::SeqCst) + 1;
                async move {
                    if attempt == 1 {
                        Err(team_tick_fenced_conflict())
                    } else {
                        Ok(())
                    }
                }
            },
        )
        .await;

        assert!(
            outcome.is_ok(),
            "must converge on the second attempt, got {outcome:?}"
        );
        assert_eq!(
            run_calls.load(Ordering::SeqCst),
            2,
            "converges on second run"
        );
        assert_eq!(
            resync_calls.load(Ordering::SeqCst),
            2,
            "R10 guard: a fresh resync (re-read) must precede every retry"
        );
        assert!(
            last_failure.is_some(),
            "the fenced first attempt must still be observable for logging"
        );
    }

    /// R10 GUARD (ghr-c905de05): a stale/drained writer that would
    /// patch-and-redrive the SAME op MUST LOSE — the shared combinator
    /// re-owns/fresh-reads between every retry via `resync`, never a
    /// blind in-append redrive of the cached sequence.
    #[tokio::test(start_paused = true)]
    async fn team_refresh_never_redrives_without_a_preceding_resync_r10_guard() {
        let resync_calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let run_calls = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let resync_calls_c = Arc::clone(&resync_calls);
        let run_calls_c = Arc::clone(&run_calls);
        let _ = rearm_after_fenced_team_refresh(
            &test_policy(3),
            || {
                resync_calls_c.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            },
            || {
                run_calls_c.fetch_add(1, Ordering::SeqCst);
                async { Err(team_tick_fenced_conflict()) }
            },
        )
        .await;

        assert!(
            resync_calls.load(Ordering::SeqCst) >= run_calls.load(Ordering::SeqCst),
            "resync ({}) must never trail run ({}) — a blind redrive would let \
             run outpace resync",
            resync_calls.load(Ordering::SeqCst),
            run_calls.load(Ordering::SeqCst)
        );
    }

    /// Give-up after the bounded cap is exhausted still fenced: the
    /// terminal failure (with its typed context) must be surfaced for
    /// exactly one `log_tick_failure` call, not lost by the Fenced
    /// classification carrying no payload through the shared combinator.
    #[tokio::test(start_paused = true)]
    async fn team_refresh_gives_up_with_last_failure_preserved_after_cap_exhausted_still_fenced() {
        let (outcome, last_failure) = rearm_after_fenced_team_refresh(
            &test_policy(3),
            || async { Ok(()) },
            || async { Err(team_tick_fenced_conflict()) },
        )
        .await;

        match outcome {
            Err(RearmError::StillFenced { attempts }) => assert_eq!(attempts, 3),
            other => panic!("expected StillFenced give-up, got {other:?}"),
        }
        let failure = last_failure.expect("last fenced failure must be preserved for logging");
        assert_eq!(failure.context.team_slug.as_deref(), Some("platform"));
    }

    /// Idempotency (CHE-0041 aggregate-owned): a converged retry after
    /// resync must not double-apply — the run closure is invoked exactly
    /// once per attempt, never re-invoked for the same attempt number.
    #[tokio::test(start_paused = true)]
    async fn team_refresh_convergence_does_not_double_invoke_run_per_attempt() {
        let run_calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let run_calls_c = Arc::clone(&run_calls);
        let (outcome, _) = rearm_after_fenced_team_refresh(
            &test_policy(3),
            || async { Ok(()) },
            move || {
                let n = run_calls_c.fetch_add(1, Ordering::SeqCst) + 1;
                async move {
                    if n == 1 {
                        Err(team_tick_fenced_conflict())
                    } else {
                        Ok(())
                    }
                }
            },
        )
        .await;

        assert!(outcome.is_ok());
        assert_eq!(
            run_calls.load(Ordering::SeqCst),
            2,
            "exactly one run invocation per attempt — no double-apply replay"
        );
    }

    /// Non-fence run failures still terminate promptly (not misclassified
    /// as a converge-worthy fence) — regression guard on the
    /// `is_fenced` classification split.
    #[tokio::test(start_paused = true)]
    async fn team_refresh_non_fence_failure_gives_up_as_run_failed_not_still_fenced() {
        let (outcome, last_failure) = rearm_after_fenced_team_refresh(
            &test_policy(2),
            || async { Ok(()) },
            || async {
                Err(crate::app::team_refresh::TickFailure {
                    error: AppError::Persistence(
                        cherry_pit_storage::PersistenceError::BackendUnavailable {
                            reason: "nats down".to_string(),
                        },
                    ),
                    context: crate::app::write_policy::WriteFailureContextOwned::default(),
                })
            },
        )
        .await;

        match outcome {
            Err(RearmError::RunFailed { attempts, .. }) => assert_eq!(attempts, 2),
            other => panic!("expected RunFailed give-up for a non-fence error, got {other:?}"),
        }
        assert!(last_failure.is_some());
    }
}

#[cfg(test)]
mod fence_propagation_tests {
    use super::*;
    use crate::app::write_policy::WriteFailure;

    fn fenced_failure() -> WriteFailure {
        WriteFailure::classify(PersistenceError::FencedConflict {
            expected_seq: Some(8901),
            actual_seq: Some(8902),
            source: Box::new(std::io::Error::other("wrong last sequence")),
        })
    }

    fn transient_failure() -> WriteFailure {
        WriteFailure::classify(PersistenceError::BackendUnavailable {
            reason: "backend down".to_string(),
        })
    }

    fn structural_failure() -> WriteFailure {
        WriteFailure::classify(PersistenceError::InvariantViolation {
            reason: "bad shape".to_string(),
        })
    }

    fn classify(failure: WriteFailure) -> DeliveryStep {
        classify_persist_failure(
            failure,
            "owner/repo",
            "repo",
            &JobSource::ScheduledBatch,
            Duration::from_millis(1),
        )
    }

    /// `success_criteria` 2: the typed conflict must NOT be turned into a
    /// log-record-and-continue. Asserts on the propagated typed error,
    /// never on a log line.
    #[test]
    fn fenced_persist_failure_is_not_swallowed_into_a_log_record() {
        let step = classify(fenced_failure());

        let DeliveryStep::Fenced(signal) = step else {
            panic!(
                "a typed FencedConflict must propagate as DeliveryStep::Fenced \
                 (CHE-0088:R3 no-swallow), not be logged and continued"
            );
        };
        assert!(
            matches!(signal.into_error(), PersistenceError::FencedConflict { .. }),
            "the fence signal must carry the typed error whole (PGN-0016:R9)"
        );
    }

    /// `success_criteria` 3: do not widen the blast radius to all persist
    /// errors — only the fence aborts the run.
    #[test]
    fn non_conflict_persist_failures_retain_existing_handling() {
        for failure in [transient_failure(), structural_failure()] {
            let category = failure.category;
            assert!(
                matches!(classify(failure), DeliveryStep::Delivered),
                "{category:?} must keep its existing log-and-continue handling"
            );
        }
    }

    async fn state_with_batch(
        count: usize,
    ) -> (Arc<AppState>, Arc<crate::app::work_queue::BatchTracker>) {
        let state = AppState::new().await;
        let tracker = crate::app::work_queue::BatchTracker::new(count);
        state.set_active_batch_tracker(Some(Arc::clone(&tracker)));
        (state, tracker)
    }

    /// `GAP-1`: a record whose persist was fenced must not complete a batch
    /// slot, and the barrier must be released so the run can abort
    /// instead of waiting on outcomes an aborted run never delivers.
    #[tokio::test]
    async fn fenced_record_aborts_the_batch_instead_of_completing_a_slot() {
        let (state, tracker) = state_with_batch(3).await;

        let DeliveryStep::Fenced(signal) = classify(fenced_failure()) else {
            panic!("fenced failure must classify as Fenced");
        };
        apply_delivery_step(
            &state,
            DeliveryStep::Fenced(signal),
            &JobSource::ScheduledBatch,
        );

        assert_eq!(
            tracker.remaining(),
            0,
            "the batch barrier must be released wholesale so the run aborts (PGN-0016:R2)"
        );
        assert!(
            state.run_is_fenced(),
            "the run-scoped fence latch must be armed for the run boundary to observe"
        );
    }

    /// `success_criteria` 4: the happy path still completes batches
    /// normally, one slot per delivered record.
    #[tokio::test]
    async fn delivered_record_completes_exactly_one_batch_slot() {
        let (state, tracker) = state_with_batch(3).await;

        apply_delivery_step(&state, DeliveryStep::Delivered, &JobSource::ScheduledBatch);

        assert_eq!(
            tracker.remaining(),
            2,
            "one delivered record completes one slot"
        );
        assert!(
            !state.run_is_fenced(),
            "a delivered record must not fence the run"
        );
    }

    /// `success_criteria` 1: the fence latched by the detached delivery
    /// task becomes the typed error at the run boundary, which the
    /// existing mapping turns into `CollectionOutcome::FencedConflict` —
    /// the value the daemon loop already routes into the single
    /// sanctioned `converge_on_fence` sink. No new re-arm call site.
    #[tokio::test(start_paused = true)]
    async fn latched_fence_converges_through_the_sanctioned_sink() {
        let (state, _tracker) = state_with_batch(1).await;
        let DeliveryStep::Fenced(signal) = classify(fenced_failure()) else {
            panic!("fenced failure must classify as Fenced");
        };
        apply_delivery_step(
            &state,
            DeliveryStep::Fenced(signal),
            &JobSource::ScheduledBatch,
        );

        let fence = state
            .take_run_fence()
            .expect("run boundary must observe the fence");
        let run_error = AppError::Persistence(fence.into_error());
        assert!(
            matches!(
                run_error,
                AppError::Persistence(PersistenceError::FencedConflict { .. })
            ),
            "the run boundary must abort with the typed conflict"
        );

        let policy = RearmPolicy {
            max_attempts: 2,
            backoff_base: Duration::from_millis(1),
        };
        let mut attempt = 0;
        let outcome = rearm_after_fenced_conflict(
            &policy,
            || async { Ok(()) },
            move || {
                attempt += 1;
                let fenced_again = attempt == 1;
                async move {
                    if fenced_again {
                        Ok(collect::CollectionOutcome::FencedConflict)
                    } else {
                        Ok(collect::CollectionOutcome::Completed)
                    }
                }
            },
        )
        .await;

        assert!(
            matches!(outcome, Ok(collect::CollectionOutcome::Completed)),
            "the fenced run must converge through converge_on_fence, got {outcome:?}"
        );
    }

    /// `GAP-2`: outcomes already in flight against a now-stale writer are
    /// discarded rather than written — the amplification mechanism. The
    /// discarded record is not lost: the converged run re-collects it.
    #[tokio::test]
    async fn in_flight_outcomes_are_discarded_once_the_run_is_fenced() {
        let (state, tracker) = state_with_batch(2).await;
        let DeliveryStep::Fenced(signal) = classify(fenced_failure()) else {
            panic!("fenced failure must classify as Fenced");
        };
        apply_delivery_step(
            &state,
            DeliveryStep::Fenced(signal),
            &JobSource::ScheduledBatch,
        );

        let (outcome_tx, outcome_rx) = tokio::sync::mpsc::channel(1);
        outcome_tx
            .send(JobOutcome::Success {
                domain_key: "stale-repo".to_string(),
                result: crate::test_fixtures::all_passing_evidence("stale-repo"),
                source: JobSource::ScheduledBatch,
                duration: Duration::from_millis(1),
                correlation: cherry_pit_core::CorrelationContext::none(),
            })
            .await
            .expect("queue outcome");
        drop(outcome_tx);

        delivery_loop(outcome_rx, Arc::clone(&state)).await;

        assert!(
            !state.projection_contains("stale-repo"),
            "a record in flight against a superseded writer must not be written (PGN-0016:R2)"
        );
        assert_eq!(
            tracker.remaining(),
            0,
            "the aborted batch stays drained; discards must not underflow the tracker"
        );
    }

    /// The latch is run-scoped: arming a fresh batch clears a fence left
    /// by the previous, aborted run.
    #[tokio::test]
    async fn arming_a_new_batch_clears_the_previous_runs_fence() {
        let (state, _tracker) = state_with_batch(1).await;
        let DeliveryStep::Fenced(signal) = classify(fenced_failure()) else {
            panic!("fenced failure must classify as Fenced");
        };
        apply_delivery_step(
            &state,
            DeliveryStep::Fenced(signal),
            &JobSource::ScheduledBatch,
        );
        assert!(state.run_is_fenced());

        let next = crate::app::work_queue::BatchTracker::new(2);
        state.set_active_batch_tracker(Some(next));

        assert!(
            !state.run_is_fenced(),
            "a new run must not inherit the aborted run's fence"
        );
    }

    struct ScriptedRecorder {
        result: std::sync::Mutex<Option<WriteFailure>>,
    }

    impl ScriptedRecorder {
        fn failing(failure: WriteFailure) -> Arc<Self> {
            Arc::new(Self {
                result: std::sync::Mutex::new(Some(failure)),
            })
        }
    }

    impl RepoRecorder for ScriptedRecorder {
        fn record_repo(
            &self,
            _domain_key: &str,
            _evidence: RepositoryEvidence,
            _repo_name: &str,
            _timestamp: &str,
        ) -> Result<(), WriteFailure> {
            match self.result.lock().unwrap().take() {
                Some(failure) => Err(failure),
                None => Ok(()),
            }
        }
    }

    async fn state_with_projected_repo() -> (Arc<AppState>, String) {
        let state = AppState::new().await;
        let evidence = crate::test_fixtures::all_passing_evidence("fail-repo");
        let domain_key = evidence.repository.inventory_key.clone();
        state.lock_projection().load_baseline(vec![evidence]);
        (state, domain_key)
    }

    fn failure_step(state: &Arc<AppState>, recorder: &ScriptedRecorder, key: &str) -> DeliveryStep {
        handle_failure_outcome(
            state,
            recorder,
            key,
            "simulated evaluation failure",
            &JobSource::ScheduledBatch,
            Duration::from_millis(1),
        )
    }

    /// `success_criteria` 2 on the sibling write: the failure-state record
    /// is a second repository append on the same fiber, so a `Conflict`
    /// there must propagate as the typed fence rather than be logged and
    /// accounted as delivered (CHE-0088:R3).
    #[tokio::test]
    async fn fenced_failure_state_persist_is_not_swallowed() {
        let (state, domain_key) = state_with_projected_repo().await;
        let recorder = ScriptedRecorder::failing(fenced_failure());

        let DeliveryStep::Fenced(signal) = failure_step(&state, &recorder, &domain_key) else {
            panic!(
                "a typed FencedConflict from the failure-state write must propagate as \
                 DeliveryStep::Fenced, not be logged and counted as delivered"
            );
        };
        assert!(
            matches!(signal.into_error(), PersistenceError::FencedConflict { .. }),
            "the fence signal must carry the typed error whole (PGN-0016:R9)"
        );
    }

    /// `success_criteria` 3 on the sibling write: the blast radius stays
    /// scoped to the fence — non-conflict failure-state persist errors
    /// keep their existing log-and-continue handling.
    #[tokio::test]
    async fn non_conflict_failure_state_persist_retains_existing_handling() {
        for failure in [transient_failure(), structural_failure()] {
            let category = failure.category;
            let (state, domain_key) = state_with_projected_repo().await;
            let recorder = ScriptedRecorder::failing(failure);

            assert!(
                matches!(
                    failure_step(&state, &recorder, &domain_key),
                    DeliveryStep::Delivered
                ),
                "{category:?} must keep its existing log-and-continue handling"
            );
        }
    }

    /// A failed job whose failure-state record persists cleanly still
    /// completes its batch slot — the happy path of the failure branch.
    #[tokio::test]
    async fn successful_failure_state_persist_stays_delivered() {
        let (state, domain_key) = state_with_projected_repo().await;
        let recorder = Arc::new(ScriptedRecorder {
            result: std::sync::Mutex::new(None),
        });

        assert!(matches!(
            failure_step(&state, &recorder, &domain_key),
            DeliveryStep::Delivered
        ));
        assert!(!state.run_is_fenced());
    }
}
