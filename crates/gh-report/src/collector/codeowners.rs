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
use crate::domain::checks::{CodeownersResult, CodeownersStatus};
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
    CodeownersResult {
        status,
        path: path.map(str::to_string),
        timestamp: timestamp.to_string(),
        parsed: None,
        truncation: None,
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
    let (parsed, truncation) = match parsed_or_truncation {
        Ok(p) => (Some(p), None),
        Err(reason) => (None, Some(reason)),
    };
    CodeownersResult {
        status,
        path: Some(path.to_string()),
        timestamp: timestamp.to_string(),
        parsed,
        truncation,
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
    Indeterminate,
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
    if outcome.status_code() == Some(403) || outcome.is_retryable() {
        return PathProbe::Indeterminate;
    }
    PathProbe::NotFound
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
            PathProbe::Indeterminate => {
                debug!(repo = %repo.name, path, status = "unknown", "CODEOWNERS path check failed (403 or transient)");
                return build_result(CodeownersStatus::Unknown, None, run_timestamp);
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

        assert_eq!(result.status, CodeownersStatus::NonConforming);
        assert_eq!(result.path.as_deref(), Some(config::DOCS_CODEOWNERS_PATH));
        let parsed = result.parsed.expect("docs/CODEOWNERS should parse");
        assert_eq!(parsed.entries[0].owners, vec!["@org/security"]);
    }

    #[test]
    fn conforming_result_structure() {
        let result = CodeownersResult {
            status: CodeownersStatus::Conforming,
            path: Some(config::CONFORMING_CODEOWNERS_PATH.to_string()),
            timestamp: "2026-01-01T00:00:00+00:00".to_string(),
            parsed: None,
            truncation: None,
        };
        assert_eq!(result.status, CodeownersStatus::Conforming);
        assert_eq!(
            result.path.as_deref(),
            Some(config::CONFORMING_CODEOWNERS_PATH)
        );
    }

    #[test]
    fn non_conforming_result_structure() {
        let result = CodeownersResult {
            status: CodeownersStatus::NonConforming,
            path: Some(config::NON_CONFORMING_CODEOWNERS_PATH.to_string()),
            timestamp: "2026-01-01T00:00:00+00:00".to_string(),
            parsed: None,
            truncation: None,
        };
        assert_eq!(result.status, CodeownersStatus::NonConforming);
        assert_eq!(
            result.path.as_deref(),
            Some(config::NON_CONFORMING_CODEOWNERS_PATH)
        );
    }

    #[test]
    fn absent_result_structure() {
        let result = CodeownersResult {
            status: CodeownersStatus::Absent,
            path: None,
            timestamp: "2026-01-01T00:00:00+00:00".to_string(),
            parsed: None,
            truncation: None,
        };
        assert_eq!(result.status, CodeownersStatus::Absent);
        assert!(result.path.is_none());
    }

    #[test]
    fn unknown_result_structure() {
        let result = CodeownersResult {
            status: CodeownersStatus::Unknown,
            path: None,
            timestamp: "2026-01-01T00:00:00+00:00".to_string(),
            parsed: None,
            truncation: None,
        };
        assert_eq!(result.status, CodeownersStatus::Unknown);
        assert!(result.path.is_none());
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
