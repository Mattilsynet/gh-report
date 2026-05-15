//! Security policy evaluation.
//!
//! Checks for the presence of a security policy via the GitHub setting
//! or by probing for SECURITY.md files in standard locations.

use tracing::{debug, instrument, trace};

use crate::config;
use crate::domain::checks::{SecurityPolicyEvidence, SecurityPolicyResult, SecurityPolicyStatus};
use crate::domain::repository::Repository;
use crate::github::client::GitHubClient;
use crate::infra::validate::sanitize_path_segment;

/// Build a `SecurityPolicyResult` from the given components.
fn build_result(
    status: SecurityPolicyStatus,
    evidence: SecurityPolicyEvidence,
    path: Option<&str>,
    timestamp: &str,
) -> SecurityPolicyResult {
    SecurityPolicyResult {
        status,
        evidence,
        path: path.map(str::to_string),
        timestamp: timestamp.to_string(),
    }
}

/// Evaluate the security policy for a repository.
///
/// Evaluation logic:
/// 1. Prefer explicit GitHub security policy setting if available.
/// 2. Fall back to file existence checks in standard locations.
/// 3. Map outcomes to pass/fail/unknown semantics.
#[instrument(skip_all, fields(repo = %repo.name))]
pub async fn evaluate(
    client: &GitHubClient,
    repo: &Repository,
    run_timestamp: &str,
) -> SecurityPolicyResult {
    // Validate repo name before URL interpolation — defense-in-depth against
    // path injection from API-derived data.
    let safe_name = match sanitize_path_segment(&repo.name, "repo_name") {
        Ok(n) => n,
        Err(e) => {
            debug!(repo = %repo.name, error = %e, "skipping security policy: invalid repo name");
            return build_result(
                SecurityPolicyStatus::Unknown,
                SecurityPolicyEvidence::TransientError,
                None,
                run_timestamp,
            );
        }
    };

    // Non-public repos: security policy evaluation is not applicable.
    if !repo.is_public() {
        debug!(repo = %repo.name, "skipping security policy for non-public repo");
        return build_result(
            SecurityPolicyStatus::NotApplicable,
            SecurityPolicyEvidence::NotApplicable,
            None,
            run_timestamp,
        );
    }

    trace!(repo = %repo.name, "evaluating security policy");
    let repo_details = client.repo_details(&safe_name).await;

    // Check the GitHub setting first
    if repo_details.is_ok()
        && repo_details
            .data()
            .and_then(|data| data.get("is_security_policy_enabled"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    {
        debug!(repo = %repo.name, evidence = "setting", "security policy enabled via GitHub setting");
        return build_result(
            SecurityPolicyStatus::Pass,
            SecurityPolicyEvidence::Setting,
            None,
            run_timestamp,
        );
    }

    let mut saw_permission_denied = repo_details.status_code() == Some(403);
    let mut saw_retryable_error = repo_details.is_retryable();

    // Fall back to file existence checks
    if let Some(result) = check_policy_files(
        client,
        repo,
        &safe_name,
        &mut saw_permission_denied,
        &mut saw_retryable_error,
        run_timestamp,
    )
    .await
    {
        return result;
    }

    if saw_permission_denied {
        debug!(repo = %repo.name, "security policy check returned permission denied");
        return build_result(
            SecurityPolicyStatus::Unknown,
            SecurityPolicyEvidence::PermissionDenied,
            None,
            run_timestamp,
        );
    }
    if saw_retryable_error {
        debug!(repo = %repo.name, "security policy check hit transient error");
        return build_result(
            SecurityPolicyStatus::Unknown,
            SecurityPolicyEvidence::TransientError,
            None,
            run_timestamp,
        );
    }

    debug!(repo = %repo.name, status = "fail", "no security policy found");
    build_result(
        SecurityPolicyStatus::Fail,
        SecurityPolicyEvidence::Absent,
        None,
        run_timestamp,
    )
}

/// Check standard file paths for a security policy.
///
/// Returns `Some(result)` if a policy file is found; `None` to continue
/// with subsequent fallback strategies.
async fn check_policy_files(
    client: &GitHubClient,
    repo: &Repository,
    safe_name: &str,
    saw_permission_denied: &mut bool,
    saw_retryable_error: &mut bool,
    run_timestamp: &str,
) -> Option<SecurityPolicyResult> {
    for &file_path in config::SECURITY_POLICY_PATHS {
        let content_path = format!(
            "/repos/{}/{}/contents/{file_path}",
            client.org_name, safe_name
        );
        let content = client
            .request(
                &content_path,
                false,
                config::DEFAULT_MAX_RETRIES,
                config::DEFAULT_REQUEST_TIMEOUT_SECS,
            )
            .await;

        if content.is_ok()
            && content
                .data()
                .and_then(|data| data.get("type"))
                .and_then(serde_json::Value::as_str)
                == Some("file")
        {
            debug!(repo = %repo.name, path = file_path, evidence = "file", "security policy found via file");
            return Some(build_result(
                SecurityPolicyStatus::Pass,
                SecurityPolicyEvidence::File,
                Some(file_path),
                run_timestamp,
            ));
        }
        if content.status_code() == Some(403) {
            *saw_permission_denied = true;
        }
        if content.is_retryable() {
            *saw_retryable_error = true;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_displays_correctly() {
        let result = SecurityPolicyResult {
            status: SecurityPolicyStatus::Pass,
            evidence: SecurityPolicyEvidence::Setting,
            path: None,
            timestamp: "2026-01-01T00:00:00+00:00".to_string(),
        };
        assert_eq!(result.status, SecurityPolicyStatus::Pass);
    }

    #[test]
    fn result_via_file() {
        let result = SecurityPolicyResult {
            status: SecurityPolicyStatus::Pass,
            evidence: SecurityPolicyEvidence::File,
            path: Some("SECURITY.md".to_string()),
            timestamp: "2026-01-01T00:00:00+00:00".to_string(),
        };
        assert_eq!(result.status, SecurityPolicyStatus::Pass);
        assert_eq!(result.path.as_deref(), Some("SECURITY.md"));
    }

    #[test]
    fn result_unknown_permission_denied() {
        let result = SecurityPolicyResult {
            status: SecurityPolicyStatus::Unknown,
            evidence: SecurityPolicyEvidence::PermissionDenied,
            path: None,
            timestamp: "2026-01-01T00:00:00+00:00".to_string(),
        };
        assert_eq!(result.status, SecurityPolicyStatus::Unknown);
        assert_eq!(result.evidence, SecurityPolicyEvidence::PermissionDenied);
    }
}
