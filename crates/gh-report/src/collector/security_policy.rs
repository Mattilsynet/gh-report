//! Security policy evaluation.
//!
//! Checks for the presence of a security policy via the GitHub setting
//! or by probing for SECURITY.md files in standard locations.

use tracing::{debug, instrument, trace};

use crate::config;
use crate::domain::checks::{SecurityPolicyEvidence, SecurityPolicyResult, SecurityPolicyStatus};
use crate::domain::repository::Repository;
use crate::github::client::GitHubClient;
use cherry_pit_web::sanitize_path_segment;

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
    use crate::domain::repository::Visibility;
    use crate::github::auth::GitHubCredential;
    use crate::github::budget::BudgetGate;
    use std::sync::Arc;
    use std::time::Duration;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TS: &str = "2026-01-01T00:00:00+00:00";

    fn test_client(base_url: &str) -> GitHubClient {
        let credential = GitHubCredential {
            mode: crate::domain::auth::AuthMode::Pat,
            token: secrecy::SecretString::from("test-token"),
            expires_at: None,
        };
        let budget = Arc::new(BudgetGate::new(
            config::API_BUDGET_LIMIT,
            Duration::from_secs(config::API_BUDGET_WAIT_SECS),
        ));
        let rate_limit = Arc::new(crate::github::rate_limit::new_default());
        GitHubClient::new(credential, base_url, "test-org", None, budget, rate_limit)
            .expect("test client construction should succeed")
    }

    fn public_repo(name: &str) -> Repository {
        crate::test_fixtures::make_repository(name, false, Visibility::Public)
    }

    fn status(code: u16) -> ResponseTemplate {
        ResponseTemplate::new(code).set_body_json(serde_json::json!({"message": "err"}))
    }

    async fn mount_details(server: &MockServer, repo: &str, template: ResponseTemplate) {
        Mock::given(path(format!("/repos/test-org/{repo}")))
            .respond_with(template)
            .mount(server)
            .await;
    }

    async fn mount_policy_path(
        server: &MockServer,
        repo: &str,
        file_path: &str,
        template: ResponseTemplate,
    ) {
        Mock::given(path(format!("/repos/test-org/{repo}/contents/{file_path}")))
            .respond_with(template)
            .mount(server)
            .await;
    }

    fn setting(enabled: bool) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "default_branch": "main",
            "is_security_policy_enabled": enabled,
        }))
    }

    fn file_response() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({"type": "file"}))
    }

    async fn mount_policy_paths_with_one_failing(server: &MockServer, repo: &str, code: u16) {
        for (index, &file_path) in config::SECURITY_POLICY_PATHS.iter().enumerate() {
            let template = if index == 0 {
                status(code)
            } else {
                status(404)
            };
            mount_policy_path(server, repo, file_path, template).await;
        }
    }

    async fn requested_paths(server: &MockServer) -> Vec<String> {
        server
            .received_requests()
            .await
            .expect("mock server should record requests")
            .iter()
            .map(|request| request.url.path().to_string())
            .collect()
    }

    #[tokio::test]
    async fn evaluator_passes_on_github_setting_without_probing_files() {
        let server = MockServer::start().await;
        mount_details(&server, "setting-repo", setting(true)).await;

        let result = evaluate(
            &test_client(&server.uri()),
            &public_repo("setting-repo"),
            TS,
        )
        .await;

        assert_eq!(result.status, SecurityPolicyStatus::Pass);
        assert_eq!(result.evidence, SecurityPolicyEvidence::Setting);
        assert!(result.path.is_none());
        assert_eq!(
            requested_paths(&server).await,
            vec!["/repos/test-org/setting-repo".to_string()]
        );
    }

    #[tokio::test]
    async fn evaluator_falls_back_to_a_later_policy_file_location() {
        let server = MockServer::start().await;
        mount_details(&server, "fallback-repo", setting(false)).await;
        mount_policy_path(&server, "fallback-repo", "SECURITY.md", status(404)).await;
        mount_policy_path(
            &server,
            "fallback-repo",
            ".github/SECURITY.md",
            file_response(),
        )
        .await;

        let result = evaluate(
            &test_client(&server.uri()),
            &public_repo("fallback-repo"),
            TS,
        )
        .await;

        assert_eq!(result.status, SecurityPolicyStatus::Pass);
        assert_eq!(result.evidence, SecurityPolicyEvidence::File);
        assert_eq!(result.path.as_deref(), Some(".github/SECURITY.md"));
    }

    #[tokio::test]
    async fn evaluator_fails_absent_when_setting_off_and_no_file_exists() {
        let server = MockServer::start().await;
        mount_details(&server, "absent-repo", setting(false)).await;
        for &file_path in config::SECURITY_POLICY_PATHS {
            mount_policy_path(&server, "absent-repo", file_path, status(404)).await;
        }

        let result = evaluate(&test_client(&server.uri()), &public_repo("absent-repo"), TS).await;

        assert_eq!(result.status, SecurityPolicyStatus::Fail);
        assert_eq!(result.evidence, SecurityPolicyEvidence::Absent);
        assert!(result.path.is_none());
    }

    #[tokio::test]
    async fn evaluator_reports_permission_denied_rather_than_absent() {
        let server = MockServer::start().await;
        mount_details(&server, "denied-repo", status(403)).await;
        for &file_path in config::SECURITY_POLICY_PATHS {
            mount_policy_path(&server, "denied-repo", file_path, status(403)).await;
        }

        let result = evaluate(&test_client(&server.uri()), &public_repo("denied-repo"), TS).await;

        assert_eq!(result.status, SecurityPolicyStatus::Unknown);
        assert_eq!(result.evidence, SecurityPolicyEvidence::PermissionDenied);
    }

    #[tokio::test]
    async fn evaluator_reports_transient_error_rather_than_absent() {
        let server = MockServer::start().await;
        mount_details(&server, "transient-repo", status(503)).await;
        for &file_path in config::SECURITY_POLICY_PATHS {
            mount_policy_path(&server, "transient-repo", file_path, status(404)).await;
        }

        let result = evaluate(
            &test_client(&server.uri()),
            &public_repo("transient-repo"),
            TS,
        )
        .await;

        assert_eq!(result.status, SecurityPolicyStatus::Unknown);
        assert_eq!(result.evidence, SecurityPolicyEvidence::TransientError);
    }

    #[tokio::test]
    async fn evaluator_reports_not_applicable_for_a_private_repo() {
        let server = MockServer::start().await;
        let repo =
            crate::test_fixtures::make_repository("private-repo", false, Visibility::Private);

        let result = evaluate(&test_client(&server.uri()), &repo, TS).await;

        assert_eq!(result.status, SecurityPolicyStatus::NotApplicable);
        assert_eq!(result.evidence, SecurityPolicyEvidence::NotApplicable);
        assert!(requested_paths(&server).await.is_empty());
    }

    #[tokio::test]
    async fn evaluator_reports_permission_denied_seen_only_while_probing_files() {
        let server = MockServer::start().await;
        mount_details(&server, "file-denied-repo", setting(false)).await;
        mount_policy_paths_with_one_failing(&server, "file-denied-repo", 403).await;

        let result = evaluate(
            &test_client(&server.uri()),
            &public_repo("file-denied-repo"),
            TS,
        )
        .await;

        assert_eq!(result.status, SecurityPolicyStatus::Unknown);
        assert_eq!(result.evidence, SecurityPolicyEvidence::PermissionDenied);
        assert!(result.path.is_none());
    }

    #[tokio::test]
    async fn evaluator_reports_transient_error_seen_only_while_probing_files() {
        let server = MockServer::start().await;
        mount_details(&server, "file-transient-repo", setting(false)).await;
        mount_policy_paths_with_one_failing(&server, "file-transient-repo", 503).await;

        let result = evaluate(
            &test_client(&server.uri()),
            &public_repo("file-transient-repo"),
            TS,
        )
        .await;

        assert_eq!(result.status, SecurityPolicyStatus::Unknown);
        assert_eq!(result.evidence, SecurityPolicyEvidence::TransientError);
        assert!(result.path.is_none());
    }

    #[tokio::test]
    async fn evaluator_prefers_a_found_policy_file_over_a_failed_details_lookup() {
        let server = MockServer::start().await;
        mount_details(&server, "denied-details-repo", status(403)).await;
        mount_policy_path(&server, "denied-details-repo", "SECURITY.md", status(404)).await;
        mount_policy_path(
            &server,
            "denied-details-repo",
            ".github/SECURITY.md",
            file_response(),
        )
        .await;

        let result = evaluate(
            &test_client(&server.uri()),
            &public_repo("denied-details-repo"),
            TS,
        )
        .await;

        assert_eq!(result.status, SecurityPolicyStatus::Pass);
        assert_eq!(result.evidence, SecurityPolicyEvidence::File);
        assert_eq!(result.path.as_deref(), Some(".github/SECURITY.md"));
    }
}
