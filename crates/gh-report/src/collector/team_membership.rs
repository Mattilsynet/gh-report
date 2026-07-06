//! GitHub team membership collection (B1).
//!
//! Fetches the complete, role-tagged member roster for each GitHub team
//! referenced by CODEOWNERS. Render-time only (oracle adr-fmt-kqavx CLASS B
//! verdict): rosters are fetched fresh every collection tick via the
//! existing budget/rate-limit-gated [`GitHubClient::request`] and are never
//! persisted to the native per-repo event payload.

use tracing::warn;

use crate::config;
use crate::domain::metrics::{TeamMember, TeamMemberRole, TeamRoster, TeamRosterStatus};
use crate::github::client::{ApiOutcome, GitHubClient};
use crate::github::dto::GhTeamMember;

/// Fetch rosters for every `(canonical_owner, team_slug)` pair.
///
/// One [`TeamRoster`] per input pair, in the same order. Each team's fetch
/// is independent; a degraded fetch for one team does not affect others.
pub async fn collect_team_rosters(
    client: &GitHubClient,
    teams: &[(String, String)],
) -> Vec<TeamRoster> {
    let mut rosters = Vec::with_capacity(teams.len());
    for (canonical_owner, team_slug) in teams {
        rosters.push(collect_one_team_roster(client, canonical_owner, team_slug).await);
    }
    rosters
}

fn degraded_roster(canonical_owner: &str, team_slug: &str, status: TeamRosterStatus) -> TeamRoster {
    TeamRoster {
        canonical_owner: canonical_owner.to_string(),
        team_slug: team_slug.to_string(),
        status,
        members: Vec::new(),
    }
}

/// Classify a failed [`ApiOutcome`] into a [`TeamRosterStatus`].
fn failure_status(outcome: &ApiOutcome) -> TeamRosterStatus {
    match outcome.status_code() {
        Some(403 | 404) => TeamRosterStatus::PermissionDenied,
        _ => TeamRosterStatus::TransientError,
    }
}

/// Parse a successful members-list `ApiOutcome` into a list of logins.
///
/// Entries that fail to parse as [`GhTeamMember`] are skipped and logged;
/// they do not fail the whole fetch.
fn logins_from_outcome(outcome: &ApiOutcome) -> Vec<String> {
    let items = outcome
        .data()
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut logins = Vec::with_capacity(items.len());
    for item in items {
        match serde_json::from_value::<GhTeamMember>(item) {
            Ok(member) => logins.push(member.login),
            Err(e) => warn!(error = %e, "failed to parse team member entry — skipping"),
        }
    }
    logins
}

/// Fetch one page-set of team members filtered to `role` (`"member"` or
/// `"maintainer"`), fully paginated via [`GitHubClient::request`].
async fn fetch_role(client: &GitHubClient, team_slug: &str, role: &str) -> ApiOutcome {
    let path = format!(
        "/orgs/{}/teams/{}/members?role={}&per_page={}",
        client.org_name,
        team_slug,
        role,
        config::DEFAULT_PAGE_SIZE
    );
    client
        .request(&path, true, 1, config::DEFAULT_REQUEST_TIMEOUT_SECS)
        .await
}

