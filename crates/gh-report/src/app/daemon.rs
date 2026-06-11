//! The daemon: scheduled collection + serving.
//!
//! Runs the web server while executing collection runs on a fixed interval.
//! The daemon is the only operational mode — it handles both data collection
//! and serving from an in-memory cache.
//!
//! ## Startup order
//!
//! 1. **Projection runtime init** — `snapshot_fast_path_init` replays
//!    the event log (or fast-paths from the latest projection snapshot
//!    per CHE-0048:R1) so the in-memory `EvidenceProjection` is current
//!    before any reader can observe it (CHE-0048:R2 — projection is
//!    the source of truth at boot; δ.3c-ii retired the prior
//!    `baseline.msgpack` snapshot file).
//! 2. **Warm-start** — render the dashboard from the projection and
//!    populate the HTML cache so the server can respond to page
//!    requests within seconds. Falls through gracefully if the
//!    projection is empty (fresh install) — the server returns 503
//!    until the first sweep completes.
//! 3. **Start the web server** — binds immediately (serves warm-start
//!    data or returns 503 if the projection was empty).
//! 4. **Background collection** — the initial API collection and subsequent
//!    scheduled runs happen in a background task. Each successful run
//!    atomically updates the HTML cache.
//! 5. **Worker pool** — started lazily by `AppState::ensure_worker_pool()`
//!    inside `collect::run_collection_inner()` after the first successful
//!    credential resolution. The pool persists across collection runs
//!    (shared between sweep and webhook jobs).
//!
//! The daemon shuts down gracefully on `Ctrl-C` or `SIGTERM`: it cancels
//! background collection loop and stops the HTTP server.
//! **`--force-unlock` semantics:** The flag is one-shot — it applies only
//! to the initial collection run. Subsequent scheduled runs do not
//! force-unlock.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cherry_pit_core::CorrelationContext;
use uuid::Uuid;

use crate::app::collect;
use crate::app::event_logging::register_logging_subscriber;
use crate::app::state::AppState;
use crate::app::work_queue::JobSource;
use crate::app::worker_pool::JobOutcome;
use crate::config;
use crate::config::runtime::RuntimeConfig;
use crate::domain::aggregates::repo::RecordEvaluation;
use crate::domain::evidence::RepositoryEvidence;
use crate::domain::run::RunMetadata;
use crate::error::{AppError, ConfigError, PersistenceError};
use tracing::{debug, error, info, warn};

/// Bounded per-handle timeout for the worker-pool and delivery-task
/// drain at daemon shutdown. The total drain budget at shutdown is
/// `2 ×` this value (pool then delivery, sequentially).
const WORKER_POOL_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Bounded budget for cooperative drain of the scheduled collection
/// task on daemon shutdown. After this elapses, the task is forcibly
/// aborted and a warning is emitted; any in-flight persist→publish is
/// recovered by `EventStore` boot replay (CHE-0024:R3 persist-before-
/// publish ordering keeps this consistent).
const COLLECTION_DRAIN_TIMEOUT: Duration = Duration::from_mins(1);

