//! Server integration — delegates to [`cherry_pit_web::serve`].
//!
//! This module provides:
//!
//! - [`status_router`] — the `/api/v1/status` route (registered as an
//!   extra route, not a `cherry_pit_web::serve` built-in).
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

/// CSP applied to the served (dashboard) `ServerConfig`, relaxing
/// `script-src` to permit `wasm-unsafe-eval` for the served WASM bundle.
pub(crate) const SERVED_CSP_WITH_WASM_UNSAFE_EVAL: &str = "default-src 'self'; style-src 'self'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; base-uri 'none'; form-action 'none'";

/// [`cherry_pit_web::serve::ValidatedConfig`] for the served-dashboard path:
/// applies [`SERVED_CSP_WITH_WASM_UNSAFE_EVAL`] on top of the defaults.
///
/// # Panics
///
/// Panics if the config fails to build (indicates a programming error in
/// the hardcoded defaults).
pub(crate) fn served_dashboard_server_config() -> cherry_pit_web::serve::ValidatedConfig {
    cherry_pit_web::serve::ServerConfig::builder()
        .csp_override(SERVED_CSP_WITH_WASM_UNSAFE_EVAL)
        .build()
        .expect("default config is valid")
}

/// Bare-default [`cherry_pit_web::serve::ValidatedConfig`] with no overrides.
///
/// # Panics
///
/// Panics if the config fails to build (indicates a programming error in
/// the hardcoded defaults).
pub(crate) fn default_server_config() -> cherry_pit_web::serve::ValidatedConfig {
    cherry_pit_web::serve::ServerConfig::builder()
        .build()
        .expect("default config is valid")
}

