//! Server integration — delegates to [`crate::infra::server`].
//!
//! This module provides:
//!
//! - [`status_router`] — the `/api/v1/status` route (registered as an
//!   extra route, not an `infra::server` built-in).
//! - [`build_router`] — convenience wrapper that wires `AppState` to the
//!   generic server router with the status route included.
//!
//! Tests verify governance-specific behaviour (status payload with
//! `organization`, evidence-to-WebSocket pipeline) that cannot be tested
//! in the generic server module's domain-free test harness.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use tower_http::limit::RequestBodyLimitLayer;

use crate::app::state::AppState;

/// Build a [`Router`] fragment for the `/api/v1/status` endpoint.
///
/// Returns a router with a 1 KB body limit (defence-in-depth for a
/// GET-only endpoint). Meant to be merged as `extra_routes` into
/// [`crate::infra::server::server::build_router`] or
/// [`crate::infra::server::server::start`].
pub(crate) fn status_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/status", get(status))
        .layer(RequestBodyLimitLayer::new(1024))
}

/// `GET /api/v1/status` — returns current/last run metadata and uptime.
///
/// Authentication is enforced at the ingress layer (Cloud Run / reverse
/// proxy). The response contains only run metadata and uptime — no secrets
/// or credentials are exposed.
async fn status(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(state.status_payload())
}

/// Build the [`Router`] for `AppState` using the generic in-memory server.
///
/// Wires `AppState` (which implements [`crate::infra::server::state::ServerState`])
/// to the generic server router with the status endpoint registered as an
/// extra route.
///
/// # Panics
///
/// Panics if the default `ServerConfig` cannot be built (indicates a
/// programming error in the hardcoded defaults).
pub fn build_router(state: Arc<AppState>) -> Router {
    crate::infra::server::server::build_router(
        state,
        &crate::infra::server::config::ServerConfig::builder()
            .build()
            .expect("default config is valid"),
        Some(status_router()),
    )
}

// ===========================================================================
// Tests — governance-specific integration tests only
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    // ── Helper: wait for server to accept connections ────────────

    async fn wait_for_server(addr: std::net::SocketAddr) {
        let timeout = std::time::Duration::from_secs(5);
        tokio::time::timeout(timeout, async {
            let mut delay = std::time::Duration::from_millis(1);
            let cap = std::time::Duration::from_secs(1);
            loop {
                if tokio::net::TcpStream::connect(addr).await.is_ok() {
                    return;
                }
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(cap);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("server at {addr} did not become ready within {timeout:?}"));
    }

    fn state_no_cache() -> Arc<AppState> {
        AppState::new()
    }

    // ── Status endpoint with governance-specific fields ──────────

    #[tokio::test]
    async fn status_endpoint_returns_json() {
        let state = state_no_cache();
        let app = build_router(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/api/v1/status"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body.get("uptime_secs").is_some());
        assert!(body.get("current_run").is_some());
        assert!(body.get("last_completed_run").is_some());

        handle.abort();
    }

    #[tokio::test]
    async fn status_endpoint_valid_json_with_concurrent_run() {
        let state = state_no_cache();

        let run = crate::domain::run::RunMetadata::new(
            "TestOrg".into(),
            crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
        );
        state.current_run.store(Arc::new(Some(run)));

        let app = build_router(Arc::clone(&state));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/api/v1/status"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body["current_run"].is_object());
        assert_eq!(body["current_run"]["organization"], "TestOrg");
        assert!(body.get("uptime_secs").is_some());
        assert!(body.get("last_completed_run").is_some());

        handle.abort();
    }

    #[tokio::test]
    async fn status_reflects_completed_run() {
        let state = state_no_cache();

        let mut run = crate::domain::run::RunMetadata::new(
            "TestOrg".into(),
            crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
        );
        run.complete();
        state.last_completed_run.store(Arc::new(Some(run)));

        let app = build_router(Arc::clone(&state));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/api/v1/status"))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body["last_completed_run"].is_object());
        assert_eq!(body["last_completed_run"]["organization"], "TestOrg");
        assert_eq!(body["last_completed_run"]["status"], "completed");

        handle.abort();
    }

    // ── Readiness with governance-specific conditions ────────────

    #[tokio::test]
    async fn readyz_returns_200_after_completed_run() {
        let state = state_no_cache();

        let mut run = crate::domain::run::RunMetadata::new(
            "Org".into(),
            crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
        );
        run.complete();
        state.last_completed_run.store(Arc::new(Some(run)));

        let app = build_router(Arc::clone(&state));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/readyz")).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], "ready");

        handle.abort();
    }

    // ── WebSocket e2e (evidence → broadcast → client) ────────────

    #[tokio::test]
    async fn ws_e2e_publish_evidence_broadcasts_to_client() {
        use crate::app::collect::publish_evidence;
        use crate::config::runtime::RuntimeConfig;
        use crate::test_fixtures;
        use futures_util::StreamExt;

        let state = state_no_cache();
        let app = build_router(Arc::clone(&state));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        // Connect WS client.
        let url = format!("ws://{addr}/ws");
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let _ = ws.next().await; // consume "connected"

        // Call the real publish_evidence() function.
        let config = RuntimeConfig {
            org_name: "TestOrg".to_string(),
            no_resume: true,
            max_workers: 4,
            store_dir: std::path::PathBuf::from("/tmp/ws-e2e-test"),
            force_unlock: false,
            dashboard_config: crate::config::dashboard::DashboardConfig::default(),
        };
        let evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            test_fixtures::make_collection_statistics(1, 1, 0, 0),
            test_fixtures::make_minimal_metrics(),
            test_fixtures::make_observability(),
            vec![test_fixtures::all_passing_evidence("e2e-repo")],
        );
        let run = crate::domain::run::RunMetadata::new(
            "TestOrg".to_string(),
            crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
        );

        // Drive the Run aggregate through Started -> Completed so the
        // publish_evidence helper's call to run_service.publish_evidence
        // (post-Inc B7'c-4) finds the routing index entry. Without this,
        // resolve_id panics per the B7'c TODO at run_service.rs:316.
        let corr = run.correlation_context();
        state
            .run_service
            .start_sweep(
                crate::domain::aggregates::run::StartSweep {
                    org: "TestOrg".to_string(),
                    repo_count: 1,
                    batch_id: run.run_id.clone(),
                    timestamp: jiff::Timestamp::now().to_string(),
                },
                &corr,
            )
            .await
            .unwrap();
        state
            .run_service
            .complete(
                &run.run_id,
                crate::domain::aggregates::run::CompleteSweep {
                    batch_id: run.run_id.clone(),
                    duration_ms: 0,
                    repo_count: 1,
                    timestamp: jiff::Timestamp::now().to_string(),
                },
                &corr,
            )
            .await
            .unwrap();

        publish_evidence(&config, &run, &run.correlation_context(), &evidence, &state)
            .await
            .unwrap();

        // WS client should receive an update event with page keys.
        let timeout_result =
            tokio::time::timeout(std::time::Duration::from_secs(3), ws.next()).await;

        let msg = timeout_result
            .expect("should receive update within 3s")
            .expect("stream should have a message")
            .expect("message should be Ok");

        let text = msg.into_text().expect("expected text message").to_string();
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["type"], "update");

        let pages = parsed["pages"].as_array().expect("pages should be array");
        assert!(!pages.is_empty(), "pages should not be empty");
        let page_names: Vec<&str> = pages.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(
            page_names.contains(&"index.html"),
            "pages should contain index.html, got: {page_names:?}"
        );

        ws.close(None).await.ok();
        handle.abort();
    }
}
