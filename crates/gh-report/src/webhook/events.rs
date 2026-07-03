//! Webhook event mapping and payload types.
//!
//! Maps GitHub webhook events to [`WebhookAction`] values: `Enqueue` (create
//! a job for re-evaluation), `Remove` (delete from evidence store), or
//! `Ignore` (irrelevant event). Push events are filtered by
//! [`is_security_relevant_push`] to avoid unnecessary API calls for
//! non-security-related commits.

use std::fmt;
use std::sync::Arc;

use tracing::debug;

use crate::app::state::AppState;
use crate::domain::repository::{Repository, Visibility};

/// Errors that can occur when mapping a webhook event to an action.
#[derive(Debug)]
#[non_exhaustive]
pub enum WebhookError {
    /// The JSON payload could not be parsed.
    InvalidJson(serde_json::Error),
    /// The payload is missing a required `repository` field.
    MissingRepository,
}

impl fmt::Display for WebhookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(e) => write!(f, "invalid JSON: {e}"),
            Self::MissingRepository => f.write_str("missing repository field"),
        }
    }
}

impl std::error::Error for WebhookError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidJson(e) => Some(e),
            Self::MissingRepository => None,
        }
    }
}

impl From<serde_json::Error> for WebhookError {
    fn from(e: serde_json::Error) -> Self {
        Self::InvalidJson(e)
    }
}

/// Minimal webhook payload — just the repository field present in most events.
#[derive(Debug, serde::Deserialize)]
pub struct WebhookPayload {
    /// The repository the event pertains to.
    pub repository: Option<WebhookRepository>,
    /// Action string (e.g., "created", "deleted", "disabled").
    pub action: Option<String>,
}

/// Repository information from webhook payloads.
#[derive(Debug, serde::Deserialize)]
pub struct WebhookRepository {
    /// Numeric repository ID.
    pub id: u64,
    /// Node ID (GraphQL).
    pub node_id: Option<String>,
    /// Repository name (not fully qualified).
    pub name: String,
    /// Full name (e.g., `"org/repo"`).
    pub full_name: Option<String>,
    /// Visibility: "public", "private", or "internal".
    pub visibility: Option<String>,
    /// Default branch name.
    pub default_branch: Option<String>,
    /// Whether the repository is archived.
    #[serde(default)]
    pub archived: bool,
    /// HTML URL.
    pub html_url: Option<String>,
}

/// Push event payload (subset of fields we need).
#[derive(Debug, serde::Deserialize)]
pub struct PushEvent {
    /// Git ref (e.g., `"refs/heads/main"`).
    #[serde(rename = "ref")]
    pub ref_field: String,
    /// Repository information.
    pub repository: PushRepository,
    /// Commits included in this push.
    #[serde(default)]
    pub commits: Vec<PushCommit>,
}

/// Repository info from push events (includes `default_branch` inline).
#[derive(Debug, serde::Deserialize)]
pub struct PushRepository {
    /// Numeric repository ID.
    pub id: u64,
    /// Repository name.
    pub name: String,
    /// Default branch name.
    #[serde(default = "default_branch_fallback")]
    pub default_branch: String,
}

fn default_branch_fallback() -> String {
    "main".to_string()
}

/// A single commit in a push event.
#[derive(Debug, serde::Deserialize)]
pub struct PushCommit {
    /// Files added in this commit.
    #[serde(default)]
    pub added: Vec<String>,
    /// Files modified in this commit.
    #[serde(default)]
    pub modified: Vec<String>,
    /// Files removed in this commit.
    #[serde(default)]
    pub removed: Vec<String>,
}

/// The action the webhook handler should take for a mapped event.
#[non_exhaustive]
pub enum WebhookAction {
    /// Enqueue a job for re-evaluation. Contains the inventory key and
    /// the `Arc<Repository>` for constructing a `JobContext`.
    Enqueue {
        inventory_key: String,
        repo: Arc<Repository>,
    },
    /// Remove the repository from the evidence store (lifecycle event).
    Remove { inventory_key: String },
    /// Event is irrelevant — return 200 OK.
    Ignore,
}

/// Paths that indicate a security-relevant file change.
const SECURITY_PATHS: &[&str] = &["SECURITY.md", ".github/SECURITY.md", "docs/SECURITY.md"];

