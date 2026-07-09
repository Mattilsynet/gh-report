//! The daemon: scheduled collection + serving.
//!
//! Runs the web server while executing collection runs on a fixed interval.
//! The daemon is the only operational mode — it handles both data collection
//! and serving from an in-memory cache.
//!
//! ## Startup order
//!
//! 1. **Serving port bind** — the TCP bind is the duplicate-instance guard
//!    and runs before store handles, projection replay, warm-start rendering,
//!    run-lock acquisition, or credential resolution.
//! 2. **Projection runtime init** — `snapshot_fast_path_init` replays
//!    the event log (or fast-paths from the latest projection snapshot
//!    per CHE-0048:R1) so the in-memory `EvidenceProjection` is current
//!    before any reader can observe it (CHE-0048:R2 — projection is
//!    the source of truth at boot; δ.3c-ii retired the prior
//!    `baseline.msgpack` snapshot file).
//! 3. **Warm-start** — render the dashboard from the projection and
//!    populate the HTML cache so the server can respond to page
//!    requests within seconds. Falls through gracefully if the
//!    projection is empty (fresh install) — the server returns 503
//!    until the first sweep completes.
//! 4. **Start the web server** — serves through the pre-bound listener
//!    immediately (serves warm-start data or returns 503 if the projection
//!    was empty).
//! 5. **Background collection** — the initial API collection and subsequent
//!    scheduled runs happen in a background task. Each successful run
//!    atomically updates the HTML cache.
//! 6. **Worker pool** — started lazily by `AppState::ensure_worker_pool()`
//!    inside `collect::run_collection_inner()` after the first successful
//!    credential resolution. The pool persists across collection runs
//!    (shared between sweep and webhook jobs).
//!
//! The daemon shuts down gracefully on `Ctrl-C` or `SIGTERM`: it cancels
//! background collection loop and stops the HTTP server.
//! **`--force-unlock` semantics:** The flag is one-shot — it applies only
//! to the initial collection run. Subsequent scheduled runs do not
//! force-unlock.
//! **`--force-refresh` semantics:** Same one-shot shape as `--force-unlock`
//! — it bypasses baseline reuse for the initial collection only.
//! Subsequent scheduled runs resume normal baseline reuse.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::app::collect;
use crate::app::state::{AppState, log_error_chain, read_rss_kb};
use crate::app::work_queue::JobSource;
use crate::app::worker_pool::JobOutcome;
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
const SERVED_CSP_WITH_WASM_UNSAFE_EVAL: &str = "default-src 'self'; style-src 'self'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; base-uri 'none'; form-action 'none'";

