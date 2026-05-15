//! Repository inventory collection from the GitHub API.
//!
//! Fetches and normalizes the full repository list for a GitHub
//! organization via paginated REST calls.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use tracing::{debug, warn};

use crate::config;
use crate::domain::repository::{Repository, Visibility};
use crate::error::InventoryError;
use crate::github::client::GitHubClient;
use crate::github::dto::GhRepository;

/// Intermediate payload returned by [`build_inventory_from_api`].
///
/// Carries the repository list and metadata needed by the collection
/// pipeline. Not persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventoryPayload {
    pub schema_version: String,
    pub organization: String,
    pub generated_at: String,
    pub repositories: Vec<Repository>,
    /// ISO 8601 timestamp of when the inventory API call completed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inventory_fetched_at: Option<String>,
}

/// Build a repository inventory from the GitHub API.
///
/// Fetches all org repositories via paginated REST, validates and normalizes
/// each entry, and returns an `InventoryPayload` for downstream processing.
///
/// # Default branch sanitization
///
/// The `default_branch` value from the API is trusted but sanitized as
/// defense-in-depth: leading/trailing whitespace is trimmed, and values
/// starting with `refs/` are replaced with `"main"`. This prevents
/// accidental path-traversal when the branch name is interpolated into
/// API URLs by evaluator modules.
///
/// # Errors
///
/// Returns `InventoryError` if the API request fails or the response cannot be parsed.
pub async fn build_inventory_from_api(
    client: &GitHubClient,
    now: Option<Timestamp>,
) -> Result<InventoryPayload, InventoryError> {
    debug!(org = %client.org_name, "building repository inventory from API");
    let now = now.unwrap_or_else(Timestamp::now);
    let path = format!(
        "/orgs/{}/repos?type=all&per_page={}",
        client.org_name,
        config::DEFAULT_PAGE_SIZE
    );
    let response = client.request(&path, true, 1, 60).await;

    if !response.is_ok() {
        let reason = response.error_message().map_or_else(
            || {
                response
                    .status_code()
                    .map_or_else(|| "unknown".to_string(), |c| c.to_string())
            },
            String::from,
        );
        warn!(reason = %reason, "inventory API fetch failed");
        return Err(InventoryError::ApiFetchFailed { reason });
    }

    let data = response
        .data()
        .cloned()
        .ok_or(InventoryError::ApiFetchFailed {
            reason: "empty response".to_string(),
        })?;

    let raw_repos: Vec<GhRepository> =
        serde_json::from_value(data).map_err(|e| InventoryError::ApiFetchFailed {
            reason: format!("failed to parse repository list: {e}"),
        })?;

    debug!(
        raw_count = raw_repos.len(),
        "parsed repository list from API"
    );

    let mut repositories: Vec<Repository> = Vec::new();
    for repo in &raw_repos {
        let visibility = resolve_visibility(repo.visibility.as_deref(), repo.private);
        let id = repo
            .id
            .map_or_else(|| format!("legacy:{}", repo.name), |id| id.to_string());
        let node_id = repo.node_id.clone();
        let default_branch =
            sanitize_default_branch(repo.default_branch.as_deref().unwrap_or("main"));
        let archived = repo.archived.unwrap_or(false);
        let inventory_key = repo.id.map_or_else(
            || node_id.clone().unwrap_or_else(|| repo.name.clone()),
            |id| id.to_string(),
        );

        repositories.push(Repository {
            id,
            node_id,
            name: repo.name.clone(),
            visibility,
            language: repo.language.clone(),
            default_branch,
            archived,
            has_issues: repo.has_issues.unwrap_or(false),
            inventory_key,
            updated_at: repo.updated_at.clone(),
            pushed_at: repo.pushed_at.clone(),
            created_at: repo.created_at.clone(),
            description: repo.description.clone(),
            fork: repo.fork.unwrap_or(false),
            html_url: repo.html_url.clone(),
            topics: repo.topics.clone().unwrap_or_default(),
            license_spdx: repo.license.as_ref().and_then(|l| l.spdx_id.clone()),
        });
    }

    sort_repositories(&mut repositories);

    debug!(
        repos = repositories.len(),
        org = %client.org_name,
        "inventory built"
    );

    Ok(InventoryPayload {
        schema_version: config::INVENTORY_SCHEMA_VERSION.to_string(),
        organization: client.org_name.clone(),
        generated_at: format_utc(now),
        repositories,
        inventory_fetched_at: Some(format_utc(Timestamp::now())),
    })
}

/// Sanitize a `default_branch` value from the GitHub API.
///
/// Defense-in-depth: the value is used in URL path interpolation by
/// evaluator modules. Trims whitespace and rejects `refs/`-prefixed
/// values (which would create malformed API paths).
fn sanitize_default_branch(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with("refs/") {
        "main".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Resolve visibility from API response fields.
fn resolve_visibility(visibility: Option<&str>, private: Option<bool>) -> Visibility {
    match visibility {
        Some("public") => Visibility::Public,
        Some("internal") => Visibility::Internal,
        Some("private") => Visibility::Private,
        _ => {
            if private.unwrap_or(false) {
                Visibility::Private
            } else {
                Visibility::Public
            }
        }
    }
}

/// Guarantees stable diff output across runs.
fn sort_repositories(repos: &mut [Repository]) {
    repos.sort();
}

/// Format a UTC timestamp as ISO 8601 without microseconds.
fn format_utc(ts: Timestamp) -> String {
    ts.strftime("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_visibility_from_string() {
        assert_eq!(resolve_visibility(Some("public"), None), Visibility::Public);
        assert_eq!(
            resolve_visibility(Some("internal"), None),
            Visibility::Internal
        );
        assert_eq!(
            resolve_visibility(Some("private"), None),
            Visibility::Private
        );
    }

    #[test]
    fn resolve_visibility_from_private_flag() {
        assert_eq!(resolve_visibility(None, Some(true)), Visibility::Private);
        assert_eq!(resolve_visibility(None, Some(false)), Visibility::Public);
        assert_eq!(resolve_visibility(None, None), Visibility::Public);
    }

    #[test]
    fn sanitize_default_branch_trims_whitespace() {
        assert_eq!(sanitize_default_branch(" main "), "main");
    }

    #[test]
    fn sanitize_default_branch_rejects_refs_prefix() {
        assert_eq!(sanitize_default_branch("refs/heads/main"), "main");
    }

    #[test]
    fn sanitize_default_branch_empty_falls_back() {
        assert_eq!(sanitize_default_branch(""), "main");
        assert_eq!(sanitize_default_branch("   "), "main");
    }

    #[test]
    fn sanitize_default_branch_preserves_normal() {
        assert_eq!(sanitize_default_branch("develop"), "develop");
        assert_eq!(sanitize_default_branch("release/v2"), "release/v2");
    }
}