/// Outcome of waiting for the next scheduled collection tick.
#[derive(Debug)]
enum NextTick {
    Run,
    Cancel,
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
pub async fn run(config: RuntimeConfig) -> Result<(), AppError> {
    let port = resolve_port()?;
    let bind_address = resolve_bind_address()?;

    info!(
        org = %config.org_name,
        bind = %bind_address,
        port,
        interval_secs = config::COLLECTION_INTERVAL_SECS,
        "daemon starting"
    );

    let events_dir = config.store_dir.join("events").join(&config.org_name);
    let projections_dir = config.store_dir.join("projections").join(&config.org_name);
    let nats = config.nats_store_config()?;
    let app_state =
        AppState::with_stores(&events_dir, projections_dir, config.pardosa_backend, nats)
            .await
            .map_err(|source| {
                AppError::Persistence(PersistenceError::LoadFailed {
                    reason: format!("open event store at {}: {source}", events_dir.display()),
                })
            })?;

    if let Err(e) = app_state.snapshot_fast_path_init().await {
        error!(error = %e, "projection runtime init failed");
        return Err(AppError::Persistence(PersistenceError::LoadFailed {
            reason: format!("projection runtime init failed: {e}"),
        }));
    }

    collect::warm_start_from_baseline(&config, &app_state).await;

    register_logging_subscriber(&app_state.bus);

    let shutdown = async {
        crate::infra::signal::wait_for_shutdown_signal().await;
        info!("shutdown signal received");
    };

    let force_flag = Arc::new(AtomicBool::new(config.force_unlock));
    let (collect_cancel_tx, collect_cancel_rx) = tokio::sync::watch::channel(false);
    let mut collection_loop = spawn_collection_loop(
        config.clone(),
        Arc::clone(&app_state),
        Arc::clone(&force_flag),
        collect_cancel_rx,
    );

    let mut extra_routes = crate::server::status_router();
    if app_state.webhook().secret.is_some() {
        info!("webhooks enabled (WEBHOOK_SECRET set)");
        extra_routes = extra_routes.merge(crate::webhook::webhook_router());
    } else {
        info!("webhooks disabled (WEBHOOK_SECRET not set)");
    }

    let server_result = crate::infra::server::server::start(
        port,
        &bind_address,
        shutdown,
        Arc::clone(&app_state),
        &crate::infra::server::config::ServerConfig::builder()
            .build()
            .expect("default config is valid"),
        None,
        Some(extra_routes),
    )
    .await;

    shutdown_workers(&app_state).await;
    drain_collection_loop(&collect_cancel_tx, &mut collection_loop).await;

    server_result.map_err(|e| crate::error::ServerError::Runtime(e.to_string()))?;

    info!("daemon stopped");
    Ok(())
}

/// Drain the worker pool + delivery task on daemon shutdown.
///
/// Closes the work queue first so the pool stops accepting new jobs and
/// finishes those in flight, then awaits both handles with an individual
/// timeout. Total drain budget upper-bounded at `2 ×
/// WORKER_POOL_DRAIN_TIMEOUT`.
async fn shutdown_workers(app_state: &Arc<AppState>) {
    app_state.work_queue.close();
    let (pool_drained, delivery_drained) =
        app_state.drain_worker_pool(WORKER_POOL_DRAIN_TIMEOUT).await;
    if !pool_drained {
        warn!(
            timeout_secs = WORKER_POOL_DRAIN_TIMEOUT.as_secs(),
            "worker pool did not drain within timeout"
        );
    }
    if !delivery_drained {
        warn!(
            timeout_secs = WORKER_POOL_DRAIN_TIMEOUT.as_secs(),
            "delivery task did not drain within timeout"
        );
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
    force_flag: Arc<AtomicBool>,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        {
            let mut cfg = config.clone();
            cfg.force_unlock = force_flag.fetch_and(false, Ordering::AcqRel);
            match collect::run(cfg, Arc::clone(&state)).await {
                Ok(()) => info!("initial collection complete"),
                Err(AppError::Persistence(PersistenceError::LockFailed { ref reason })) => {
                    warn!(reason = %reason, "initial collection skipped: lock held");
                }
                Err(e) => error!(error = %e, "initial collection failed — will retry"),
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
            let mut cfg = config.clone();
            cfg.force_unlock = force_flag.load(Ordering::Acquire);
            match collect::run(cfg, Arc::clone(&state)).await {
                Ok(()) => info!("scheduled collection complete"),
                Err(AppError::Persistence(PersistenceError::LockFailed { ref reason })) => {
                    warn!(reason = %reason, "collection skipped: lock held");
                }
                Err(e) => error!(error = %e, "scheduled collection failed"),
            }
        }
    })
}

/// Cooperative shutdown of the scheduled collection task.
///
/// Sends the cancel signal so the loop exits between scheduled
/// iterations rather than mid-`collect::run`, then awaits the
/// `JoinHandle` with a bounded budget. On timeout the task is forcibly
/// aborted and a structured warning is emitted: the outcome of any
/// in-flight persist→publish is unknown to this process, but
/// `EventStore` boot replay reconciles state on next startup
/// (CHE-0024:R3 persist-before-publish ensures the durable record is
/// the source of truth).
async fn drain_collection_loop(
    cancel: &tokio::sync::watch::Sender<bool>,
    handle: &mut tokio::task::JoinHandle<()>,
) {
    let _ = cancel.send(true);
    match tokio::time::timeout(COLLECTION_DRAIN_TIMEOUT, &mut *handle).await {
        Ok(Ok(())) => info!("collection task drained cleanly"),
        Ok(Err(join_err)) => warn!(
            error = %join_err,
            "collection task ended abnormally during drain",
        ),
        Err(_) => {
            warn!(
                timeout_secs = COLLECTION_DRAIN_TIMEOUT.as_secs(),
                "collection task forced abort: outcome of any in-flight persist→publish is unknown to this process; EventStore boot replay will reconcile on next startup",
            );
            handle.abort();
            let _ = handle.await;
        }
    }
}

/// Delivery task: consumes job outcomes from the worker pool channel.
///
/// Responsibilities:
/// 1. Publish `RepoEvaluated` success envelopes (drives `EvidenceProjection`
///    via `apply` per CHE-0048:R2 — projection is the sole writer of the
///    read-model post-M2.cd).
/// 2. Publish `RepoEvaluated` failure envelopes carrying synthesised
///    `Unknown`-status evidence (so the dashboard shows error state rather
///    than stale passing data).
/// 3. `batch_tracker.complete_one()` for `ScheduledBatch` outcomes
///    (countdown so the sweep knows when all jobs are done)
///
/// Log lines include repo-name enrichment: the repository name is extracted
/// from evidence so operators can identify repos without looking up numeric IDs.
///
/// Phase E adds: incremental rendering, WS broadcast.
pub(crate) async fn delivery_loop(
    mut rx: tokio::sync::mpsc::Receiver<JobOutcome<RepositoryEvidence>>,
    state: Arc<AppState>,
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

        let corr_ctx = state.current_run.load().as_ref().as_ref().map_or_else(
            || {
                debug!("delivery_loop: outcome arrived with no in-flight run; using nil ctx");
                CorrelationContext::correlated(Uuid::nil())
            },
            RunMetadata::correlation_context,
        );

        match outcome {
            JobOutcome::Success {
                domain_key, result, ..
            } => {
                handle_success_outcome(&state, &corr_ctx, domain_key, result, &source, duration)
                    .await;
            }
            JobOutcome::Failure {
                domain_key, error, ..
            } => {
                handle_failure_outcome(&state, &corr_ctx, domain_key, error, &source, duration)
                    .await;
            }
            _ => {
                warn!("delivery_loop: unhandled JobOutcome variant, skipping");
                continue;
            }
        }

        if matches!(source, JobSource::ScheduledBatch) {
            let tracker_guard = state.evidence().batch_tracker.load();
            if let Some(tracker) = tracker_guard.as_ref() {
                tracker.complete_one();
            }
        }
    }
    info!("delivery task exiting — outcome channel closed");
}

