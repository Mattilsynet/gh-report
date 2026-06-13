//! GitHub webhook receiver.
//!
//! Provides a `POST /webhook` endpoint that validates HMAC signatures,
//! maps events to jobs, and enqueues them via the existing [`WorkQueue`].
//!
//! The route is registered conditionally: only when `WEBHOOK_SECRET` is
//! set in the environment. When disabled, the route is not registered
//! and the fallback handler returns 404.
//!
//! ## Handler flow
//!
//! 1. Validate `X-Hub-Signature-256` header (HMAC-SHA256, constant-time)
//! 2. Extract `X-GitHub-Event` and `X-GitHub-Delivery` headers
//! 3. Replay protection via `replay_cache` (moka, 100k cap, 1h TTL)
//! 4. Debounce check (push events only, per-repo, 5s window)
//! 5. Map event to `WebhookAction` (Enqueue / Remove / Ignore)
//! 6. Execute action and return appropriate HTTP status

pub mod events;
pub mod signature;

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use cherry_pit_core::CorrelationContext;
use tower_http::limit::RequestBodyLimitLayer;
use tracing::{debug, info, warn};

use crate::app::collect::JobContext;
use crate::app::state::AppState;
use crate::app::work_queue::{EnqueueResult, JobSource, JobSpec};
use crate::config;
use crate::domain::repository::Repository;

use self::events::{WebhookAction, map_event_to_action};
use self::signature::verify_signature;

/// Build the webhook router with a 1 MB body limit.
///
/// Returns a `Router<Arc<AppState>>` to be merged as `extra_routes`
/// into the generic in-memory server. The body limit is applied per-route so
/// built-in read-only routes retain their 1 KB limit.
pub fn webhook_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/webhook", post(webhook_handler))
        .route_layer(RequestBodyLimitLayer::new(config::MAX_WEBHOOK_BODY_BYTES))
}

/// `POST /webhook` — GitHub webhook receiver.
///
/// Validates the HMAC-SHA256 signature, maps the event to a job, and
/// enqueues it via the work queue. See module-level docs for the full
/// handler flow and HTTP response code table.
async fn webhook_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let (event_type, delivery_id) = match validate_request(&state, &headers, &body) {
        Ok(parts) => parts,
        Err(status) => return status,
    };

    let action = match map_event_to_action(&event_type, &body, &state) {
        Ok(action) => action,
        Err(e) => {
            warn!(
                event = %event_type,
                delivery = %delivery_id,
                err = %e,
                "webhook parse error"
            );
            return StatusCode::BAD_REQUEST;
        }
    };

    let entry = state
        .webhook()
        .replay_cache
        .entry(delivery_id.clone())
        .or_insert(())
        .await;
    if !entry.is_fresh() {
        debug!(delivery = %delivery_id, "replay detected, idempotent skip");
        return StatusCode::OK;
    }

    match action {
        WebhookAction::Remove { inventory_key } => {
            execute_remove(&state, &event_type, &delivery_id, &inventory_key)
        }
        WebhookAction::Enqueue {
            inventory_key,
            repo,
        } => {
            info!(delivery = %delivery_id, repo = %repo.name, "webhook enqueue");
            execute_enqueue(&state, &event_type, &delivery_id, inventory_key, repo).await
        }
        WebhookAction::Ignore => execute_ignore(&event_type, &delivery_id),
    }
}