fn duration_millis(duration: Duration) -> u128 {
    duration.as_millis()
}

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

    let force_flag = Arc::new(AtomicBool::new(config.force_unlock));
    let force_refresh_flag = Arc::new(AtomicBool::new(config.force_refresh));
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
        collect_cancel_rx,
    );
    let server_config = cherry_pit_web::serve::ServerConfig::builder()
        .csp_override(SERVED_CSP_WITH_WASM_UNSAFE_EVAL)
        .build()
        .expect("default config is valid");

    let server_result = cherry_pit_web::serve::start(
        port,
        &bind_address,
        Some(listener),
        shutdown,
        Arc::clone(&app_state),
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

/// Spawn the background collection task: one initial run with the
/// caller-supplied `force_unlock` flag, then a scheduled loop that
/// honours a cooperative cancellation signal between iterations. The
/// loop never cancels an in-flight `collect::run`; persist→publish
/// runs to completion before the next tick is considered.
fn spawn_collection_loop(
    config: RuntimeConfig,
    state: Arc<AppState>,
    force_flag: Arc<AtomicBool>,
    force_refresh_flag: Arc<AtomicBool>,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        {
            let mut cfg = config.clone();
            cfg.force_unlock = force_flag.fetch_and(false, Ordering::AcqRel);
            cfg.force_refresh = force_refresh_flag.fetch_and(false, Ordering::AcqRel);
            match collect::run_with_outcome(cfg, Arc::clone(&state)).await {
                Ok(collect::CollectionOutcome::Completed) => info!("initial collection complete"),
                Ok(collect::CollectionOutcome::Cancelled) => {
                    info!("initial collection aborted on shutdown — no report published");
                }
                Ok(collect::CollectionOutcome::FencedConflict) => {
                    warn!(owner_id = %state.owner_id, "initial collection fenced by another writer — schedule re-armed");
                }
                Err(AppError::Persistence(PersistenceError::LockFailed { ref reason })) => {
                    error!(reason = %reason, "initial collection skipped: lock held");
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
            let mut cfg = config.clone();
            cfg.force_unlock = force_flag.load(Ordering::Acquire);
            cfg.force_refresh = force_refresh_flag.load(Ordering::Acquire);
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
                    warn!(owner_id = %state.owner_id, "scheduled collection fenced by another writer — schedule re-armed");
                }
                Err(AppError::Persistence(PersistenceError::LockFailed { ref reason })) => {
                    warn!(reason = %reason, "collection skipped: lock held");
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

        match outcome {
            JobOutcome::Success {
                domain_key, result, ..
            } => {
                handle_success_outcome(&state, &domain_key, result, &source, duration);
            }
            JobOutcome::Failure {
                domain_key, error, ..
            } => {
                handle_failure_outcome(&state, &domain_key, &error, &source, duration);
            }
            _ => {
                warn!("delivery_loop: unhandled JobOutcome variant, skipping");
                continue;
            }
        }

        if matches!(source, JobSource::ScheduledBatch) {
            state.complete_active_batch();
        }
    }
    info!("delivery task exiting — outcome channel closed");
}

/// Publish a successful repo evaluation and log completion.
///
/// Extracted from [`delivery_loop`] for cohesion; no behavioural change.
fn handle_success_outcome(
    state: &Arc<AppState>,
    domain_key: &str,
    result: RepositoryEvidence,
    source: &JobSource,
    duration: Duration,
) {
    let repo_name = result.repository.name.clone();
    let timestamp = jiff::Timestamp::now().to_string();
    if let Err(e) = state.record_repo(domain_key, result, &repo_name, &timestamp) {
        tracing::error!(
            persist_error_variant = persist_error_variant(&e),
            ?e,
            "repository state record failed, non-fatal"
        );
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
fn handle_failure_outcome(
    state: &Arc<AppState>,
    domain_key: &str,
    error: &str,
    source: &JobSource,
    duration: Duration,
) {
    let existing = state.projection_get(domain_key);
    let repo_name = if let Some(existing) = existing {
        let name = existing.repository.name.clone();
        let failure = collect::failure_evidence(
            &std::sync::Arc::new(existing.repository.clone()),
            &jiff::Timestamp::now().to_string(),
        );
        let timestamp = jiff::Timestamp::now().to_string();
        if let Err(e) = state.record_repo(domain_key, failure, &name, &timestamp) {
            tracing::error!(
                persist_error_variant = persist_error_variant(&e),
                ?e,
                "repository failure state record failed, non-fatal"
            );
        }
        name
    } else {
        domain_key.to_string()
    };
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
    use std::io::Write;
    use tracing_subscriber::fmt::MakeWriter;

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

    #[tokio::test]
    async fn handle_success_outcome_escalates_swallowed_persist_failure_to_error() {
        let state = AppState::new().await;
        let evidence = crate::test_fixtures::all_passing_evidence("");

        let output = capture_tracing(|| {
            handle_success_outcome(
                &state,
                "escalation-test-key",
                evidence,
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
        let config = cherry_pit_web::serve::ServerConfig::builder()
            .csp_override(SERVED_CSP_WITH_WASM_UNSAFE_EVAL)
            .build()
            .unwrap();
        assert_eq!(
            config.csp_override(),
            Some(SERVED_CSP_WITH_WASM_UNSAFE_EVAL)
        );
    }

    #[test]
    fn one_shot_flag_yields_true_once_then_false_on_subsequent_runs() {
        let force_refresh_flag = Arc::new(AtomicBool::new(true));

        let initial_run_value = force_refresh_flag.fetch_and(false, Ordering::AcqRel);
        let scheduled_run_value = force_refresh_flag.load(Ordering::Acquire);
        let second_scheduled_run_value = force_refresh_flag.load(Ordering::Acquire);

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
}
