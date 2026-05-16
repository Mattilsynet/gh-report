//! Dependabot security updates evaluation.
//!
//! Checks the `security_and_analysis.dependabot_security_updates.status`
//! field from repository details.

use tracing::{debug, instrument, trace};

use crate::domain::checks::{DependabotResult, DependabotStatus};
use crate::domain::repository::Repository;
use crate::github::client::GitHubClient;
use cherry_pit_web::sanitize_path_segment;

/// Extract the `security_and_analysis.dependabot_security_updates.status` field.
fn extract_status(data: &serde_json::Value) -> Option<DependabotStatus> {
    let status_str = data
        .get("security_and_analysis")
        .and_then(|sa| sa.get("dependabot_security_updates"))
        .and_then(|ds| ds.get("status"))
        .and_then(serde_json::Value::as_str)?;
    match status_str {
        "enabled" => Some(DependabotStatus::Enabled),
        "paused" => Some(DependabotStatus::Paused),
        "disabled" => Some(DependabotStatus::Disabled),
        _ => None,
    }
}

fn build_result(
    status: DependabotStatus,
    timestamp: &str,
    reason: Option<&str>,
) -> DependabotResult {
    DependabotResult {
        status,
        reason: reason.map(String::from),
        timestamp: timestamp.to_string(),
    }
}

/// Evaluate Dependabot security updates for a repository.
///
/// Evaluation logic:
/// 1. Call `repo_details()` to get the `security_and_analysis` status.
/// 2. If status is `enabled`, `paused`, or `disabled`, return it directly.
/// 3. Otherwise, return `unknown` with a reason based on the error type.
#[instrument(skip_all, fields(repo = %repo.name))]
pub async fn evaluate(
    client: &GitHubClient,
    repo: &Repository,
    run_timestamp: &str,
) -> DependabotResult {
    // Validate repo name before URL interpolation — defense-in-depth against
    // path injection from API-derived data. repo_details() also validates
    // internally, but we guard at the collector entry point for consistency
    // with all other collectors.
    let safe_name = match sanitize_path_segment(&repo.name, "repo_name") {
        Ok(n) => n,
        Err(e) => {
            debug!(repo = %repo.name, error = %e, "skipping dependabot: invalid repo name");
            return build_result(
                DependabotStatus::Unknown,
                run_timestamp,
                Some("invalid_repo_name"),
            );
        }
    };

    trace!(repo = %repo.name, "evaluating dependabot security updates");
    let repo_details = client.repo_details(&safe_name).await;

    if repo_details.is_ok()
        && let Some(data) = repo_details.data()
        && let Some(status) = extract_status(data)
    {
        debug!(repo = %repo.name, status = %status, "dependabot evaluation complete");
        return build_result(status, run_timestamp, None);
    }

    if repo_details.status_code() == Some(403) {
        debug!(repo = %repo.name, reason = "permission_denied", "dependabot check returned 403");
        return build_result(
            DependabotStatus::Unknown,
            run_timestamp,
            Some("permission_denied"),
        );
    }

    if repo_details.is_retryable() {
        debug!(repo = %repo.name, reason = "transient_error", "dependabot check hit transient error");
        return build_result(
            DependabotStatus::Unknown,
            run_timestamp,
            Some("transient_error"),
        );
    }

    debug!(repo = %repo.name, reason = "insufficient_evidence", "dependabot status unknown");
    build_result(
        DependabotStatus::Unknown,
        run_timestamp,
        Some("insufficient_evidence"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_status_enabled() {
        let data = serde_json::json!({
            "security_and_analysis": {
                "dependabot_security_updates": {
                    "status": "enabled"
                }
            }
        });
        assert_eq!(extract_status(&data), Some(DependabotStatus::Enabled));
    }

    #[test]
    fn extract_status_disabled() {
        let data = serde_json::json!({
            "security_and_analysis": {
                "dependabot_security_updates": {
                    "status": "disabled"
                }
            }
        });
        assert_eq!(extract_status(&data), Some(DependabotStatus::Disabled));
    }

    #[test]
    fn extract_status_paused() {
        let data = serde_json::json!({
            "security_and_analysis": {
                "dependabot_security_updates": {
                    "status": "paused"
                }
            }
        });
        assert_eq!(extract_status(&data), Some(DependabotStatus::Paused));
    }

    #[test]
    fn extract_status_missing_field() {
        let data = serde_json::json!({"name": "test"});
        assert_eq!(extract_status(&data), None);
    }

    #[test]
    fn extract_status_unknown_value() {
        let data = serde_json::json!({
            "security_and_analysis": {
                "dependabot_security_updates": {
                    "status": "something_else"
                }
            }
        });
        assert_eq!(extract_status(&data), None);
    }

    #[test]
    fn build_result_no_reason() {
        let result = build_result(DependabotStatus::Enabled, "2026-01-01T00:00:00+00:00", None);
        assert_eq!(result.status, DependabotStatus::Enabled);
        assert!(result.reason.is_none());
        assert_eq!(result.timestamp, "2026-01-01T00:00:00+00:00");
    }

    #[test]
    fn build_result_with_reason() {
        let result = build_result(
            DependabotStatus::Unknown,
            "2026-01-01T00:00:00+00:00",
            Some("permission_denied"),
        );
        assert_eq!(result.status, DependabotStatus::Unknown);
        assert_eq!(result.reason.as_deref(), Some("permission_denied"));
    }
}