/// Paths for CODEOWNERS file changes.
const CODEOWNERS_PATHS: &[&str] = &[".github/CODEOWNERS", "CODEOWNERS"];

/// Check if a push event touches security-relevant files on the default branch.
///
/// Returns `true` only if:
/// 1. The push targets the default branch
/// 2. At least one commit touches `SECURITY.md` or `CODEOWNERS`
#[must_use]
pub fn is_security_relevant_push(payload: &PushEvent) -> bool {
    let Some(ref_name) = payload.ref_field.strip_prefix("refs/heads/") else {
        return false;
    };
    if ref_name != payload.repository.default_branch {
        return false;
    }

    payload.commits.iter().any(|commit| {
        commit
            .added
            .iter()
            .chain(commit.modified.iter())
            .chain(commit.removed.iter())
            .any(|file| {
                SECURITY_PATHS.iter().any(|p| file == p)
                    || CODEOWNERS_PATHS.iter().any(|p| file == p)
            })
    })
}

/// Build a minimal [`Repository`] from a webhook payload.
///
/// Used when the repository is not yet in the evidence store (e.g.,
/// `repository.created`) or when evidence store lookup fails.
#[must_use]
pub fn build_repository_from_payload(payload: &WebhookRepository) -> Arc<Repository> {
    let visibility = match payload.visibility.as_deref() {
        Some("public") => Visibility::Public,
        Some("internal") => Visibility::Internal,
        _ => Visibility::Private,
    };

    Arc::new(Repository {
        id: payload.id.to_string(),
        node_id: payload.node_id.clone(),
        name: payload.name.clone(),
        visibility,
        language: None,
        default_branch: payload
            .default_branch
            .clone()
            .unwrap_or_else(|| "main".to_string()),
        archived: payload.archived,
        inventory_key: payload.id.to_string(),
        updated_at: None,
        has_issues: false,
        pushed_at: None,
        created_at: None,
        description: None,
        fork: false,
        html_url: payload.html_url.clone(),
        topics: Vec::new(),
        license_spdx: None,
    })
}

/// Map a webhook event type and body to a [`WebhookAction`].
///
/// Uses the evidence store for repository resolution. Falls back to
/// constructing a `Repository` from the payload for new repos.
///
/// # Errors
///
/// Returns [`WebhookError`] if the JSON payload cannot be parsed or
/// is missing a required `repository` field.
pub fn map_event_to_action(
    event_type: &str,
    body: &[u8],
    state: &AppState,
) -> Result<WebhookAction, WebhookError> {
    match event_type {
        "repository" => map_repository_event(body, state),
        "branch_protection_configuration"
        | "branch_protection_rule"
        | "dependabot_alert"
        | "code_scanning_alert"
        | "secret_scanning_alert"
        | "member" => map_generic_enqueue_event(body, state),
        "push" => map_push_event(body, state),
        _ => {
            debug!(event = event_type, "ignoring unhandled webhook event type");
            Ok(WebhookAction::Ignore)
        }
    }
}

/// Map a `repository` event. `deleted`/`archived` → Remove, others → Enqueue.
fn map_repository_event(body: &[u8], state: &AppState) -> Result<WebhookAction, WebhookError> {
    let payload: WebhookPayload = serde_json::from_slice(body)?;

    let action = payload.action.as_deref().unwrap_or("");
    let repo_payload = payload
        .repository
        .as_ref()
        .ok_or(WebhookError::MissingRepository)?;

    let inventory_key = repo_payload.id.to_string();

    match action {
        "deleted" | "archived" => Ok(WebhookAction::Remove { inventory_key }),
        "created" | "unarchived" | "publicized" | "privatized" => {
            let repo = resolve_or_build_repo(&inventory_key, repo_payload, state);
            Ok(WebhookAction::Enqueue {
                inventory_key,
                repo,
            })
        }
        _ => {
            debug!(action, "ignoring repository event action");
            Ok(WebhookAction::Ignore)
        }
    }
}

