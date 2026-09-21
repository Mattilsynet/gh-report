//! CODEOWNERS evaluation.
//!
//! Checks for CODEOWNERS in `.github/CODEOWNERS` (conforming), root
//! `CODEOWNERS`, and `docs/CODEOWNERS` (both non-conforming), matching
//! GitHub's own three-location search order. When a CODEOWNERS file is
//! found, the content is downloaded, base64-decoded, and parsed to
//! extract owner references.

use base64::Engine;
use tracing::{debug, instrument, trace, warn};

use crate::collector::codeowners_parser::{self, ParsedCodeowners};
use crate::config;
use crate::domain::checks::{
    CodeownersContent, CodeownersNonConformingLocation, CodeownersResult, CodeownersStatus,
    IndeterminateReason,
};
use crate::domain::codeowners::CodeownersTruncationReason;
use crate::domain::repository::Repository;
use crate::github::client::GitHubClient;
use cherry_pit_web::sanitize_path_segment;

/// Maximum raw base64 string length before decoding (~133 KB → ~100 KB decoded).
const MAX_BASE64_LENGTH: usize = 133 * 1024;

/// Check if a content API response represents a file.
fn is_file_response(result: &crate::github::client::ApiOutcome) -> bool {
    result.is_ok()
        && result
            .data()
            .and_then(|d| d.get("type"))
            .and_then(serde_json::Value::as_str)
            == Some("file")
}

/// Build a `CodeownersResult` from the given status, path, and timestamp.
fn build_result(status: CodeownersStatus, path: Option<&str>, timestamp: &str) -> CodeownersResult {
    let timestamp = timestamp.to_string();
    match (status, path) {
        (CodeownersStatus::Conforming, _) => CodeownersResult::Conforming {
            content: CodeownersContent::Unparsed,
            timestamp,
        },
        (CodeownersStatus::NonConforming, Some("docs/CODEOWNERS")) => {
            CodeownersResult::NonConforming {
                location: CodeownersNonConformingLocation::Docs,
                content: CodeownersContent::Unparsed,
                timestamp,
            }
        }
        (CodeownersStatus::NonConforming, _) => CodeownersResult::NonConforming {
            location: CodeownersNonConformingLocation::Root,
            content: CodeownersContent::Unparsed,
            timestamp,
        },
        (CodeownersStatus::Absent, _) => CodeownersResult::Absent { timestamp },
        (CodeownersStatus::Unknown, _) => CodeownersResult::Unobservable {
            reason: IndeterminateReason::Invalid,
            timestamp,
        },
    }
}

/// Build a `CodeownersResult` for a file-found case, recording either parsed
/// content or the truncation reason that prevented parsing.
fn build_result_with_parsed(
    status: CodeownersStatus,
    path: &str,
    timestamp: &str,
    parsed_or_truncation: Result<ParsedCodeowners, CodeownersTruncationReason>,
) -> CodeownersResult {
    let content = match parsed_or_truncation {
        Ok(p) => CodeownersContent::Parsed(p),
        Err(reason) => CodeownersContent::Truncated(reason),
    };
    match build_result(status, Some(path), timestamp) {
        CodeownersResult::Conforming { timestamp, .. } => {
            CodeownersResult::Conforming { content, timestamp }
        }
        CodeownersResult::NonConforming {
            location,
            timestamp,
            ..
        } => CodeownersResult::NonConforming {
            location,
            content,
            timestamp,
        },
        other => other,
    }
}