/// Publish a successful repo evaluation and log completion.
///
/// Extracted from [`delivery_loop`] for cohesion; no behavioural change.
async fn handle_success_outcome(
    state: &Arc<AppState>,
    corr_ctx: &CorrelationContext,
    domain_key: String,
    result: RepositoryEvidence,
    source: &JobSource,
    duration: Duration,
) {
    let repo_name = result.repository.name.clone();
    let evidence_for_event = Box::new(result);
    if let Err(e) = state
        .repo_service
        .record_evaluation(
            &domain_key,
            RecordEvaluation {
                domain_key: domain_key.clone(),
                repo_name: repo_name.clone(),
                success: true,
                source: format!("{source:?}"),
                duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                timestamp: jiff::Timestamp::now().to_string(),
                evidence: Some(evidence_for_event),
            },
            corr_ctx,
        )
        .await
    {
        tracing::warn!(?e, "RepoEvaluated publish failed, non-fatal");
    }
    info!(
        key = %domain_key,
        repo = %repo_name,
        source = ?source,
        duration_ms = duration.as_millis(),
        "job completed"
    );
}

/// Publish a failed repo evaluation and log the failure.
///
/// Extracted from [`delivery_loop`] for cohesion; no behavioural change.
async fn handle_failure_outcome(
    state: &Arc<AppState>,
    corr_ctx: &CorrelationContext,
    domain_key: String,
    error: String,
    source: &JobSource,
    duration: Duration,
) {
    let existing = state.projection_get(&domain_key);
    let (repo_name, evidence_for_event) = if let Some(existing) = existing {
        let name = existing.repository.name.clone();
        let failure = collect::failure_evidence(
            &std::sync::Arc::new(existing.repository.clone()),
            &jiff::Timestamp::now().to_string(),
        );
        let failure_for_event = Box::new(failure);
        (name, Some(failure_for_event))
    } else {
        (domain_key.clone(), None)
    };
    if let Err(e) = state
        .repo_service
        .record_evaluation(
            &domain_key,
            RecordEvaluation {
                domain_key: domain_key.clone(),
                repo_name: repo_name.clone(),
                success: false,
                source: format!("{source:?}"),
                duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                timestamp: jiff::Timestamp::now().to_string(),
                evidence: evidence_for_event,
            },
            corr_ctx,
        )
        .await
    {
        tracing::warn!(?e, "RepoEvaluated publish failed, non-fatal");
    }
    error!(
        key = %domain_key,
        repo = %repo_name,
        source = ?source,
        error = %error,
        duration_ms = duration.as_millis(),
        "job failed"
    );
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