/// Map a push event — filter for security-relevant changes only.
fn map_push_event(body: &[u8], state: &AppState) -> Result<WebhookAction, WebhookError> {
    let payload: PushEvent = serde_json::from_slice(body)?;

    if !is_security_relevant_push(&payload) {
        return Ok(WebhookAction::Ignore);
    }

    let inventory_key = payload.repository.id.to_string();
    let repo = resolve_repo_from_store(&inventory_key, &payload.repository.name, state);
    Ok(WebhookAction::Enqueue {
        inventory_key,
        repo,
    })
}

/// Map a generic event that always results in Enqueue (alerts, BP, member).
fn map_generic_enqueue_event(body: &[u8], state: &AppState) -> Result<WebhookAction, WebhookError> {
    let payload: WebhookPayload = serde_json::from_slice(body)?;

    let repo_payload = payload
        .repository
        .as_ref()
        .ok_or(WebhookError::MissingRepository)?;

    let inventory_key = repo_payload.id.to_string();
    let repo = resolve_or_build_repo(&inventory_key, repo_payload, state);
    Ok(WebhookAction::Enqueue {
        inventory_key,
        repo,
    })
}

/// Resolve `Arc<Repository>` from evidence store, or build from webhook payload.
fn resolve_or_build_repo(
    inventory_key: &str,
    payload: &WebhookRepository,
    state: &AppState,
) -> Arc<Repository> {
    if let Some(evidence) = state.projection_get(inventory_key) {
        Arc::new(evidence.repository)
    } else {
        build_repository_from_payload(payload)
    }
}