/// Validate HMAC signature, extract event/delivery headers, build correlation ctx.
///
/// Returns `Err(StatusCode)` on any validation failure (`NOT_FOUND` if secret
/// is unconfigured, `UNAUTHORIZED` on missing/bad signature, `BAD_REQUEST` on
/// missing headers). Extracted from [`webhook_handler`] for cohesion; no
/// behavioural change.
fn validate_request(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<(String, String), StatusCode> {
    let Some(ref secret) = state.webhook().secret else {
        return Err(StatusCode::NOT_FOUND);
    };

    let Some(signature) = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
    else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    if !verify_signature(secret, body, signature) {
        warn!("webhook HMAC signature validation failed");
        return Err(StatusCode::UNAUTHORIZED);
    }

    let Some(event_type) = headers.get("x-github-event").and_then(|v| v.to_str().ok()) else {
        return Err(StatusCode::BAD_REQUEST);
    };

    let Some(delivery_id) = headers
        .get("x-github-delivery")
        .and_then(|v| v.to_str().ok())
    else {
        return Err(StatusCode::BAD_REQUEST);
    };

    Ok((event_type.to_string(), delivery_id.to_string()))
}

/// Execute the remove action: publish `WebhookReceived` + (conditionally) `RepoRemoved`.
///
/// Extracted from [`webhook_handler`] for cohesion; no behavioural change.
fn execute_remove(
    state: &Arc<AppState>,
    event_type: &str,
    delivery_id: &str,
    inventory_key: &str,
) -> StatusCode {
    let had_evidence = state.projection_contains(inventory_key);
    info!(delivery = %delivery_id, repo = %inventory_key, "webhook remove");
    if had_evidence
        && let Err(e) = state.remove_repo(
            inventory_key,
            inventory_key,
            &jiff::Timestamp::now().to_string(),
        )
    {
        tracing::warn!(?e, "repository removal failed, non-fatal");
    }
    info!(
        event = event_type,
        delivery = delivery_id,
        key = %inventory_key,
        had_evidence = had_evidence,
        "webhook: repository removed from evidence store"
    );
    StatusCode::OK
}

/// Execute the ignore action: publish `WebhookReceived` with `action=ignore`.
///
/// Extracted from [`webhook_handler`] for cohesion; no behavioural change.
fn execute_ignore(event_type: &str, delivery_id: &str) -> StatusCode {
    debug!(
        event = event_type,
        delivery = delivery_id,
        "webhook event ignored"
    );
    StatusCode::OK
}

/// Execute the enqueue action: debounce check (push only), then submit job.
async fn execute_enqueue(
    state: &AppState,
    event_type: &str,
    delivery_id: &str,
    inventory_key: String,
    repo: Arc<Repository>,
) -> StatusCode {
    if event_type == "push" {
        let now = tokio::time::Instant::now();
        if let Some(last) = state.webhook().debounce_cache.get(&inventory_key).await
            && now.duration_since(last).as_secs() < config::DEFAULT_WEBHOOK_DEBOUNCE_SECS
        {
            debug!(
                event = event_type,
                delivery = %delivery_id,
                key = %inventory_key,
                "webhook debounced (push within window)"
            );
            return StatusCode::OK;
        }
        state
            .webhook()
            .debounce_cache
            .insert(inventory_key.clone(), now)
            .await;
    }

    let job = JobSpec::new(
        inventory_key.clone(),
        JobContext {
            repo,
            run_timestamp: jiff::Timestamp::now().to_string(),
        },
        JobSource::External {
            id: delivery_id.to_string(),
            kind: event_type.to_string(),
        },
        CorrelationContext::none(),
    );

    match state.work_queue.enqueue(job) {
        EnqueueResult::Accepted => {
            info!(
                event = event_type,
                delivery = %delivery_id,
                key = %inventory_key,
                "webhook job enqueued"
            );
            StatusCode::ACCEPTED
        }
        EnqueueResult::Deduplicated => {
            info!(
                event = event_type,
                delivery = %delivery_id,
                key = %inventory_key,
                "webhook job deduplicated"
            );
            StatusCode::OK
        }
        EnqueueResult::QueueFull => {
            warn!(
                event = event_type,
                delivery = %delivery_id,
                key = %inventory_key,
                "webhook queue full"
            );
            StatusCode::SERVICE_UNAVAILABLE
        }
        other => {
            warn!(
                event = event_type,
                delivery = %delivery_id,
                key = %inventory_key,
                result = ?other,
                "webhook: unexpected EnqueueResult variant"
            );
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use hmac::digest::KeyInit;
    use hmac::{Hmac, Mac};
    use secrecy::ExposeSecret;
    use sha2::Sha256;
    use tower::ServiceExt;

    /// Compute HMAC-SHA256 signature for test payloads.
    fn sign(secret: &str, body: &[u8]) -> String {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key size");
        mac.update(body);
        let result = mac.finalize();
        let hex: String = {
            use std::fmt::Write;
            result
                .into_bytes()
                .iter()
                .fold(String::with_capacity(64), |mut s, b| {
                    let _ = write!(s, "{b:02x}");
                    s
                })
        };
        format!("sha256={hex}")
    }

    /// Build a test `AppState` with a known webhook secret.
    async fn test_state_with_secret(secret: &str) -> Arc<AppState> {
        AppState::new_with_webhook_secret(secret).await
    }

    fn build_test_app(state: Arc<AppState>) -> Router {
        let extra = webhook_router();
        crate::infra::server::runtime::build_router(
            state,
            &crate::infra::server::config::ServerConfig::builder()
                .build()
                .expect("default config is valid"),
            Some(extra),
        )
    }

    #[tokio::test]
    async fn webhook_invalid_hmac() {
        let state = test_state_with_secret("test-secret").await;
        let app = build_test_app(state);

        let body = b"{}";
        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("x-hub-signature-256", "sha256=deadbeef")
            .header("x-github-event", "push")
            .header("x-github-delivery", "test-delivery-1")
            .body(Body::from(body.as_slice()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn webhook_missing_signature() {
        let state = test_state_with_secret("test-secret").await;
        let app = build_test_app(state);

        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("x-github-event", "push")
            .header("x-github-delivery", "test-delivery-2")
            .body(Body::from("{}"))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn webhook_missing_event_header() {
        let state = test_state_with_secret("test-secret").await;
        let secret = state
            .webhook()
            .secret
            .as_ref()
            .unwrap()
            .expose_secret()
            .to_string();
        let app = build_test_app(state);

        let body = b"{}";
        let sig = sign(&secret, body);

        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("x-hub-signature-256", sig)
            .header("x-github-delivery", "test-delivery-3")
            .body(Body::from(body.as_slice()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn webhook_missing_delivery_header() {
        let state = test_state_with_secret("test-secret").await;
        let secret = state
            .webhook()
            .secret
            .as_ref()
            .unwrap()
            .expose_secret()
            .to_string();
        let app = build_test_app(state);

        let body = b"{}";
        let sig = sign(&secret, body);

        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("x-hub-signature-256", sig)
            .header("x-github-event", "push")
            .body(Body::from(body.as_slice()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn webhook_valid_enqueue() {
        let state = test_state_with_secret("test-secret").await;
        let secret = state
            .webhook()
            .secret
            .as_ref()
            .unwrap()
            .expose_secret()
            .to_string();
        let app = build_test_app(Arc::clone(&state));

        let body = serde_json::json!({
            "action": "disabled",
            "repository": { "id": 12345, "name": "test-repo" }
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let sig = sign(&secret, &body_bytes);

        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("x-hub-signature-256", sig)
            .header("x-github-event", "branch_protection_configuration")
            .header("x-github-delivery", "delivery-valid-1")
            .body(Body::from(body_bytes))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn webhook_replay_duplicate() {
        let state = test_state_with_secret("test-secret").await;
        let secret = state
            .webhook()
            .secret
            .as_ref()
            .unwrap()
            .expose_secret()
            .to_string();

        let body = serde_json::json!({
            "action": "disabled",
            "repository": { "id": 99999, "name": "replay-repo" }
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let sig = sign(&secret, &body_bytes);

        let app1 = build_test_app(Arc::clone(&state));
        let request1 = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("x-hub-signature-256", &sig)
            .header("x-github-event", "branch_protection_configuration")
            .header("x-github-delivery", "delivery-replay-1")
            .body(Body::from(body_bytes.clone()))
            .unwrap();
        let response1 = app1.oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::ACCEPTED);

        let app2 = build_test_app(Arc::clone(&state));
        let request2 = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("x-hub-signature-256", &sig)
            .header("x-github-event", "branch_protection_configuration")
            .header("x-github-delivery", "delivery-replay-1")
            .body(Body::from(body_bytes))
            .unwrap();
        let response2 = app2.oneshot(request2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn webhook_malformed_json() {
        let state = test_state_with_secret("test-secret").await;
        let secret = state
            .webhook()
            .secret
            .as_ref()
            .unwrap()
            .expose_secret()
            .to_string();
        let app = build_test_app(state);

        let body = b"not valid json {{{";
        let sig = sign(&secret, body);

        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("x-hub-signature-256", sig)
            .header("x-github-event", "dependabot_alert")
            .header("x-github-delivery", "delivery-malformed-1")
            .body(Body::from(body.as_slice()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn webhook_ignored_event_returns_200() {
        let state = test_state_with_secret("test-secret").await;
        let secret = state
            .webhook()
            .secret
            .as_ref()
            .unwrap()
            .expose_secret()
            .to_string();
        let app = build_test_app(state);

        let body = b"{}";
        let sig = sign(&secret, body);

        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("x-hub-signature-256", sig)
            .header("x-github-event", "star")
            .header("x-github-delivery", "delivery-ignore-1")
            .body(Body::from(body.as_slice()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn dedup_returns_200_not_503() {
        let state = test_state_with_secret("test-secret").await;
        let secret = state
            .webhook()
            .secret
            .as_ref()
            .unwrap()
            .expose_secret()
            .to_string();

        let body = serde_json::json!({
            "action": "created",
            "repository": { "id": 77777, "name": "dedup-repo" }
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let sig = sign(&secret, &body_bytes);

        let app1 = build_test_app(Arc::clone(&state));
        let request1 = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("x-hub-signature-256", &sig)
            .header("x-github-event", "dependabot_alert")
            .header("x-github-delivery", "delivery-dedup-1")
            .body(Body::from(body_bytes.clone()))
            .unwrap();
        let response1 = app1.oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::ACCEPTED);

        let sig2 = sign(&secret, &body_bytes);
        let app2 = build_test_app(Arc::clone(&state));
        let request2 = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("x-hub-signature-256", &sig2)
            .header("x-github-event", "dependabot_alert")
            .header("x-github-delivery", "delivery-dedup-2")
            .body(Body::from(body_bytes))
            .unwrap();
        let response2 = app2.oneshot(request2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn webhook_repository_deleted_returns_200() {
        let state = test_state_with_secret("test-secret").await;
        let secret = state
            .webhook()
            .secret
            .as_ref()
            .unwrap()
            .expose_secret()
            .to_string();
        let app = build_test_app(state);

        let body = serde_json::json!({
            "action": "deleted",
            "repository": { "id": 11111, "name": "deleted-repo" }
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let sig = sign(&secret, &body_bytes);

        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("x-hub-signature-256", sig)
            .header("x-github-event", "repository")
            .header("x-github-delivery", "delivery-deleted-1")
            .body(Body::from(body_bytes))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn webhook_body_over_1mb_returns_413() {
        let state = test_state_with_secret("test-secret").await;
        let secret = state
            .webhook()
            .secret
            .as_ref()
            .unwrap()
            .expose_secret()
            .to_string();
        let app = build_test_app(state);

        let body = vec![b'{'; config::MAX_WEBHOOK_BODY_BYTES + 1];
        let sig = sign(&secret, &body);

        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("x-hub-signature-256", sig)
            .header("x-github-event", "push")
            .header("x-github-delivery", "delivery-big-body-1")
            .body(Body::from(body))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn webhook_push_debounce_within_window() {
        let state = test_state_with_secret("test-secret").await;
        let secret = state
            .webhook()
            .secret
            .as_ref()
            .unwrap()
            .expose_secret()
            .to_string();

        let body = serde_json::json!({
            "ref": "refs/heads/main",
            "repository": {
                "id": 55555,
                "name": "debounce-repo",
                "default_branch": "main"
            },
            "commits": [{
                "added": [],
                "modified": ["SECURITY.md"],
                "removed": []
            }]
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let sig = sign(&secret, &body_bytes);

        let app1 = build_test_app(Arc::clone(&state));
        let request1 = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("x-hub-signature-256", &sig)
            .header("x-github-event", "push")
            .header("x-github-delivery", "delivery-debounce-1")
            .body(Body::from(body_bytes.clone()))
            .unwrap();
        let response1 = app1.oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::ACCEPTED);

        let sig2 = sign(&secret, &body_bytes);
        let app2 = build_test_app(Arc::clone(&state));
        let request2 = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("x-hub-signature-256", &sig2)
            .header("x-github-event", "push")
            .header("x-github-delivery", "delivery-debounce-2")
            .body(Body::from(body_bytes))
            .unwrap();
        let response2 = app2.oneshot(request2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::OK);
    }
}
