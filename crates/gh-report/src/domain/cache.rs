//! Cross-run cache types for repository detail data.
//!
//! `CachedRepoDetail` holds the subset of repository detail fields that
//! need to persist across collection runs. It lives in `domain` rather
//! than `app` or `github` because:
//!
//! - It is consumed by multiple layers (collection pipeline, GitHub client,
//!   cross-run moka cache on `AppState`).
//! - Placing it in `domain` eliminates the upward dependency from
//!   `github::client` → `app::state` that previously existed.
//!
//! The `etag` field is an HTTP-level concern but is included here because
//! it is tightly coupled to the `security_and_analysis` data it validates.
//! Splitting it into a separate wrapper would add indirection with no
//! practical benefit in a single-adapter codebase.

use jiff::Timestamp;

/// Typed cache entry for repository details.
///
/// Fields `security_and_analysis` and `is_security_policy_enabled` are
/// **only available from the single-repo detail endpoint**
/// (`GET /repos/{org}/{repo}`), never from the inventory list endpoint
/// (`GET /orgs/{org}/repos`). These are preserved here so that
/// cross-run cache hits provide enough data for the `security_policy`,
/// `dependabot`, and `ghas_scanning` collectors.
#[derive(Debug, Clone)]
pub struct CachedRepoDetail {
    /// Default branch name.
    pub default_branch: String,
    /// Last updated timestamp from the GitHub API.
    pub updated_at: Option<String>,
    /// Security and analysis settings (e.g., secret scanning, dependabot).
    /// Stored as raw JSON to avoid coupling cache shape to DTO evolution.
    pub security_and_analysis: Option<serde_json::Value>,
    /// Whether the repository has a security policy enabled.
    pub is_security_policy_enabled: Option<bool>,
    /// When this cache entry was fetched.
    pub fetched_at: Timestamp,
    /// `ETag` from the GitHub API response, used for conditional requests.
    pub etag: Option<String>,
}