/// Build a [`Router`] fragment for the `/api/v1/status` endpoint.
///
/// Returns a router with a 1 KB body limit (defence-in-depth for a
/// GET-only endpoint). Meant to be merged as `extra_routes` into
/// [`cherry_pit_web::serve::build_router`] or
/// [`cherry_pit_web::serve::start`].
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
/// Wires `AppState` (which implements [`cherry_pit_web::serve::ServerState`])
/// to the generic server router with the status endpoint registered as an
/// extra route.
///
/// # Panics
///
/// Panics if the default `ServerConfig` cannot be built (indicates a
/// programming error in the hardcoded defaults).
pub fn build_router(state: Arc<AppState>) -> Router {
    let config = default_server_config();
    cherry_pit_web::serve::build_router(state, &config, Some(status_router()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn served_dashboard_server_config_carries_the_relaxed_csp() {
        let config = served_dashboard_server_config();
        assert_eq!(
            config.csp_override(),
            Some(SERVED_CSP_WITH_WASM_UNSAFE_EVAL)
        );
    }

    #[test]
    fn default_server_config_has_no_csp_override() {
        let config = default_server_config();
        assert!(config.csp_override().is_none());
    }

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

    async fn state_no_cache() -> Arc<AppState> {
        AppState::new().await
    }

    #[tokio::test]
    async fn status_endpoint_returns_json() {
        let state = state_no_cache().await;
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
    async fn status_endpoint_exposes_memory_gauges() {
        use crate::config::runtime::{NatsStoreConfig, PardosaBackend};
        use crate::test_fixtures;

        let dir = tempfile::tempdir().unwrap();
        let events_dir = dir.path().join("events");
        let nats =
            NatsStoreConfig::for_org("MemGaugeOrg", crate::config::runtime::DEFAULT_NATS_URL)
                .unwrap();
        let writer_state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats.clone())
            .await
            .unwrap();
        let timestamp = "2026-06-14T00:00:00Z";
        for name in ["gauge-repo-a", "gauge-repo-b"] {
            let evidence = test_fixtures::all_passing_evidence(name);
            writer_state
                .record_repo(
                    &evidence.repository.inventory_key,
                    evidence.clone(),
                    &evidence.repository.name,
                    timestamp,
                )
                .unwrap();
        }
        drop(writer_state);

        let state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats)
            .await
            .unwrap();
        assert_eq!(state.projection_len(), 2);

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

        assert_eq!(body["projection_repo_count"], serde_json::json!(2));
        assert_eq!(
            body["projection_bytes_est"],
            serde_json::json!(
                2 * std::mem::size_of::<crate::domain::evidence::RepositoryEvidence>()
            )
        );

        #[cfg(target_os = "linux")]
        assert!(
            body["rss_kb"].as_u64().is_some_and(|kb| kb > 0),
            "rss_kb must be a positive integer on linux: {:?}",
            body["rss_kb"]
        );
        #[cfg(not(target_os = "linux"))]
        assert!(
            body["rss_kb"].is_null(),
            "rss_kb must be null off-linux: {:?}",
            body["rss_kb"]
        );

        handle.abort();
    }

    #[tokio::test]
    async fn status_endpoint_valid_json_with_concurrent_run() {
        let state = state_no_cache().await;

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
        let state = state_no_cache().await;

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

    #[tokio::test]
    async fn readyz_returns_200_after_completed_run() {
        let state = state_no_cache().await;

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

    #[tokio::test]
    async fn admin_html_serves_from_dashboard_cache_read_only() {
        use cherry_pit_web::serve::CachedPage;
        use std::collections::HashMap;

        let state = state_no_cache().await;
        let mut pages = HashMap::new();
        pages.insert(
            "admin.html".to_string(),
            CachedPage::new("admin.html", b"<html>Admin Diagnostics</html>".to_vec()),
        );
        state.set_html_cache(pages);

        let app = build_router(Arc::clone(&state));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let get_resp = reqwest::get(format!("http://{addr}/admin.html"))
            .await
            .unwrap();
        assert_eq!(get_resp.status(), 200);
        assert_eq!(
            get_resp.text().await.unwrap(),
            "<html>Admin Diagnostics</html>"
        );

        let post_resp = reqwest::Client::new()
            .post(format!("http://{addr}/admin.html"))
            .send()
            .await
            .unwrap();
        assert_eq!(post_resp.status(), 405);

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn coldstart_projection_serves_200_without_github_api() {
        use crate::app::collect::warm_start_from_baseline;
        use crate::config::dashboard::DashboardConfig;
        use crate::config::runtime::{NatsStoreConfig, PardosaBackend, RuntimeConfig};
        use crate::test_fixtures;
        use cherry_pit_web::serve::ServerState;

        let dir = tempfile::tempdir().unwrap();
        let events_dir = dir.path().join("events");
        let nats =
            NatsStoreConfig::for_org("TestOrg", crate::config::runtime::DEFAULT_NATS_URL).unwrap();
        let writer_state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats.clone())
            .await
            .unwrap();
        let timestamp = "2026-06-14T00:00:00Z";
        let active = test_fixtures::all_passing_evidence("active-repo");
        let mut archived = test_fixtures::all_passing_evidence("archived-repo");
        archived.repository.archived = true;

        for evidence in [active, archived] {
            writer_state
                .record_repo(
                    &evidence.repository.inventory_key,
                    evidence.clone(),
                    &evidence.repository.name,
                    timestamp,
                )
                .unwrap();
        }
        drop(writer_state);

        let state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats)
            .await
            .unwrap();
        assert_eq!(state.projection_len(), 2);
        assert!(state.github_client().is_none());
        assert!(
            state.is_ready(),
            "populated event-log projection should be ready without run/cache or GitHub API"
        );

        let config = RuntimeConfig {
            org_name: "TestOrg".to_string(),
            no_resume: true,
            max_workers: 4,
            store_dir: dir.path().to_path_buf(),
            pardosa_backend: PardosaBackend::Pgno,
            nats_url: crate::config::runtime::DEFAULT_NATS_URL.to_string(),
            nats_creds: None,
            force_unlock: false,
            force_refresh: false,
            dashboard_config: DashboardConfig::default(),
            team_roster_read_from_projection: true,
        };

        assert!(warm_start_from_baseline(&config, &state).await);
        assert!(state.github_client().is_none());

        let app = build_router(Arc::clone(&state));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/")).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = resp.text().await.unwrap();
        assert!(body.contains("1 non-archived"));
        assert!(body.contains("1 archived repositories"));

        handle.abort();
    }

    #[tokio::test]
    async fn readyz_returns_503_when_cache_warm_but_backend_connect_failed() {
        use cherry_pit_web::serve::CachedPage;
        use std::collections::HashMap;

        let state = state_no_cache().await;
        let mut pages = HashMap::new();
        pages.insert(
            "index.html".to_string(),
            CachedPage::new("index.html", b"<html>cached</html>".to_vec()),
        );
        state.set_html_cache(pages);
        state.event_store.mark_backend_connect_failure_for_test();

        let app = build_router(Arc::clone(&state));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let resp = reqwest::get(format!("http://{addr}/readyz")).await.unwrap();
        assert_eq!(resp.status(), 503);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], "not_ready");

        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ws_e2e_publish_evidence_broadcasts_to_client() {
        use crate::app::collect::publish_evidence;
        use crate::config::runtime::RuntimeConfig;
        use crate::test_fixtures;
        use futures_util::StreamExt;

        let state = state_no_cache().await;
        let app = build_router(Arc::clone(&state));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        wait_for_server(addr).await;

        let url = format!("ws://{addr}/ws");
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let _ = ws.next().await;

        let config = RuntimeConfig {
            org_name: "TestOrg".to_string(),
            no_resume: true,
            max_workers: 4,
            store_dir: std::path::PathBuf::from("/tmp/ws-e2e-test"),
            pardosa_backend: crate::config::runtime::PardosaBackend::Pgno,
            nats_url: crate::config::runtime::DEFAULT_NATS_URL.to_string(),
            nats_creds: None,
            force_unlock: false,
            force_refresh: false,
            dashboard_config: crate::config::dashboard::DashboardConfig::default(),
            team_roster_read_from_projection: true,
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

        publish_evidence(&config, &run, &evidence, &state)
            .await
            .unwrap();

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