/// Try to extract and parse CODEOWNERS content from an API response.
///
/// Returns `Err(CodeownersTruncationReason)` when the file was located but
/// parsing was skipped (encoding mismatch, oversized payload, decode failure,
/// invalid UTF-8). All such failures are logged at `warn` level so silent
/// data loss is observable in the operator's log stream.
fn try_parse_content(
    data: &serde_json::Value,
    repo_name: &str,
) -> Result<ParsedCodeowners, CodeownersTruncationReason> {
    let Some(encoding) = data.get("encoding").and_then(serde_json::Value::as_str) else {
        warn!(
            repo = %repo_name,
            "CODEOWNERS content encoding field missing or null, skipping parse"
        );
        return Err(CodeownersTruncationReason::NotBase64Encoded);
    };
    if encoding != "base64" {
        warn!(
            repo = %repo_name,
            encoding = encoding,
            "CODEOWNERS content encoding is not base64, skipping parse"
        );
        return Err(CodeownersTruncationReason::NotBase64Encoded);
    }

    let Some(raw_content) = data.get("content").and_then(serde_json::Value::as_str) else {
        warn!(
            repo = %repo_name,
            "CODEOWNERS content field missing or null, skipping parse"
        );
        return Err(CodeownersTruncationReason::ContentMissing);
    };

    if raw_content.len() > MAX_BASE64_LENGTH {
        warn!(
            repo = %repo_name,
            length = raw_content.len(),
            max = MAX_BASE64_LENGTH,
            "CODEOWNERS base64 content too large, skipping parse"
        );
        return Err(CodeownersTruncationReason::OversizedBase64);
    }

    let cleaned: String = raw_content
        .chars()
        .filter(|c| *c != '\n' && *c != '\r')
        .collect();

    let decoded = match base64::engine::general_purpose::STANDARD.decode(&cleaned) {
        Ok(d) => d,
        Err(e) => {
            warn!(
                repo = %repo_name,
                error = %e,
                "failed to base64-decode CODEOWNERS content"
            );
            return Err(CodeownersTruncationReason::DecodeFailed);
        }
    };

    let text = match String::from_utf8(decoded) {
        Ok(t) => t,
        Err(e) => {
            warn!(
                repo = %repo_name,
                error = %e,
                "CODEOWNERS content is not valid UTF-8"
            );
            return Err(CodeownersTruncationReason::InvalidUtf8);
        }
    };

    Ok(codeowners_parser::parse_codeowners(&text))
}

