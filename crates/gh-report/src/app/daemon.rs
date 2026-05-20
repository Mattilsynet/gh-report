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

    // Create shared application state.
    //
    // WU-6 v2 B3' + B4' (charter wu6v2-charter-1778415390): wire both
    // durable stores at sibling subtrees per BC-v2-13 (events/ and
    // projections/ disjoint):
    //
    //   <store_dir>/events/<org>/      — PardosaLogEventStore<DomainEvent>
    //                                     (file-per-aggregate, xxh64-framed
    //                                     pardosa_encoding::Encode envelopes)
    //   <store_dir>/projections/<org>/ — FileProjectionStore<EvidenceProjection>
    //
    // The event-store directory is created and locked at `open` time
    // (CHE-0043:R1 — exclusive advisory flock on {dir}/.lock held for
    // the store's lifetime). The projection-store directory is created
    // lazily on first write. Held but not yet exercised:
    //   - B5' wires ProjectionDriverExt to consume both stores
    //     (snapshot fast-path replay, CHE-0051:R5).
    //   - B7' rewrites collectors to call event_store.append(...)
    //     then bus.publish(...) per BC-v2-1 / CHE-0024:R1.
    let events_dir = config.store_dir.join("events").join(&config.org_name);
    let projections_dir = config.store_dir.join("projections").join(&config.org_name);
    let app_state = AppState::with_stores(&events_dir, projections_dir)
        .await
        .map_err(|source| {
            AppError::Persistence(PersistenceError::LoadFailed {
                reason: format!("open event store at {}: {source}", events_dir.display()),
            })
        })?;

    // ── Step 1b: Snapshot-fast-path projection runtime init (B5') ─
    // Replays events past the persisted checkpoint into the
    // in-memory projection state and registers the bus handler that
    // keeps it current. Per CHE-0048:R2 the snapshot is the source
    // of truth at boot — must run before warm_start so any reader
    // sees the up-to-date projection. Failure here aborts startup
    // (the daemon cannot serve correct evidence with a broken
    // projection runtime).
    if let Err(e) = app_state.snapshot_fast_path_init().await {
        error!(error = %e, "projection runtime init failed");
        return Err(AppError::Persistence(PersistenceError::LoadFailed {
            reason: format!("projection runtime init failed: {e}"),
        }));
    }

    // ── Step 2: Warm-start from baseline ────────────────────────
    // Populates the HTML cache from the most recent baseline so the
    // server can start serving immediately. Falls through gracefully
    // if no baseline exists (server will return 503 until first
    // collection completes).
    collect::warm_start_from_baseline(&config, &app_state).await;

    // ── Step 2b: Register domain event logging handler ─────────
    // Registers a synchronous handler on the projection-runtime bus
    // that logs every domain event via tracing. Proves fan-out: it
    // runs alongside the projection handler registered by
    // `snapshot_fast_path_init` (CHE-0024:§7 — handlers invoked
    // synchronously inside `publish`). No task to manage.
    register_logging_subscriber(&app_state.bus);

    // ── Step 3: Start the web server ────────────────────────────
    // The server blocks until the shutdown signal is received. It
    // starts immediately — warm-start data (if available) is already
    // in the cache.
    let shutdown = async {
        crate::infra::signal::wait_for_shutdown_signal().await;
        info!("shutdown signal received");
    };

    // ── Step 4: Background collection (initial + loop) ──────────
    // `force_unlock` is one-shot: applies to the initial run only.
    let force_flag = Arc::new(AtomicBool::new(config.force_unlock));
    let collection_loop = {
        let loop_config = config.clone();
        let loop_state = Arc::clone(&app_state);
        let loop_force = Arc::clone(&force_flag);
        tokio::spawn(async move {
            // Initial collection (first iteration, no sleep).
            {
                let mut cfg = loop_config.clone();
                cfg.force_unlock = loop_force.fetch_and(false, Ordering::AcqRel);
                match collect::run(cfg, Arc::clone(&loop_state)).await {
                    Ok(()) => info!("initial collection complete"),
                    Err(AppError::Persistence(PersistenceError::LockFailed { ref reason })) => {
                        warn!(reason = %reason, "initial collection skipped: lock held");
                    }
                    Err(e) => error!(error = %e, "initial collection failed — will retry"),
                }
            }

            // Scheduled loop.
            loop {
                tokio::time::sleep(Duration::from_secs(config::COLLECTION_INTERVAL_SECS)).await;
                let mut cfg = loop_config.clone();
                cfg.force_unlock = loop_force.load(Ordering::Acquire);
                match collect::run(cfg, Arc::clone(&loop_state)).await {
                    Ok(()) => {
                        info!("scheduled collection complete");
                    }
                    Err(AppError::Persistence(PersistenceError::LockFailed { ref reason })) => {
                        warn!(reason = %reason, "collection skipped: lock held");
                    }
                    Err(e) => error!(error = %e, "scheduled collection failed"),
                }
            }
        })
    };

    // ── Build extra routes ─────────────────────────────────────────
    // Status endpoint is always registered. Webhook route is conditional
    // on WEBHOOK_SECRET being set.
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

    // ── Step 5: Graceful shutdown ───────────────────────────────
    // Close the work queue so the worker pool drains remaining jobs
    // and exits cleanly. This must happen before aborting the
    // collection task to avoid losing in-flight webhook jobs.
    app_state.work_queue.close();

    // The collection task may already be shutting down cooperatively
    // via its own SIGTERM handler (collect.rs). abort() is a superset
    // that forcibly drops the task if it hasn't exited yet.
    collection_loop.abort();

    // The logging handler is a synchronous closure registered on
    // `app_state.bus` (no task), so there is nothing to abort here —
    // it stops being invoked as soon as nothing publishes to the bus.

    // Propagate server error after cleanup. The upstream server-error type
    // is collapsed at this boundary so the gh-report error chain does not
    // depend on the donor crate's enum shape.
    server_result.map_err(|e| crate::error::ServerError::Runtime(e.to_string()))?;

    info!("daemon stopped");
    Ok(())
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

        // Derive cycle-root CorrelationContext from the in-flight run.
        // Daemon delivery is *inside* a collect cycle (copernicus T1.e +
        // oracle bonus): collect::run stores RunMetadata into
        // state.current_run before dispatching jobs whose outcomes arrive
        // here, so current_run is Some during normal operation. The None
        // branch is defensive — outcome arriving between cycles is a
        // structural surprise we log but tolerate via Uuid::nil() fallback
        // (deterministic; parallels Inc 3's webhook fallback).
        //
        // Reuses Inc 2's RunMetadata::correlation_context() projection;
        // constructor is CorrelationContext::correlated(uuid_from_run_id),
        // identical to the collect-cycle root in collect.rs (same chain).
        //
        // Held in scope; consumed at Inc 5 by the 2 publish sites in the
        // `match outcome` block below when they route through publish_event.
        let corr_ctx = state.current_run.load().as_ref().as_ref().map_or_else(
            || {
                debug!("delivery_loop: outcome arrived with no in-flight run; using nil ctx");
                CorrelationContext::correlated(Uuid::nil())
            },
            RunMetadata::correlation_context,
        );

        // Upsert evidence.
        // Invariant: delivery_loop is the sole consumer of `rx`, so the
        // get-then-upsert sequence in the failure arm cannot race.
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

        // Count down batch tracker for sweep jobs (AD3).
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
    // Clone for the projection envelope (CHE-0048:R2 + BC-v2-2:
    // payload must carry the materialised state). RepositoryEvidence
    // wraps Repository in Arc, so Clone is shallow on the heaviest
    // field; boxed because the type is ~560 bytes and would
    // otherwise dominate the DomainEvent enum size. B6'.
    // M2.cd: direct-write to `store` deleted (W1). The
    // `RepoEvaluated` envelope published below carries the
    // evidence; `EvidenceProjection::apply` materialises it
    // into the read-model. CHE-0048:R2 sole-writer.
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
    // Insert failure evidence: synthetic record with Unknown
    // statuses. Prevents stale passing data from persisting.
    // B6': also carry the synthetic record on the
    // `RepoEvaluated` envelope so `EvidenceProjection` reflects
    // the failure state in the read model.
    // M2.cd: read existing evidence from projection (sole reader);
    // direct-write to `store` deleted (W2). The `RepoEvaluated`
    // failure envelope below carries the synthesised failure
    // evidence; `EvidenceProjection::apply` materialises it.
    let existing = state.lock_projection().get(&domain_key);
    let (repo_name, evidence_for_event) = if let Some(existing) = existing {
        let name = existing.repository.name.clone();
        let failure = collect::failure_evidence(
            &std::sync::Arc::new(existing.repository.clone()),
            &jiff::Timestamp::now().to_string(),
        );
        let failure_for_event = Box::new(failure);
        (name, Some(failure_for_event))
    } else {
        // No baseline repo — we lack the inputs to synthesise
        // RepositoryEvidence (no Repository handle). Emit
        // metadata-only; projection treats `None` as no-op.
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
}
