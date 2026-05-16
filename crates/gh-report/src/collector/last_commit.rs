//! Last commit collector.
//!
//! Fetches the most recent commit on each repository's default branch
//! to populate "last committer" data in the security posture report.
//! This is informational — failures return `None` and never block
//! the evaluation pipeline.

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use tracing::{debug, instrument};

use crate::config;
use crate::domain::evidence::LastCommitInfo;
use crate::domain::repository::Repository;
use crate::github::client::GitHubClient;
use cherry_pit_web::sanitize_path_segment;

/// Fetch the most recent commit on the repository's default branch.
///
/// Returns `Some(LastCommitInfo)` on success, `None` on any failure
/// (API error, empty repository, permission denied, etc.).
///
/// # URL safety
///
/// The repository name is validated via [`sanitize_path_segment`] and
/// the branch name is percent-encoded for safe query string interpolation
/// (branch names can contain `/`, `&`, `#`, `=`, etc.).
#[instrument(skip_all, fields(repo = %repo.name))]
pub(crate) async fn fetch_last_commit(
    client: &GitHubClient,
    repo: &Repository,
) -> Option<LastCommitInfo> {
    let safe_name = match sanitize_path_segment(&repo.name, "repo_name") {
        Ok(n) => n,
        Err(e) => {
            debug!(repo = %repo.name, error = %e, "skipping last commit: invalid repo name");
            return None;
        }
    };

    // Percent-encode the branch name for the query string parameter.
    // Branch names can contain `/` (e.g., `feature/login`), `&`, `#`, etc.
    // which would corrupt the URL if interpolated raw.
    let encoded_branch: String =
        utf8_percent_encode(&repo.default_branch, NON_ALPHANUMERIC).to_string();

    let path = format!(
        "/repos/{}/{}/commits?sha={}&per_page=1",
        client.org_name, safe_name, encoded_branch,
    );

    let result = client
        .request(
            &path,
            false,
            config::DEFAULT_MAX_RETRIES,
            config::DEFAULT_REQUEST_TIMEOUT_SECS,
        )
        .await;

    if !result.is_ok() {
        debug!(
            repo = %repo.name,
            status = ?result.status_code(),
            "last commit fetch failed (non-blocking)"
        );
        return None;
    }

    let data = result.data()?;
    let commits = data.as_array()?;
    let first = commits.first()?;

    let committer_login = first
        .get("committer")
        .and_then(|c| c.get("login"))
        .and_then(|v| v.as_str())
        .map(String::from);

    let committer_name = first
        .get("commit")
        .and_then(|c| c.get("committer"))
        .and_then(|c| c.get("name"))
        .and_then(|v| v.as_str())
        .map(String::from);

    let commit_date = first
        .get("commit")
        .and_then(|c| c.get("committer"))
        .and_then(|c| c.get("date"))
        .and_then(|v| v.as_str())
        .map(String::from);

    Some(LastCommitInfo {
        committer_login,
        committer_name,
        commit_date,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encodes_branch_with_special_chars() {
        // Verify that branch names with special characters are properly encoded.
        let encoded: String =
            utf8_percent_encode("feature/login&fix", NON_ALPHANUMERIC).to_string();
        assert_eq!(encoded, "feature%2Flogin%26fix");

        let encoded: String = utf8_percent_encode("release#1.0", NON_ALPHANUMERIC).to_string();
        assert_eq!(encoded, "release%231%2E0");

        let encoded: String = utf8_percent_encode("name=value", NON_ALPHANUMERIC).to_string();
        assert_eq!(encoded, "name%3Dvalue");
    }

    #[test]
    fn percent_encodes_simple_branch() {
        let encoded: String = utf8_percent_encode("main", NON_ALPHANUMERIC).to_string();
        assert_eq!(encoded, "main");
    }
}