/// Outcome of probing a single candidate CODEOWNERS path.
enum PathProbe {
    /// File found at this path; classify as `status` and parse content.
    Found(CodeownersStatus, &'static str, serde_json::Value),
    /// Permission denied or transient failure — evaluation should stop
    /// and report `unknown`.
    Indeterminate(IndeterminateReason),
    /// No file at this path; caller should try the next candidate.
    NotFound,
}

/// Probe a single candidate CODEOWNERS path via the Contents API.
async fn probe_path(
    client: &GitHubClient,
    safe_name: &str,
    path: &'static str,
    status: CodeownersStatus,
) -> PathProbe {
    let outcome = client
        .request(
            &format!("/repos/{}/{}/contents/{}", client.org_name, safe_name, path),
            false,
            config::DEFAULT_MAX_RETRIES,
            config::DEFAULT_REQUEST_TIMEOUT_SECS,
        )
        .await;

    if is_file_response(&outcome) {
        let Some(data) = outcome.data().cloned() else {
            return PathProbe::Found(status, path, serde_json::Value::Null);
        };
        return PathProbe::Found(status, path, data);
    }
    match (outcome.is_ok(), outcome.status_code()) {
        (false, Some(401 | 403)) => PathProbe::Indeterminate(IndeterminateReason::PermissionDenied),
        (false, Some(404)) => PathProbe::NotFound,
        (false, Some(429)) => PathProbe::Indeterminate(IndeterminateReason::RateLimited),
        (false, _) if outcome.is_retryable() => {
            PathProbe::Indeterminate(IndeterminateReason::Transient)
        }
        (true, _)
            if outcome.data().is_some_and(|data| {
                data.is_array()
                    || matches!(
                        data.get("type").and_then(serde_json::Value::as_str),
                        Some("dir" | "symlink" | "submodule")
                    )
            }) =>
        {
            PathProbe::NotFound
        }
        _ => PathProbe::Indeterminate(IndeterminateReason::Invalid),
    }
}

/// Evaluate CODEOWNERS for a repository.
///
/// Checks candidate paths in order — `.github/CODEOWNERS` (conforming),
/// root `CODEOWNERS`, then `docs/CODEOWNERS` (both non-conforming, matching
/// GitHub's own three-location search) — returning at the first match.
/// A permission-denied or transient failure on any candidate short-circuits
/// to `unknown`; no file at any candidate yields `absent`.
///
/// When a file is found, the content is downloaded, base64-decoded, and
/// parsed to extract owner references.
#[instrument(skip_all, fields(repo = %repo.name))]
pub async fn evaluate(
    client: &GitHubClient,
    repo: &Repository,
    run_timestamp: &str,
) -> CodeownersResult {
    let safe_name = match sanitize_path_segment(&repo.name, "repo_name") {
        Ok(n) => n,
        Err(e) => {
            debug!(repo = %repo.name, error = %e, "skipping CODEOWNERS: invalid repo name");
            return build_result(CodeownersStatus::Unknown, None, run_timestamp);
        }
    };

    trace!(repo = %repo.name, "evaluating CODEOWNERS");

    let candidates = [
        (
            config::CONFORMING_CODEOWNERS_PATH,
            CodeownersStatus::Conforming,
        ),
        (
            config::NON_CONFORMING_CODEOWNERS_PATH,
            CodeownersStatus::NonConforming,
        ),
        (
            config::DOCS_CODEOWNERS_PATH,
            CodeownersStatus::NonConforming,
        ),
    ];

    for (path, status) in candidates {
        match probe_path(client, &safe_name, path, status).await {
            PathProbe::Found(status, path, data) => {
                debug!(repo = %repo.name, path, status = %status, "CODEOWNERS found");
                let parsed_or_truncation = try_parse_content(&data, &repo.name);
                return build_result_with_parsed(status, path, run_timestamp, parsed_or_truncation);
            }
            PathProbe::Indeterminate(reason) => {
                debug!(repo = %repo.name, path, status = "unknown", "CODEOWNERS path check failed (403 or transient)");
                return CodeownersResult::Unobservable {
                    reason,
                    timestamp: run_timestamp.to_string(),
                };
            }
            PathProbe::NotFound => {}
        }
    }

    debug!(repo = %repo.name, status = "absent", "no CODEOWNERS file found");
    build_result(CodeownersStatus::Absent, None, run_timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn malformed_and_unauthorized_codeowners_remain_unobservable() {
        for (response, reason) in [
            (
                ResponseTemplate::new(200).set_body_json(serde_json::json!({})),
                IndeterminateReason::Invalid,
            ),
            (error_status(401), IndeterminateReason::PermissionDenied),
        ] {
            let server = MockServer::start().await;
            mount_candidate(
                &server,
                "repo",
                config::CONFORMING_CODEOWNERS_PATH,
                response,
            )
            .await;
            let result = evaluate_repo(&server, "repo").await;
            assert!(
                matches!(result, CodeownersResult::Unobservable { reason: actual, .. } if actual == reason)
            );
        }
    }
    use crate::github::auth::GitHubCredential;
    use crate::github::budget::BudgetGate;
    use std::sync::Arc;
    use std::time::Duration;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

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

    fn not_found() -> ResponseTemplate {
        ResponseTemplate::new(404).set_body_json(serde_json::json!({"message": "Not Found"}))
    }

    fn error_status(code: u16) -> ResponseTemplate {
        ResponseTemplate::new(code).set_body_json(serde_json::json!({"message": "err"}))
    }

    fn candidate_paths() -> [&'static str; 3] {
        [
            config::CONFORMING_CODEOWNERS_PATH,
            config::NON_CONFORMING_CODEOWNERS_PATH,
            config::DOCS_CODEOWNERS_PATH,
        ]
    }

    fn file_with(content: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "type": "file",
            "encoding": "base64",
            "content": base64::engine::general_purpose::STANDARD.encode(content),
        }))
    }

    async fn mount_candidate(
        server: &MockServer,
        repo: &str,
        candidate: &str,
        template: ResponseTemplate,
    ) {
        Mock::given(path(format!("/repos/test-org/{repo}/contents/{candidate}")))
            .respond_with(template)
            .mount(server)
            .await;
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

    async fn evaluate_repo(server: &MockServer, repo: &str) -> CodeownersResult {
        let client = test_client(&server.uri());
        let repository = crate::test_fixtures::make_repository(
            repo,
            false,
            crate::domain::repository::Visibility::Public,
        );
        evaluate(&client, &repository, "2026-01-01T00:00:00+00:00").await
    }

    /// A repo whose only CODEOWNERS lives at `docs/CODEOWNERS` (GitHub's
    /// third search location) must still be classified `non_conforming`
    /// and parsed — not silently treated as `absent`.
    #[tokio::test]
    async fn docs_codeowners_is_found_and_classified_non_conforming() {
        let server = MockServer::start().await;

        Mock::given(path(format!(
            "/repos/test-org/docs-only/contents/{}",
            config::CONFORMING_CODEOWNERS_PATH
        )))
        .respond_with(not_found())
        .mount(&server)
        .await;
        Mock::given(path(format!(
            "/repos/test-org/docs-only/contents/{}",
            config::NON_CONFORMING_CODEOWNERS_PATH
        )))
        .respond_with(not_found())
        .mount(&server)
        .await;
        Mock::given(path(format!(
            "/repos/test-org/docs-only/contents/{}",
            config::DOCS_CODEOWNERS_PATH
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "type": "file",
            "encoding": "base64",
            "content": base64::engine::general_purpose::STANDARD.encode("* @org/security\n"),
        })))
        .mount(&server)
        .await;

        let client = test_client(&server.uri());
        let repo = crate::test_fixtures::make_repository(
            "docs-only",
            false,
            crate::domain::repository::Visibility::Public,
        );
        let result = evaluate(&client, &repo, "2026-01-01T00:00:00+00:00").await;

        assert_eq!(result.status(), CodeownersStatus::NonConforming);
        assert_eq!(result.path(), Some(config::DOCS_CODEOWNERS_PATH));
        let parsed = result.parsed().expect("docs/CODEOWNERS should parse");
        assert_eq!(parsed.entries[0].owners, vec!["@org/security"]);
    }

    #[tokio::test]
    async fn evaluator_classifies_github_location_as_conforming() {
        let server = MockServer::start().await;
        mount_candidate(
            &server,
            "conforming",
            config::CONFORMING_CODEOWNERS_PATH,
            file_with("* @org/security\n"),
        )
        .await;

        let result = evaluate_repo(&server, "conforming").await;

        assert_eq!(result.status(), CodeownersStatus::Conforming);
        assert_eq!(result.path(), Some(config::CONFORMING_CODEOWNERS_PATH));
        let parsed = result.parsed().expect(".github/CODEOWNERS should parse");
        assert_eq!(parsed.entries[0].owners, vec!["@org/security"]);
    }

    #[tokio::test]
    async fn evaluator_falls_back_to_root_location_as_non_conforming() {
        let server = MockServer::start().await;
        mount_candidate(
            &server,
            "root-only",
            config::CONFORMING_CODEOWNERS_PATH,
            not_found(),
        )
        .await;
        mount_candidate(
            &server,
            "root-only",
            config::NON_CONFORMING_CODEOWNERS_PATH,
            file_with("* @org/platform\n"),
        )
        .await;

        let result = evaluate_repo(&server, "root-only").await;

        assert_eq!(result.status(), CodeownersStatus::NonConforming);
        assert_eq!(result.path(), Some(config::NON_CONFORMING_CODEOWNERS_PATH));
        let parsed = result.parsed().expect("root CODEOWNERS should parse");
        assert_eq!(parsed.entries[0].owners, vec!["@org/platform"]);
    }

    #[tokio::test]
    async fn evaluator_reports_absent_when_no_candidate_exists() {
        let server = MockServer::start().await;
        for candidate in candidate_paths() {
            mount_candidate(&server, "no-owners", candidate, not_found()).await;
        }

        let result = evaluate_repo(&server, "no-owners").await;

        assert_eq!(result.status(), CodeownersStatus::Absent);
        assert!(result.path().is_none());
        assert!(result.parsed().is_none());
    }

    #[tokio::test]
    async fn evaluator_reports_unknown_on_permission_denied_rather_than_absent() {
        let server = MockServer::start().await;
        for candidate in candidate_paths() {
            mount_candidate(&server, "denied", candidate, error_status(403)).await;
        }

        let result = evaluate_repo(&server, "denied").await;

        assert_eq!(result.status(), CodeownersStatus::Unknown);
        assert!(result.path().is_none());
        assert_eq!(
            requested_paths(&server).await,
            vec![format!(
                "/repos/test-org/denied/contents/{}",
                config::CONFORMING_CODEOWNERS_PATH
            )]
        );
    }

    #[tokio::test]
    async fn evaluator_stops_at_the_first_failing_candidate() {
        let server = MockServer::start().await;
        mount_candidate(
            &server,
            "denied-then-file",
            config::CONFORMING_CODEOWNERS_PATH,
            error_status(403),
        )
        .await;
        mount_candidate(
            &server,
            "denied-then-file",
            config::NON_CONFORMING_CODEOWNERS_PATH,
            file_with("* @org/platform\n"),
        )
        .await;

        let result = evaluate_repo(&server, "denied-then-file").await;

        assert_eq!(result.status(), CodeownersStatus::Unknown);
        assert!(result.parsed().is_none());
        assert_eq!(
            requested_paths(&server).await,
            vec![format!(
                "/repos/test-org/denied-then-file/contents/{}",
                config::CONFORMING_CODEOWNERS_PATH
            )]
        );
    }

    #[tokio::test]
    async fn evaluator_reports_unknown_on_transient_error_rather_than_absent() {
        let server = MockServer::start().await;
        for candidate in candidate_paths() {
            mount_candidate(&server, "flaky", candidate, error_status(503)).await;
        }

        let result = evaluate_repo(&server, "flaky").await;

        assert_eq!(result.status(), CodeownersStatus::Unknown);
        assert!(result.path().is_none());
        let first_candidate = format!(
            "/repos/test-org/flaky/contents/{}",
            config::CONFORMING_CODEOWNERS_PATH
        );
        let requested = requested_paths(&server).await;
        assert!(requested.iter().all(|p| *p == first_candidate));
        assert!(requested.len() <= usize::try_from(config::DEFAULT_MAX_RETRIES).unwrap() + 1);
    }

    #[tokio::test]
    async fn evaluator_prefers_a_found_later_candidate_over_an_earlier_absence() {
        let server = MockServer::start().await;
        mount_candidate(
            &server,
            "absent-then-file",
            config::CONFORMING_CODEOWNERS_PATH,
            not_found(),
        )
        .await;
        mount_candidate(
            &server,
            "absent-then-file",
            config::NON_CONFORMING_CODEOWNERS_PATH,
            error_status(403),
        )
        .await;

        let result = evaluate_repo(&server, "absent-then-file").await;

        assert_eq!(result.status(), CodeownersStatus::Unknown);
        assert_eq!(
            requested_paths(&server).await,
            vec![
                format!(
                    "/repos/test-org/absent-then-file/contents/{}",
                    config::CONFORMING_CODEOWNERS_PATH
                ),
                format!(
                    "/repos/test-org/absent-then-file/contents/{}",
                    config::NON_CONFORMING_CODEOWNERS_PATH
                ),
            ]
        );
    }

    #[test]
    fn status_display() {
        assert_eq!(CodeownersStatus::Conforming.to_string(), "conforming");
        assert_eq!(
            CodeownersStatus::NonConforming.to_string(),
            "non_conforming"
        );
        assert_eq!(CodeownersStatus::Absent.to_string(), "absent");
        assert_eq!(CodeownersStatus::Unknown.to_string(), "unknown");
    }

    /// Encode a string as base64 (standard, no padding stripping).
    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    #[test]
    fn try_parse_content_valid_base64() {
        let data = serde_json::json!({
            "encoding": "base64",
            "content": b64("* @org/security\n")
        });
        let parsed = try_parse_content(&data, "test-repo");
        assert!(parsed.is_ok(), "valid base64 content should parse");
        let p = parsed.unwrap();
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.entries[0].owners, vec!["@org/security"]);
    }

    #[test]
    fn try_parse_content_with_embedded_newlines_in_base64() {
        let raw = b64("* @org/security\n");
        let wrapped = raw
            .as_bytes()
            .chunks(10)
            .map(|chunk| std::str::from_utf8(chunk).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        let data = serde_json::json!({
            "encoding": "base64",
            "content": wrapped
        });
        let parsed = try_parse_content(&data, "test-repo");
        assert!(
            parsed.is_ok(),
            "embedded newlines in base64 should be stripped"
        );
    }

    #[test]
    fn try_parse_content_with_crlf_in_base64() {
        let raw = b64("* @team\n");
        let wrapped = raw
            .as_bytes()
            .chunks(10)
            .map(|chunk| std::str::from_utf8(chunk).unwrap())
            .collect::<Vec<_>>()
            .join("\r\n");
        let data = serde_json::json!({
            "encoding": "base64",
            "content": wrapped
        });
        let parsed = try_parse_content(&data, "test-repo");
        assert!(parsed.is_ok(), "\\r\\n in base64 should be stripped");
    }

    #[test]
    fn try_parse_content_encoding_not_base64() {
        let data = serde_json::json!({
            "encoding": "none",
            "content": "* @org/security\n"
        });
        let parsed = try_parse_content(&data, "test-repo");
        assert_eq!(parsed, Err(CodeownersTruncationReason::NotBase64Encoded));
    }

    #[test]
    fn try_parse_content_encoding_null() {
        let data = serde_json::json!({
            "encoding": null,
            "content": b64("* @team\n")
        });
        let parsed = try_parse_content(&data, "test-repo");
        assert_eq!(parsed, Err(CodeownersTruncationReason::NotBase64Encoded));
    }

    #[test]
    fn try_parse_content_encoding_missing() {
        let data = serde_json::json!({
            "content": b64("* @team\n")
        });
        let parsed = try_parse_content(&data, "test-repo");
        assert_eq!(parsed, Err(CodeownersTruncationReason::NotBase64Encoded));
    }

    #[test]
    fn try_parse_content_content_missing() {
        let data = serde_json::json!({
            "encoding": "base64"
        });
        let parsed = try_parse_content(&data, "test-repo");
        assert_eq!(parsed, Err(CodeownersTruncationReason::ContentMissing));
    }

    #[test]
    fn try_parse_content_content_null() {
        let data = serde_json::json!({
            "encoding": "base64",
            "content": null
        });
        let parsed = try_parse_content(&data, "test-repo");
        assert_eq!(parsed, Err(CodeownersTruncationReason::ContentMissing));
    }

    #[test]
    fn try_parse_content_oversized_base64() {
        let huge = "A".repeat(MAX_BASE64_LENGTH + 1);
        let data = serde_json::json!({
            "encoding": "base64",
            "content": huge
        });
        let parsed = try_parse_content(&data, "test-repo");
        assert_eq!(parsed, Err(CodeownersTruncationReason::OversizedBase64));
    }

    #[test]
    fn try_parse_content_invalid_base64() {
        let data = serde_json::json!({
            "encoding": "base64",
            "content": "not-valid-base64!!!"
        });
        let parsed = try_parse_content(&data, "test-repo");
        assert_eq!(parsed, Err(CodeownersTruncationReason::DecodeFailed));
    }

    #[test]
    fn try_parse_content_invalid_utf8() {
        let bad_bytes: &[u8] = &[0xFF, 0xFE, 0x00, 0x01];
        let encoded = base64::engine::general_purpose::STANDARD.encode(bad_bytes);
        let data = serde_json::json!({
            "encoding": "base64",
            "content": encoded
        });
        let parsed = try_parse_content(&data, "test-repo");
        assert_eq!(parsed, Err(CodeownersTruncationReason::InvalidUtf8));
    }

    #[test]
    fn try_parse_content_empty_after_decode() {
        let data = serde_json::json!({
            "encoding": "base64",
            "content": b64("")
        });
        let parsed = try_parse_content(&data, "test-repo");
        assert!(parsed.is_ok(), "empty content should still parse");
        assert!(parsed.unwrap().entries.is_empty());
    }
}