/// Resolve `Arc<Repository>` from evidence store for push events.
/// Falls back to a minimal Repository with just id and name.
fn resolve_repo_from_store(
    inventory_key: &str,
    repo_name: &str,
    state: &AppState,
) -> Arc<Repository> {
    if let Some(evidence) = state.projection_get(inventory_key) {
        Arc::new(evidence.repository)
    } else {
        Arc::new(Repository {
            id: inventory_key.to_string(),
            node_id: None,
            name: repo_name.to_string(),
            visibility: Visibility::Private,
            language: None,
            default_branch: "main".to_string(),
            archived: false,
            inventory_key: inventory_key.to_string(),
            updated_at: None,
            has_issues: false,
            pushed_at: None,
            created_at: None,
            description: None,
            fork: false,
            html_url: None,
            topics: Vec::new(),
            license_spdx: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::AppState;
    use std::error::Error;

    async fn test_state() -> Arc<AppState> {
        AppState::new().await
    }

    #[tokio::test]
    async fn event_repository_deleted() {
        let state = test_state().await;
        let body = serde_json::json!({
            "action": "deleted",
            "repository": { "id": 123, "name": "test-repo" }
        });
        let result = map_event_to_action("repository", body.to_string().as_bytes(), &state);
        match result {
            Ok(WebhookAction::Remove { inventory_key }) => {
                assert_eq!(inventory_key, "123");
            }
            other => panic!(
                "expected Remove, got {other:?}",
                other = std::mem::discriminant(&other.unwrap())
            ),
        }
    }

    #[tokio::test]
    async fn event_repository_created() {
        let state = test_state().await;
        let body = serde_json::json!({
            "action": "created",
            "repository": {
                "id": 456,
                "name": "new-repo",
                "visibility": "public",
                "default_branch": "main"
            }
        });
        let result =
            map_event_to_action("repository", body.to_string().as_bytes(), &state).unwrap();
        assert!(matches!(result, WebhookAction::Enqueue { .. }));
    }

    #[tokio::test]
    async fn event_repository_deleted_absent_key() {
        let state = test_state().await;
        let key = "nonexistent";
        let evidence = state.projection_get(key);
        assert!(evidence.is_none());
    }

    #[tokio::test]
    async fn event_branch_protection_disabled() {
        let state = test_state().await;
        let body = serde_json::json!({
            "action": "disabled",
            "repository": { "id": 789, "name": "bp-repo" }
        });
        let result = map_event_to_action(
            "branch_protection_configuration",
            body.to_string().as_bytes(),
            &state,
        )
        .unwrap();
        assert!(matches!(result, WebhookAction::Enqueue { .. }));
    }

    #[tokio::test]
    async fn event_dependabot_alert_created() {
        let state = test_state().await;
        let body = serde_json::json!({
            "action": "created",
            "repository": { "id": 100, "name": "dep-repo" }
        });
        let result =
            map_event_to_action("dependabot_alert", body.to_string().as_bytes(), &state).unwrap();
        assert!(matches!(result, WebhookAction::Enqueue { .. }));
    }

    #[tokio::test]
    async fn event_unknown_type() {
        let state = test_state().await;
        let body = b"{}";
        let result = map_event_to_action("star", body, &state).unwrap();
        assert!(matches!(result, WebhookAction::Ignore));
    }

    #[test]
    fn event_push_security_md() {
        let payload = PushEvent {
            ref_field: "refs/heads/main".to_string(),
            repository: PushRepository {
                id: 1,
                name: "test".to_string(),
                default_branch: "main".to_string(),
            },
            commits: vec![PushCommit {
                added: vec![],
                modified: vec!["SECURITY.md".to_string()],
                removed: vec![],
            }],
        };
        assert!(is_security_relevant_push(&payload));
    }

    #[test]
    fn event_push_codeowners() {
        let payload = PushEvent {
            ref_field: "refs/heads/main".to_string(),
            repository: PushRepository {
                id: 1,
                name: "test".to_string(),
                default_branch: "main".to_string(),
            },
            commits: vec![PushCommit {
                added: vec![".github/CODEOWNERS".to_string()],
                modified: vec![],
                removed: vec![],
            }],
        };
        assert!(is_security_relevant_push(&payload));
    }

    #[test]
    fn event_push_readme() {
        let payload = PushEvent {
            ref_field: "refs/heads/main".to_string(),
            repository: PushRepository {
                id: 1,
                name: "test".to_string(),
                default_branch: "main".to_string(),
            },
            commits: vec![PushCommit {
                added: vec![],
                modified: vec!["README.md".to_string()],
                removed: vec![],
            }],
        };
        assert!(!is_security_relevant_push(&payload));
    }

    #[test]
    fn event_push_non_default_branch() {
        let payload = PushEvent {
            ref_field: "refs/heads/feature-branch".to_string(),
            repository: PushRepository {
                id: 1,
                name: "test".to_string(),
                default_branch: "main".to_string(),
            },
            commits: vec![PushCommit {
                added: vec!["SECURITY.md".to_string()],
                modified: vec![],
                removed: vec![],
            }],
        };
        assert!(!is_security_relevant_push(&payload));
    }

    #[test]
    fn event_push_tag_ref() {
        let payload = PushEvent {
            ref_field: "refs/tags/v1.0.0".to_string(),
            repository: PushRepository {
                id: 1,
                name: "test".to_string(),
                default_branch: "main".to_string(),
            },
            commits: vec![PushCommit {
                added: vec!["SECURITY.md".to_string()],
                modified: vec![],
                removed: vec![],
            }],
        };
        assert!(!is_security_relevant_push(&payload));
    }

    #[test]
    fn event_push_github_security_md() {
        let payload = PushEvent {
            ref_field: "refs/heads/main".to_string(),
            repository: PushRepository {
                id: 1,
                name: "test".to_string(),
                default_branch: "main".to_string(),
            },
            commits: vec![PushCommit {
                added: vec![".github/SECURITY.md".to_string()],
                modified: vec![],
                removed: vec![],
            }],
        };
        assert!(is_security_relevant_push(&payload));
    }

    #[test]
    fn build_repository_from_payload_public() {
        let payload = WebhookRepository {
            id: 42,
            node_id: Some("MDQ6UmVwb3NpdG9yeTQy".to_string()),
            name: "my-repo".to_string(),
            full_name: Some("org/my-repo".to_string()),
            visibility: Some("public".to_string()),
            default_branch: Some("develop".to_string()),
            archived: false,
            html_url: Some("https://github.com/org/my-repo".to_string()),
        };
        let repo = build_repository_from_payload(&payload);
        assert_eq!(repo.id, "42");
        assert_eq!(repo.name, "my-repo");
        assert_eq!(repo.visibility, Visibility::Public);
        assert_eq!(repo.default_branch, "develop");
        assert_eq!(repo.inventory_key, "42");
    }

    #[tokio::test]
    async fn push_event_full_json_round_trip() {
        let state = test_state().await;
        let body = serde_json::json!({
            "ref": "refs/heads/main",
            "repository": {
                "id": 555,
                "name": "push-repo",
                "default_branch": "main"
            },
            "commits": [{
                "added": ["SECURITY.md"],
                "modified": [],
                "removed": []
            }]
        });
        let result = map_event_to_action("push", body.to_string().as_bytes(), &state).unwrap();
        assert!(matches!(result, WebhookAction::Enqueue { .. }));
    }

    #[tokio::test]
    async fn push_event_irrelevant_returns_ignore() {
        let state = test_state().await;
        let body = serde_json::json!({
            "ref": "refs/heads/main",
            "repository": {
                "id": 555,
                "name": "push-repo",
                "default_branch": "main"
            },
            "commits": [{
                "added": [],
                "modified": ["src/main.rs"],
                "removed": []
            }]
        });
        let result = map_event_to_action("push", body.to_string().as_bytes(), &state).unwrap();
        assert!(matches!(result, WebhookAction::Ignore));
    }

    #[tokio::test]
    async fn event_repository_archived_returns_remove() {
        let state = test_state().await;
        let body = serde_json::json!({
            "action": "archived",
            "repository": { "id": 888, "name": "archived-repo", "archived": true }
        });
        let result =
            map_event_to_action("repository", body.to_string().as_bytes(), &state).unwrap();
        match result {
            WebhookAction::Remove { inventory_key } => assert_eq!(inventory_key, "888"),
            _ => panic!("expected Remove for archived action"),
        }
    }

    #[tokio::test]
    async fn event_repository_lifecycle_actions_return_enqueue() {
        let state = test_state().await;
        for action in &["unarchived", "publicized", "privatized"] {
            let body = serde_json::json!({
                "action": action,
                "repository": {
                    "id": 999,
                    "name": "lifecycle-repo",
                    "visibility": "public"
                }
            });
            let result =
                map_event_to_action("repository", body.to_string().as_bytes(), &state).unwrap();
            assert!(
                matches!(result, WebhookAction::Enqueue { .. }),
                "expected Enqueue for action '{action}'"
            );
        }
    }

    #[tokio::test]
    async fn push_event_empty_commits_returns_ignore() {
        let state = test_state().await;
        let body = serde_json::json!({
            "ref": "refs/heads/main",
            "repository": {
                "id": 777,
                "name": "empty-push-repo",
                "default_branch": "main"
            },
            "commits": []
        });
        let result = map_event_to_action("push", body.to_string().as_bytes(), &state).unwrap();
        assert!(matches!(result, WebhookAction::Ignore));
    }

    #[test]
    fn webhook_error_display_invalid_json() {
        let err: Result<WebhookPayload, _> = serde_json::from_slice(b"not json");
        let webhook_err = WebhookError::from(err.unwrap_err());
        let msg = webhook_err.to_string();
        assert!(msg.starts_with("invalid JSON:"), "got: {msg}");
        assert!(webhook_err.source().is_some());
    }

    #[test]
    fn webhook_error_display_missing_repository() {
        let err = WebhookError::MissingRepository;
        assert_eq!(err.to_string(), "missing repository field");
        assert!(err.source().is_none());
    }

    #[tokio::test]
    async fn generic_enqueue_missing_repository_returns_error() {
        let state = test_state().await;
        let body = serde_json::json!({ "action": "created" });
        let result = map_event_to_action("dependabot_alert", body.to_string().as_bytes(), &state);
        assert!(matches!(result, Err(WebhookError::MissingRepository)));
    }

    #[tokio::test]
    async fn repository_event_invalid_json_returns_error() {
        let state = test_state().await;
        let result = map_event_to_action("repository", b"{{bad", &state);
        assert!(matches!(result, Err(WebhookError::InvalidJson(_))));
    }

    #[test]
    fn build_repository_defaults_private_and_main() {
        let payload = WebhookRepository {
            id: 50,
            node_id: None,
            name: "minimal".to_string(),
            full_name: None,
            visibility: None,
            default_branch: None,
            archived: false,
            html_url: None,
        };
        let repo = build_repository_from_payload(&payload);
        assert_eq!(repo.visibility, Visibility::Private);
        assert_eq!(repo.default_branch, "main");
    }
}
