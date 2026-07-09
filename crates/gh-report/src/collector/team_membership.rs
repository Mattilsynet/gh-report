//! GitHub team and organization membership collection (B1, item9 Part B).
//!
//! Fetches the complete, role-tagged member roster for each GitHub team
//! referenced by CODEOWNERS, plus (optionally) the current org-members
//! list used to cross-check whether a team member or individual-user
//! CODEOWNERS owner has left the organization. Render-time only (oracle
//! adr-fmt-kqavx CLASS B verdict): rosters and the org-members set are
//! fetched fresh every collection tick via the existing
//! budget/rate-limit-gated [`GitHubClient::request`] and are never
//! persisted to the native per-repo event payload.

use std::collections::HashSet;

use tracing::{info, warn};

use crate::config;
use crate::domain::metrics::{TeamMember, TeamMemberRole, TeamRoster, TeamRosterStatus};
use crate::github::client::{ApiOutcome, GitHubClient};
use crate::github::dto::{GhOrgMember, GhTeamMember};

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
///
/// A 404 means the team itself is gone (a CODEOWNERS reference to a team
/// GitHub has deleted); a 403 means the team may still exist but access was
/// denied. These are distinct outcomes for both logging (a 404 is routine
/// and should not warn) and rendering (`Deleted` vs `Permission denied`).
fn failure_status(outcome: &ApiOutcome) -> TeamRosterStatus {
    match outcome.status_code() {
        Some(404) => TeamRosterStatus::Deleted,
        Some(403) => TeamRosterStatus::PermissionDenied,
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
        let status = failure_status(&all_outcome);
        if status == TeamRosterStatus::Deleted {
            info!(team_slug, "team no longer exists on GitHub; skipping");
        } else {
            warn!(
                team_slug,
                status = ?all_outcome.status_code(),
                "team roster fetch failed"
            );
        }
        return degraded_roster(canonical_owner, team_slug, status);
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
            TeamMember {
                login,
                role,
                in_org: None,
            }
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

/// Fetch the organization's current member logins (item9 Part B).
///
/// Optional capability: mirrors
/// [`crate::collector::ghas_scanning::collect_org_alerts`]'s shape exactly
/// — same paginated, budget/rate-limit-gated [`GitHubClient::request`]
/// call, same graceful-degradation-on-failure discipline. Degrades to
/// `None` on any fetch failure (403, network error, transient 5xx, etc.)
/// *or* a truncated-but-technically-successful paginated fetch (adr-fmt-
/// jlfs1 H1) rather than returning an empty or partial set — an
/// incomplete set is indistinguishable from "the org genuinely has zero
/// (or only these) members", and callers must never read a failed or
/// truncated fetch as "everyone else left the org". See
/// [`org_members_from_outcome`] for the degrade decision.
///
/// The returned set is lowercased so callers can cross-check a login with
/// a single `.to_lowercase()` on their side (`alice` matches `Alice`).
pub async fn collect_org_members(client: &GitHubClient) -> Option<HashSet<String>> {
    let path = format!(
        "/orgs/{}/members?per_page={}",
        client.org_name,
        config::DEFAULT_PAGE_SIZE
    );
    let outcome = client
        .request(&path, true, 1, config::DEFAULT_REQUEST_TIMEOUT_SECS)
        .await;

    org_members_from_outcome(&outcome)
}

/// Decide the org-members set from an already-fetched `ApiOutcome`, or
/// degrade to `None` (item9 H1, adr-fmt-jlfs1).
///
/// Degrades on ANY of: outright failure (`!outcome.is_ok()`), OR a
/// truncated-but-technically-successful paginated fetch
/// (`outcome.is_truncated()` — pagination-page cap, paginated-item cap,
/// or a concurrent rate-limit/budget halt mid-pagination all surface
/// this way per [`GitHubClient`]'s `request_paginated`). A truncated
/// fetch's `is_ok()` is `true`, so checking `is_ok()` alone is not
/// sufficient — a partial set is exactly as dangerous as no set at all: a
/// genuine current member who happens to live on an unfetched page would
/// otherwise be falsely flagged as departed. Mirrors the `is_truncated()`
/// precedent at [`crate::collector::inventory`]'s `InventoryPayload`
/// construction (`complete: !response.is_truncated()`).
fn org_members_from_outcome(outcome: &ApiOutcome) -> Option<HashSet<String>> {
    if !outcome.is_ok() || outcome.is_truncated() {
        warn!(
            status = ?outcome.status_code(),
            retryable = outcome.is_retryable(),
            truncated = outcome.is_truncated(),
            "org members fetch failed or truncated — degrading to unknown (no departure flags this run)"
        );
        return None;
    }

    Some(
        org_member_logins_from_outcome(outcome)
            .into_iter()
            .map(|login| login.to_lowercase())
            .collect(),
    )
}

/// Parse a successful org-members-list `ApiOutcome` into a list of logins.
///
/// Entries that fail to parse as [`GhOrgMember`] are skipped and logged;
/// they do not fail the whole fetch. Mirrors [`logins_from_outcome`]
/// exactly, against the org-members DTO instead of the team-members DTO.
fn org_member_logins_from_outcome(outcome: &ApiOutcome) -> Vec<String> {
    let items = outcome
        .data()
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut logins = Vec::with_capacity(items.len());
    for item in items {
        match serde_json::from_value::<GhOrgMember>(item) {
            Ok(member) => logins.push(member.login),
            Err(e) => warn!(error = %e, "failed to parse org member entry — skipping"),
        }
    }
    logins
}

/// Cross-check every team member's login against the org-members set,
/// setting [`TeamMember::in_org`] in place (item9 Part B).
///
/// `org_members` is `None` when the org-members fetch was unfetched or
/// degraded — every member's `in_org` is set to `None` in that case (no
/// flag on missing data, per [`collect_org_members`]'s contract). When
/// `Some`, both sides of the comparison are lowercased (`alice` in the set
/// matches login `Alice`).
pub(crate) fn enrich_team_rosters_with_org_membership(
    rosters: &mut [TeamRoster],
    org_members: Option<&HashSet<String>>,
) {
    for roster in rosters.iter_mut() {
        for member in &mut roster.members {
            member.in_org = org_members.map(|set| set.contains(&member.login.to_lowercase()));
        }
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

    fn failing_outcome(status: Option<u16>) -> ApiOutcome {
        ApiOutcome::Failure {
            status_code: status,
            error: "simulated failure".to_string(),
            retryable: status.is_none(),
        }
    }

    #[test]
    fn failure_status_maps_404_to_deleted_403_to_permission_denied_other_to_transient() {
        assert_eq!(
            failure_status(&failing_outcome(Some(404))),
            TeamRosterStatus::Deleted
        );
        assert_eq!(
            failure_status(&failing_outcome(Some(403))),
            TeamRosterStatus::PermissionDenied
        );
        assert_eq!(
            failure_status(&failing_outcome(Some(500))),
            TeamRosterStatus::TransientError
        );
        assert_eq!(
            failure_status(&failing_outcome(None)),
            TeamRosterStatus::TransientError
        );
    }

    #[test]
    fn deleted_status_is_distinct_from_permission_denied() {
        assert_ne!(
            failure_status(&failing_outcome(Some(404))),
            failure_status(&failing_outcome(Some(403))),
            "404 (deleted team) and 403 (permission denied) must classify distinctly"
        );
    }

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

    /// item9 Part B test (d): the org-members fetch is complete across a
    /// multi-page, Link-header-paginated response — mirrors
    /// [`roster_fetch_is_complete_across_pages_and_roles`]'s pagination
    /// shape, proving `collect_org_members` inherits
    /// `request_paginated`'s Link-header-driven pagination loop rather
    /// than reading only the first page.
    #[tokio::test]
    async fn collect_org_members_is_complete_across_pages() {
        let server = MockServer::start().await;

        Mock::given(path("/orgs/test-org/members"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{"login": "Alice"}, {"login": "bob"}]))
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

        let client = test_client(&server.uri());
        let members = collect_org_members(&client)
            .await
            .expect("fetch should succeed");

        let mut logins: Vec<&String> = members.iter().collect();
        logins.sort_unstable();
        assert_eq!(
            logins,
            vec!["alice", "bob", "carol"],
            "org-members set must be complete across every page, and lowercased"
        );
    }

    /// item9 Part B test (d): a failed org-members fetch degrades to
    /// `None` — mirrors [`roster_fetch_permission_denied_degrades_status`]'s
    /// degradation discipline. `None`, not an empty set, so callers never
    /// read "fetch failed" as "org has zero members".
    #[tokio::test]
    async fn collect_org_members_degrades_to_none_on_failure() {
        let server = MockServer::start().await;

        Mock::given(path("/orgs/test-org/members"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let members = collect_org_members(&client).await;

        assert_eq!(
            members, None,
            "a degraded fetch must yield None, never an empty set"
        );
    }

    /// item9 Part B test (e), H1 fix (adr-fmt-jlfs1): a truncated-but-
    /// technically-successful paginated fetch (`ApiOutcome::Success {
    /// truncated: true, .. }` — `is_ok()` is `true`, so the pre-fix guard
    /// `!outcome.is_ok()` alone would NOT degrade this) must still yield
    /// `None`, never `Some(partial_set)`. The outcome carries a real
    /// member login ("alice") to prove this isn't merely an
    /// empty-response case caught by some other path — a truncated
    /// response WITH real data must still degrade fully, because a
    /// genuine current member could sit on the unfetched remainder.
    #[test]
    fn org_members_from_outcome_degrades_on_truncation_even_with_real_data() {
        let truncated = ApiOutcome::Success {
            status_code: 200,
            data: Some(serde_json::json!([{"login": "alice"}])),
            headers: None,
            truncated: true,
        };

        assert_eq!(
            org_members_from_outcome(&truncated),
            None,
            "a truncated paginated fetch must degrade to None, never Some(partial), \
             even when the partial data contains real members"
        );
    }

    /// item9 Part B test (e), enrichment-layer proof: chaining a
    /// truncated outcome's `None` result into
    /// [`enrich_team_rosters_with_org_membership`] confirms the whole
    /// path — not just the fetch function in isolation — leaves a real,
    /// present member's `in_org` at `None`, never falsely `Some(false)`
    /// (departed).
    #[test]
    fn truncated_fetch_flags_nobody_through_the_full_enrichment_chain() {
        let truncated = ApiOutcome::Success {
            status_code: 200,
            data: Some(serde_json::json!([{"login": "alice"}])),
            headers: None,
            truncated: true,
        };
        let org_members = org_members_from_outcome(&truncated);

        let mut rosters = vec![TeamRoster {
            canonical_owner: "@test-org/team-a".to_string(),
            team_slug: "team-a".to_string(),
            status: TeamRosterStatus::Complete,
            members: vec![TeamMember {
                login: "alice".to_string(),
                role: TeamMemberRole::Member,
                in_org: None,
            }],
        }];

        enrich_team_rosters_with_org_membership(&mut rosters, org_members.as_ref());

        assert_eq!(
            rosters[0].members[0].in_org, None,
            "a genuine present member must not be flagged departed just because \
             the org-members fetch that would have confirmed them was truncated"
        );
    }

    /// item9 Part B test (b): a team member NOT in the org-members set is
    /// flagged `in_org = Some(false)`; one IN the set is `Some(true)`.
    /// Comparison is lowercase on both sides — a set entry `"alice"`
    /// matches login `"Alice"`.
    #[test]
    fn enrich_team_rosters_flags_departed_member_and_clears_present_member() {
        let mut rosters = vec![TeamRoster {
            canonical_owner: "@test-org/team-a".to_string(),
            team_slug: "team-a".to_string(),
            status: TeamRosterStatus::Complete,
            members: vec![
                TeamMember {
                    login: "Alice".to_string(),
                    role: TeamMemberRole::Member,
                    in_org: None,
                },
                TeamMember {
                    login: "departed-bob".to_string(),
                    role: TeamMemberRole::Member,
                    in_org: None,
                },
            ],
        }];
        let org_members: HashSet<String> = ["alice".to_string()].into_iter().collect();

        enrich_team_rosters_with_org_membership(&mut rosters, Some(&org_members));

        let members = &rosters[0].members;
        assert_eq!(
            members[0].in_org,
            Some(true),
            "'Alice' must match lowercased set entry 'alice'"
        );
        assert_eq!(
            members[1].in_org,
            Some(false),
            "'departed-bob' is absent from the set — flagged departed"
        );
    }

    /// item9 Part B test (c): when the org-members fetch degraded
    /// (`org_members: None`), no member is flagged — every `in_org` stays
    /// `None`, not `Some(false)`. This is the whole point: absence of the
    /// list must never be read as "everyone departed".
    #[test]
    fn enrich_team_rosters_flags_nobody_when_org_members_degraded() {
        let mut rosters = vec![TeamRoster {
            canonical_owner: "@test-org/team-a".to_string(),
            team_slug: "team-a".to_string(),
            status: TeamRosterStatus::Complete,
            members: vec![TeamMember {
                login: "alice".to_string(),
                role: TeamMemberRole::Member,
                in_org: None,
            }],
        }];

        enrich_team_rosters_with_org_membership(&mut rosters, None);

        assert_eq!(
            rosters[0].members[0].in_org, None,
            "degraded org-members fetch must not flag anyone"
        );
    }
}