async fn collect_one_team_roster(
    client: &GitHubClient,
    canonical_owner: &str,
    team_slug: &str,
) -> TeamRoster {
    let safe_slug = match cherry_pit_web::sanitize_path_segment(team_slug, "team_slug") {
        Ok(s) => s.into_owned(),
        Err(e) => {
            warn!(team_slug, error = %e, "invalid team slug — skipping roster fetch");
            return degraded_roster(canonical_owner, team_slug, TeamRosterStatus::TransientError);
        }
    };

    let all_outcome = fetch_role(client, &safe_slug, "all").await;
    if !all_outcome.is_ok() {
        warn!(
            team_slug,
            status = ?all_outcome.status_code(),
            "team roster fetch failed"
        );
        return degraded_roster(canonical_owner, team_slug, failure_status(&all_outcome));
    }
    let all_logins = logins_from_outcome(&all_outcome);

    let maintainer_outcome = fetch_role(client, &safe_slug, "maintainer").await;
    if !maintainer_outcome.is_ok() {
        warn!(
            team_slug,
            status = ?maintainer_outcome.status_code(),
            "team maintainer-role fetch failed — roles default to Member"
        );
    }
    let maintainer_logins: std::collections::HashSet<String> = if maintainer_outcome.is_ok() {
        logins_from_outcome(&maintainer_outcome)
    } else {
        Vec::new()
    }
    .into_iter()
    .map(|login| login.to_lowercase())
    .collect();

    let mut members: Vec<TeamMember> = all_logins
        .into_iter()
        .map(|login| {
            let role = if maintainer_logins.contains(&login.to_lowercase()) {
                TeamMemberRole::Maintainer
            } else {
                TeamMemberRole::Member
            };
            TeamMember { login, role }
        })
        .collect();
    members.sort_by_cached_key(|m| m.login.to_lowercase());

    TeamRoster {
        canonical_owner: canonical_owner.to_string(),
        team_slug: team_slug.to_string(),
        status: TeamRosterStatus::Complete,
        members,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::auth::GitHubCredential;
    use crate::github::budget::BudgetGate;
    use std::sync::Arc;
    use std::time::Duration;
    use wiremock::matchers::{path, query_param};
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

    /// A4 regression guard: the roster fetch must be complete for a
    /// multi-page, multi-role team. Reproduces the drop this mission
    /// resolves — a naive fetch (single page, `role=member` only) silently
    /// drops the second page's members and every maintainer (proven red
    /// against the single-page/single-role implementation this test was
    /// first written against).
    #[tokio::test]
    async fn roster_fetch_is_complete_across_pages_and_roles() {
        let server = MockServer::start().await;

        Mock::given(path("/orgs/test-org/teams/big-team/members"))
            .and(query_param("role", "all"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{"login": "alice"}, {"login": "bob"}]))
                    .insert_header(
                        "link",
                        format!("<{}/members-page-2>; rel=\"next\"", server.uri()),
                    ),
            )
            .mount(&server)
            .await;

        Mock::given(path("/members-page-2"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{"login": "carol"}])),
            )
            .mount(&server)
            .await;

        Mock::given(path("/orgs/test-org/teams/big-team/members"))
            .and(query_param("role", "maintainer"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{"login": "alice"}])),
            )
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let rosters = collect_team_rosters(
            &client,
            &[("@test-org/big-team".to_string(), "big-team".to_string())],
        )
        .await;

        assert_eq!(rosters.len(), 1);
        let roster = &rosters[0];
        assert_eq!(roster.status, TeamRosterStatus::Complete);

        let mut logins: Vec<&str> = roster.members.iter().map(|m| m.login.as_str()).collect();
        logins.sort_unstable();
        assert_eq!(
            logins,
            vec!["alice", "bob", "carol"],
            "roster must include every member across every page and role — dropped: {:?}",
            ["alice", "bob", "carol"]
                .iter()
                .filter(|l| !logins.contains(l))
                .collect::<Vec<_>>()
        );

        let alice_role = roster
            .members
            .iter()
            .find(|m| m.login == "alice")
            .map(|m| m.role);
        assert_eq!(
            alice_role,
            Some(TeamMemberRole::Maintainer),
            "alice is a maintainer per the role=maintainer fetch"
        );
        let bob_role = roster
            .members
            .iter()
            .find(|m| m.login == "bob")
            .map(|m| m.role);
        assert_eq!(bob_role, Some(TeamMemberRole::Member));
    }

    /// Permission-denied on the completeness-bearing `all`-role fetch
    /// degrades the roster rather than fabricating a partial one.
    #[tokio::test]
    async fn roster_fetch_permission_denied_degrades_status() {
        let server = MockServer::start().await;

        Mock::given(path("/orgs/test-org/teams/secret-team/members"))
            .and(query_param("role", "all"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let rosters = collect_team_rosters(
            &client,
            &[(
                "@test-org/secret-team".to_string(),
                "secret-team".to_string(),
            )],
        )
        .await;

        assert_eq!(rosters.len(), 1);
        assert_eq!(rosters[0].status, TeamRosterStatus::PermissionDenied);
        assert!(rosters[0].members.is_empty());
    }

    /// A maintainer-role fetch failure degrades role tagging only — the
    /// roster (driven by the `all`-role fetch) stays complete and nobody
    /// is dropped.
    #[tokio::test]
    async fn maintainer_fetch_failure_falls_back_to_member_role_without_dropping_anyone() {
        let server = MockServer::start().await;

        Mock::given(path("/orgs/test-org/teams/flaky-team/members"))
            .and(query_param("role", "all"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{"login": "dan"}])),
            )
            .mount(&server)
            .await;

        Mock::given(path("/orgs/test-org/teams/flaky-team/members"))
            .and(query_param("role", "maintainer"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let rosters = collect_team_rosters(
            &client,
            &[("@test-org/flaky-team".to_string(), "flaky-team".to_string())],
        )
        .await;

        assert_eq!(rosters.len(), 1);
        assert_eq!(rosters[0].status, TeamRosterStatus::Complete);
        assert_eq!(rosters[0].members.len(), 1);
        assert_eq!(rosters[0].members[0].login, "dan");
        assert_eq!(rosters[0].members[0].role, TeamMemberRole::Member);
    }
}
