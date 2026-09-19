//! GitHub team and organization membership collection (B1, item9 Part B).
//!
//! Fetches the complete, role-tagged member roster for each GitHub team
//! referenced by CODEOWNERS, plus (optionally) the current org-members
//! list used to cross-check whether a team member or individual-user
//! CODEOWNERS owner has left the organization. Both fetches run on the
//! decoupled team-refresh tick ([`crate::app::team_refresh`]) via the
//! existing budget/rate-limit-gated [`GitHubClient::request`], not on
//! the repo collect cycle; the collect cycle calls them synchronously
//! only under the `team_roster_read_from_projection = false` rollback
//! seam. The fetched rosters ARE persisted durably, as
//! `TeamStateCaptured` events — the 2026-07-16 amendment to oracle
//! ghr-893fde5c (mission ghr-deb615c4) ratified roster persistence
//! (CHE-0073:R10, CHE-0089). The surviving CLASS B verdict is narrower
//! than "render-time only": no team data enters the
//! `RepositoryStateCaptured` payload, and orphan attribution stays a
//! render-time derivation.

use std::collections::HashSet;

use tracing::{debug, info, warn};

use crate::config;
use crate::domain::metrics::{TeamMember, TeamMemberRole, TeamRoster, TeamRosterStatus};
use crate::github::client::{ApiOutcome, GitHubClient};
use crate::github::dto::{GhOrgMember, GhTeamMember};

/// Why an org-team enumeration failed to establish complete identity
/// coverage. Each variant is a distinct epistemic state, never folded into
/// "no teams" (COM-0028:R2, CHE-0092:R4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryGap {
    /// The enumeration request itself did not succeed.
    Failed,
    /// The enumeration succeeded but stopped before exhausting pagination,
    /// so the slugs returned are a prefix of the org's teams.
    Truncated,
    /// The enumeration succeeded and completed, but at least one entry
    /// carried no usable slug, so the teams it named remain unknown.
    Invalid,
}

/// A team identity that passed the same path-segment boundary the roster
/// fetch itself applies, so it names a team that can actually be read.
///
/// The inner string is private and minted only by validating parse, reached
/// through [`discover_org_teams`]; an unvalidated string therefore has no
/// route into a discovery result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamSlug(String);

impl TeamSlug {
    fn parse(raw: &str) -> Option<Self> {
        cherry_pit_web::sanitize_path_segment(raw, "team_slug")
            .ok()
            .map(|slug| Self(slug.into_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TeamSlug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Outcome of enumerating an organization's teams.
///
/// Omission authority is granted only when the enumeration succeeded,
/// exhausted pagination, and yielded a usable identity for every entry.
/// That authority has no public constructor: the state is private and
/// minted only by [`discover_org_teams`], so no caller can fabricate,
/// relabel, or later mutate a complete enumeration.
///
/// A fabricated complete enumeration does not compile:
///
/// ```compile_fail
/// use gh_report::collector::team_membership::TeamDiscovery;
/// let forged = TeamDiscovery { slugs: Vec::new(), gap: None };
/// ```
///
/// Neither does emptying a legitimately complete one after the fact:
///
/// ```compile_fail
/// # async fn f(client: &gh_report::github::client::GitHubClient) {
/// let mut discovery = gh_report::collector::team_membership::discover_org_teams(client).await;
/// discovery.slugs.clear();
/// # }
/// ```
///
/// Reading the observed identities is always allowed:
///
/// ```
/// # async fn f(client: &gh_report::github::client::GitHubClient) {
/// let discovery = gh_report::collector::team_membership::discover_org_teams(client).await;
/// for slug in discovery.slugs() {
///     let _: &str = slug.as_str();
/// }
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamDiscovery {
    slugs: Vec<TeamSlug>,
    gap: Option<DiscoveryGap>,
}

impl TeamDiscovery {
    fn complete(slugs: Vec<TeamSlug>) -> Self {
        Self { slugs, gap: None }
    }

    fn incomplete(slugs: Vec<TeamSlug>, gap: DiscoveryGap) -> Self {
        Self {
            slugs,
            gap: Some(gap),
        }
    }

    /// Every usable team identity this enumeration observed. Non-empty on
    /// an incomplete enumeration that saw a prefix: those are positive
    /// facts.
    #[must_use]
    pub fn slugs(&self) -> &[TeamSlug] {
        &self.slugs
    }

    /// Why this enumeration established no complete coverage, or `None`
    /// when it did.
    #[must_use]
    pub fn gap(&self) -> Option<DiscoveryGap> {
        self.gap
    }

    /// Whether a known team's omission from this enumeration may be read
    /// as the team's absence — true only for a complete, fully-validated
    /// enumeration.
    #[must_use]
    pub fn authorizes_omission_detach(&self) -> bool {
        self.gap.is_none()
    }
}

/// Enumerate the organization's teams from `/orgs/{org}/teams`.
pub async fn discover_org_teams(client: &GitHubClient) -> TeamDiscovery {
    let path = format!("/orgs/{}/teams?per_page=100", client.org_name);
    let outcome = client
        .request(&path, true, 1, config::DEFAULT_REQUEST_TIMEOUT_SECS)
        .await;
    let Some(items) = outcome.data().and_then(serde_json::Value::as_array) else {
        return TeamDiscovery::incomplete(Vec::new(), DiscoveryGap::Failed);
    };
    let mut slugs = Vec::with_capacity(items.len());
    let mut unusable_entries = 0usize;
    for item in items {
        match item
            .get("slug")
            .and_then(serde_json::Value::as_str)
            .and_then(TeamSlug::parse)
        {
            Some(slug) => slugs.push(slug),
            None => unusable_entries += 1,
        }
    }
    match (outcome.is_truncated(), unusable_entries) {
        (true, _) => {
            warn!(
                observed_teams = slugs.len(),
                "org team enumeration truncated; team absence cannot be inferred"
            );
            TeamDiscovery::incomplete(slugs, DiscoveryGap::Truncated)
        }
        (false, 0) => TeamDiscovery::complete(slugs),
        (false, unusable) => {
            warn!(
                unusable_entries = unusable,
                "org team entries carried no usable identity; team absence cannot be inferred"
            );
            TeamDiscovery::incomplete(slugs, DiscoveryGap::Invalid)
        }
    }
}

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
        fetched_at: None,
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

enum WireEntry<T> {
    Member(T),
    NotAnObject,
    Malformed(serde_json::Error),
}

fn member_entry<T: serde::de::DeserializeOwned>(item: &serde_json::Value) -> WireEntry<T> {
    match item {
        serde_json::Value::Object(_) => match serde_json::from_value::<T>(item.clone()) {
            Ok(member) => WireEntry::Member(member),
            Err(e) => WireEntry::Malformed(e),
        },
        _ => WireEntry::NotAnObject,
    }
}

fn usable_login(login: String) -> Option<String> {
    (!login.trim().is_empty()).then_some(login)
}

/// Parse a successful members-list `ApiOutcome` into logins paired with
/// the role GitHub reported for each, or `None` when the page-set cannot
/// be read as a whole member list.
fn members_from_outcome(outcome: &ApiOutcome) -> Option<Vec<(String, TeamMemberRole)>> {
    let items = outcome.data().and_then(serde_json::Value::as_array)?;
    let mut members = Vec::with_capacity(items.len());
    for item in items {
        let member = match member_entry::<GhTeamMember>(item) {
            WireEntry::Member(member) => member,
            WireEntry::NotAnObject => {
                warn!("team member entry was not a member object — roster completeness unknown");
                return None;
            }
            WireEntry::Malformed(e) => {
                warn!(error = %e, "team member entry unreadable — roster completeness unknown");
                return None;
            }
        };
        let Some(login) = usable_login(member.login) else {
            warn!("team member entry carried no usable login — roster completeness unknown");
            return None;
        };
        let role = role_from_wire(member.role.as_deref(), &login);
        members.push((login, role));
    }
    Some(members)
}

/// Classify GitHub's raw role string.
///
/// Absent and unrecognised both yield [`TeamMemberRole::Unknown`]. They
/// are NOT folded into `Member`: COM-0028:R2 — a missing or unparseable
/// probe result is its own verdict, and calling an unknown role "Member"
/// would assert something GitHub never said, silently downgrading a
/// maintainer we simply failed to read.
fn role_from_wire(role: Option<&str>, login: &str) -> TeamMemberRole {
    match role {
        Some("maintainer") => TeamMemberRole::Maintainer,
        Some("member") => TeamMemberRole::Member,
        Some(other) => {
            warn!(
                login,
                role = other,
                "unrecognised team member role — recording as Unknown"
            );
            TeamMemberRole::Unknown
        }
        None => {
            warn!(
                login,
                "team member entry carried no role — recording as Unknown"
            );
            TeamMemberRole::Unknown
        }
    }
}

/// Fetch one page-set of team members filtered to `role` (`"all"`,
/// `"member"`, or `"maintainer"`), fully paginated via
/// [`GitHubClient::request`].
///
/// `role` filters WHICH members are returned; each returned member
/// carries its own role either way, so `"all"` is the only value this
/// collector needs.
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

    if client.is_known_deleted_team(&safe_slug) {
        debug!(
            team_slug,
            "team known deleted from a prior fetch this process; skipping roster fetch"
        );
        return degraded_roster(canonical_owner, team_slug, TeamRosterStatus::Deleted);
    }

    let all_outcome = fetch_role(client, &safe_slug, "all").await;
    if !all_outcome.is_ok() || all_outcome.is_truncated() {
        let status = failure_status(&all_outcome);
        if status == TeamRosterStatus::Deleted {
            client.record_deleted_team(&safe_slug);
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
    let Some(parsed) = members_from_outcome(&all_outcome) else {
        warn!(
            team_slug,
            "team member list could not be read in full; roster completeness unknown"
        );
        return degraded_roster(canonical_owner, team_slug, TeamRosterStatus::TransientError);
    };
    let fetched_at = jiff::Timestamp::now().to_string();
    let mut members: Vec<TeamMember> = parsed
        .into_iter()
        .map(|(login, role)| TeamMember {
            login,
            role,
            in_org: None,
        })
        .collect();
    members.sort_by_cached_key(|m| m.login.to_lowercase());

    TeamRoster {
        fetched_at: Some(fetched_at),
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
/// degrade to `None` (item9 H1, ghr-a1fa33bd).
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
        org_member_logins_from_outcome(outcome)?
            .into_iter()
            .map(|login| login.to_lowercase())
            .collect(),
    )
}

/// Parse a successful org-members-list `ApiOutcome` into a list of logins,
/// or `None` when the page-set cannot be read as a whole member list.
fn org_member_logins_from_outcome(outcome: &ApiOutcome) -> Option<Vec<String>> {
    let items = outcome.data().and_then(serde_json::Value::as_array)?;
    let mut logins = Vec::with_capacity(items.len());
    for item in items {
        let member = match member_entry::<GhOrgMember>(item) {
            WireEntry::Member(member) => member,
            WireEntry::NotAnObject => {
                warn!("org member entry was not a member object — org membership unknown");
                return None;
            }
            WireEntry::Malformed(e) => {
                warn!(error = %e, "org member entry unreadable — org membership unknown");
                return None;
            }
        };
        let Some(login) = usable_login(member.login) else {
            warn!("org member entry carried no usable login — org membership unknown");
            return None;
        };
        logins.push(login);
    }
    Some(logins)
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
    /// multi-page, multi-role team. Reproduces the drop the original
    /// mission resolved — a naive fetch (single page, `role=member` only)
    /// silently drops the second page's members and every maintainer.
    ///
    /// The observable outcomes asserted here are unchanged from the
    /// two-fetch era (alice is a Maintainer, bob a Member, carol survives
    /// pagination); only the source of the role moved, from a second
    /// `role=maintainer` page-set to the role field the `role=all`
    /// response already carried.
    #[tokio::test]
    async fn roster_fetch_is_complete_across_pages_and_roles() {
        let server = MockServer::start().await;

        Mock::given(path("/orgs/test-org/teams/big-team/members"))
            .and(query_param("role", "all"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([
                        {"login": "alice", "role": "maintainer"},
                        {"login": "bob", "role": "member"},
                    ]))
                    .insert_header(
                        "link",
                        format!("<{}/members-page-2>; rel=\"next\"", server.uri()),
                    ),
            )
            .mount(&server)
            .await;

        Mock::given(path("/members-page-2"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{"login": "carol", "role": "member"}])),
            )
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let before_fetch = jiff::Timestamp::now();
        let rosters = collect_team_rosters(
            &client,
            &[("@test-org/big-team".to_string(), "big-team".to_string())],
        )
        .await;

        assert_eq!(rosters.len(), 1);
        let roster = &rosters[0];
        assert_eq!(roster.status, TeamRosterStatus::Complete);
        let captured: jiff::Timestamp = roster
            .fetched_at
            .as_deref()
            .expect("successful live fetch has an actual observation instant")
            .parse()
            .unwrap();
        assert!(captured >= before_fetch && captured <= jiff::Timestamp::now());

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
            "alice is a maintainer per the role her role=all entry carried"
        );
        let bob_role = roster
            .members
            .iter()
            .find(|m| m.login == "bob")
            .map(|m| m.role);
        assert_eq!(bob_role, Some(TeamMemberRole::Member));
    }

    #[tokio::test]
    async fn roster_page_cap_degrades_without_fresh_complete_observation() {
        let server = MockServer::start().await;
        let members_path = "/orgs/test-org/teams/big-team/members";
        Mock::given(path(members_path))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{"login": "alice", "role": "member"}]))
                    .insert_header(
                        "link",
                        format!("<{}{members_path}>; rel=\"next\"", server.uri()),
                    ),
            )
            .expect(u64::try_from(config::MAX_PAGINATION_PAGES).unwrap())
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let roster = collect_one_team_roster(&client, "@test-org/big-team", "big-team").await;
        server.verify().await;
        assert_eq!(
            (roster.status, roster.fetched_at, roster.members.len()),
            (TeamRosterStatus::TransientError, None, 0),
            "page-capped success must not become a fresh complete roster"
        );
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

    /// An unreadable role degrades that member's ROLE only: the roster
    /// stays Complete and nobody is dropped.
    ///
    /// Replaces the former `maintainer_fetch_failure_falls_back_to_member`
    /// test, whose failure mode (a failed second fetch) no longer exists.
    /// The outcome it protected — a role problem must never cost a member
    /// their place on the roster — is asserted here, minus the silent
    /// fallback to Member that COM-0028:R2 forbids.
    #[tokio::test]
    async fn an_unreadable_role_degrades_the_role_only_and_drops_nobody() {
        let server = MockServer::start().await;

        Mock::given(path("/orgs/test-org/teams/flaky-team/members"))
            .and(query_param("role", "all"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"login": "dan", "role": "wrangler"},
            ])))
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
        assert_eq!(rosters[0].members[0].role, TeamMemberRole::Unknown);
    }

    /// GUARD-BITE (AGENTS.md § Code-quality methods rule 2). Plants a
    /// real violation: one member whose entry carries NO `role` key at
    /// all, and one whose role is a value this build does not recognise.
    /// Both must surface as [`TeamMemberRole::Unknown`].
    ///
    /// Proven to bite: run against the `role=maintainer`-set-membership
    /// implementation — which classified every non-maintainer as
    /// `Member` — this assertion FAILS on both members, because "absent"
    /// and "unrecognised" were indistinguishable from "ordinary member".
    /// That silent default is precisely what COM-0028:R2 forbids.
    #[tokio::test]
    async fn absent_or_unrecognised_role_becomes_unknown_never_member() {
        let server = MockServer::start().await;

        Mock::given(path("/orgs/test-org/teams/odd-team/members"))
            .and(query_param("role", "all"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"login": "alice", "role": "maintainer"},
                {"login": "bob", "role": "member"},
                {"login": "carol"},
                {"login": "dave", "role": "triage-lead"},
            ])))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let rosters = collect_team_rosters(
            &client,
            &[("@test-org/odd-team".to_string(), "odd-team".to_string())],
        )
        .await;

        let roles: Vec<(&str, TeamMemberRole)> = rosters[0]
            .members
            .iter()
            .map(|m| (m.login.as_str(), m.role))
            .collect();
        assert_eq!(
            roles,
            vec![
                ("alice", TeamMemberRole::Maintainer),
                ("bob", TeamMemberRole::Member),
                ("carol", TeamMemberRole::Unknown),
                ("dave", TeamMemberRole::Unknown),
            ],
            "a missing `role` key and an unrecognised role value must each \
             surface as Unknown; reporting either as Member would state a \
             fact GitHub never told us (COM-0028:R2)"
        );
    }

    /// Exactly ONE paginated fetch per team. `.expect(1)` plus
    /// `server.verify()` fails the test if the members path is hit twice,
    /// and no `role=maintainer` mock exists, so the deleted second fetch
    /// cannot quietly return.
    #[tokio::test]
    async fn one_paginated_members_fetch_per_team_and_no_maintainer_fetch() {
        let server = MockServer::start().await;

        Mock::given(path("/orgs/test-org/teams/one-fetch/members"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"login": "alice", "role": "maintainer"},
                {"login": "bob", "role": "member"},
            ])))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let rosters = collect_team_rosters(
            &client,
            &[("@test-org/one-fetch".to_string(), "one-fetch".to_string())],
        )
        .await;

        assert_eq!(rosters[0].status, TeamRosterStatus::Complete);
        assert_eq!(rosters[0].members.len(), 2);
        server.verify().await;
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

    /// item9 Part B test (e), H1 fix (ghr-a1fa33bd): a truncated-but-
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
            fetched_at: None,
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
            fetched_at: None,
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
        let org_members: HashSet<String> = HashSet::from(["alice".to_string()]);

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
            fetched_at: None,
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

    #[tokio::test]
    async fn deleted_team_second_fetch_short_circuits_without_a_second_api_call() {
        let server = MockServer::start().await;

        Mock::given(path("/orgs/test-org/teams/gone-team/members"))
            .and(query_param("role", "all"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let teams = [("@test-org/gone-team".to_string(), "gone-team".to_string())];

        let first = collect_team_rosters(&client, &teams).await;
        assert_eq!(first[0].status, TeamRosterStatus::Deleted);

        let second = collect_team_rosters(&client, &teams).await;
        assert_eq!(
            second[0].status,
            TeamRosterStatus::Deleted,
            "cache short-circuit must still report Deleted"
        );

        server.verify().await;
    }

    #[tokio::test]
    async fn non_deleted_team_is_not_cached() {
        let server = MockServer::start().await;

        Mock::given(path("/orgs/test-org/teams/secret-team-2/members"))
            .and(query_param("role", "all"))
            .respond_with(ResponseTemplate::new(403))
            .expect(2)
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let teams = [(
            "@test-org/secret-team-2".to_string(),
            "secret-team-2".to_string(),
        )];

        collect_team_rosters(&client, &teams).await;
        collect_team_rosters(&client, &teams).await;

        server.verify().await;
    }

    async fn discovery_against(server: &MockServer, response: ResponseTemplate) -> TeamDiscovery {
        Mock::given(path("/orgs/test-org/teams"))
            .respond_with(response)
            .mount(server)
            .await;
        discover_org_teams(&test_client(&server.uri())).await
    }

    fn slug_strings(discovery: &TeamDiscovery) -> Vec<String> {
        discovery
            .slugs()
            .iter()
            .map(std::string::ToString::to_string)
            .collect()
    }

    #[tokio::test]
    async fn failed_enumeration_classifies_as_failed_gap_not_empty_team_set() {
        let server = MockServer::start().await;
        let discovery = discovery_against(&server, ResponseTemplate::new(403)).await;
        assert_eq!(
            (slug_strings(&discovery), discovery.gap()),
            (Vec::new(), Some(DiscoveryGap::Failed)),
            "a failed enumeration must not be indistinguishable from an org with no teams"
        );
        assert!(!discovery.authorizes_omission_detach());
    }

    #[tokio::test]
    async fn truncated_enumeration_retains_observed_prefix_without_authority() {
        let server = MockServer::start().await;
        let teams_path = "/orgs/test-org/teams";
        Mock::given(path(teams_path))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{"slug": "visible"}]))
                    .insert_header(
                        "link",
                        format!("<{}{teams_path}>; rel=\"next\"", server.uri()),
                    ),
            )
            .expect(u64::try_from(config::MAX_PAGINATION_PAGES).unwrap())
            .mount(&server)
            .await;

        let discovery = discover_org_teams(&test_client(&server.uri())).await;
        server.verify().await;

        assert!(!discovery.authorizes_omission_detach());
        assert_eq!(
            discovery.slugs().first().map(TeamSlug::as_str),
            Some("visible"),
            "the prefix a truncated enumeration DID observe is still a positive fact"
        );
        assert_eq!(discovery.gap(), Some(DiscoveryGap::Truncated));
    }

    #[tokio::test]
    async fn entry_without_slug_classifies_invalid_while_keeping_valid_slugs() {
        let server = MockServer::start().await;
        let discovery = discovery_against(
            &server,
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!([{"slug": "alpha"}, {"name": "no-slug"}])),
        )
        .await;
        assert_eq!(
            (slug_strings(&discovery), discovery.gap()),
            (vec!["alpha".to_string()], Some(DiscoveryGap::Invalid)),
            "a skipped entry leaves coverage unproven but does not discard the slug read"
        );
    }

    #[tokio::test]
    async fn path_invalid_slugs_are_excluded_and_leave_coverage_unproven() {
        let server = MockServer::start().await;
        let discovery = discovery_against(
            &server,
            ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"slug": "alpha"},
                {"slug": "../bad"},
                {"slug": ""},
                {"slug": "x/y"},
                {"slug": "ctl\u{0001}"},
                {"slug": 7},
                {"slug": null},
            ])),
        )
        .await;
        assert!(
            !discovery.authorizes_omission_detach(),
            "an entry whose slug is not a usable identity leaves the org's team \
             set unproven: the team it named was never actually read"
        );
        assert_eq!(
            slug_strings(&discovery),
            vec!["alpha".to_string()],
            "a genuinely valid slug from the same response is still a positive \
             fact and must survive alongside the rejected entries"
        );
    }

    #[tokio::test]
    async fn complete_empty_enumeration_is_an_absence_fact() {
        let server = MockServer::start().await;
        let discovery = discovery_against(
            &server,
            ResponseTemplate::new(200).set_body_json(serde_json::json!([])),
        )
        .await;
        assert_eq!(slug_strings(&discovery), Vec::<String>::new());
        assert_eq!(discovery.gap(), None);
        assert!(
            discovery.authorizes_omission_detach(),
            "a complete enumeration returning no teams genuinely establishes absence"
        );
    }

    async fn team_member_page_status(slug: &str, body: serde_json::Value) -> TeamRosterStatus {
        let server = MockServer::start().await;
        Mock::given(path(format!("/orgs/test-org/teams/{slug}/members")))
            .and(query_param("role", "all"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let rosters =
            collect_team_rosters(&client, &[(format!("@test-org/{slug}"), slug.to_string())]).await;
        rosters[0].status
    }

    fn org_member_page_set(body: serde_json::Value) -> Option<HashSet<String>> {
        org_members_from_outcome(&ApiOutcome::Success {
            status_code: 200,
            data: Some(body),
            headers: None,
            truncated: false,
        })
    }

    #[tokio::test]
    async fn positional_array_team_member_entry_degrades_roster_status() {
        assert_ne!(
            team_member_page_status(
                "positional-team",
                serde_json::json!([["alice", null, null]]),
            )
            .await,
            TeamRosterStatus::Complete,
            "a positional array is not a member object and must not yield a login"
        );
    }

    #[test]
    fn positional_array_org_member_entry_degrades_set_to_none() {
        assert_eq!(
            org_member_page_set(serde_json::json!([["alice"]])),
            None,
            "a positional array is not a member object and must not yield a login"
        );
    }

    #[tokio::test]
    async fn unusable_team_member_logins_all_degrade_roster_status() {
        for (slug, entry) in [
            ("missing-login", serde_json::json!({"id": 7})),
            ("null-login", serde_json::json!({"login": null})),
            ("nonstring-login", serde_json::json!({"login": 7})),
            ("empty-login", serde_json::json!({"login": ""})),
            ("blank-login", serde_json::json!({"login": "   "})),
        ] {
            assert_ne!(
                team_member_page_status(
                    slug,
                    serde_json::json!([{"login": "alice", "role": "member"}, entry]),
                )
                .await,
                TeamRosterStatus::Complete,
                "{slug}: an entry without a usable login cannot complete a roster"
            );
        }
    }

    #[test]
    fn unusable_org_member_logins_all_degrade_set_to_none() {
        for entry in [
            serde_json::json!({"id": 7}),
            serde_json::json!({"login": null}),
            serde_json::json!({"login": 7}),
            serde_json::json!({"login": ""}),
            serde_json::json!({"login": "   "}),
        ] {
            assert_eq!(
                org_member_page_set(serde_json::json!([{"login": "alice"}, entry.clone()])),
                None,
                "{entry}: an entry without a usable login cannot be authoritative"
            );
        }
    }

    #[tokio::test]
    async fn malformed_team_member_entry_degrades_roster_status() {
        let server = MockServer::start().await;
        Mock::given(path("/orgs/test-org/teams/big-team/members"))
            .and(query_param("role", "all"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"login": "alice", "role": "member"},
                {"id": 7}
            ])))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let rosters = collect_team_rosters(
            &client,
            &[("@test-org/big-team".to_string(), "big-team".to_string())],
        )
        .await;

        assert_ne!(
            rosters[0].status,
            TeamRosterStatus::Complete,
            "a member list carrying an unusable identity cannot be a complete roster"
        );
        assert!(
            rosters[0].members.is_empty(),
            "a degraded roster carries no members"
        );
    }

    #[tokio::test]
    async fn blank_team_member_login_degrades_roster_status() {
        let server = MockServer::start().await;
        Mock::given(path("/orgs/test-org/teams/blank-team/members"))
            .and(query_param("role", "all"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"login": "   ", "role": "member"}
            ])))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let rosters = collect_team_rosters(
            &client,
            &[("@test-org/blank-team".to_string(), "blank-team".to_string())],
        )
        .await;

        assert_ne!(rosters[0].status, TeamRosterStatus::Complete);
    }

    #[tokio::test]
    async fn ordinary_roster_with_unknown_role_stays_complete() {
        let server = MockServer::start().await;
        Mock::given(path("/orgs/test-org/teams/ok-team/members"))
            .and(query_param("role", "all"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"login": "alice", "role": "member"},
                {"login": "bob"}
            ])))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let rosters = collect_team_rosters(
            &client,
            &[("@test-org/ok-team".to_string(), "ok-team".to_string())],
        )
        .await;

        assert_eq!(rosters[0].status, TeamRosterStatus::Complete);
        assert_eq!(rosters[0].members.len(), 2);
        assert_eq!(
            rosters[0]
                .members
                .iter()
                .find(|m| m.login == "bob")
                .map(|m| m.role),
            Some(TeamMemberRole::Unknown),
            "an absent role stays Unknown; it is not a malformed identity"
        );
    }

    #[tokio::test]
    async fn genuinely_empty_team_stays_complete() {
        let server = MockServer::start().await;
        Mock::given(path("/orgs/test-org/teams/empty-team/members"))
            .and(query_param("role", "all"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let rosters = collect_team_rosters(
            &client,
            &[("@test-org/empty-team".to_string(), "empty-team".to_string())],
        )
        .await;

        assert_eq!(rosters[0].status, TeamRosterStatus::Complete);
        assert!(rosters[0].members.is_empty());
    }

    #[tokio::test]
    async fn non_array_member_body_degrades_roster_status() {
        let server = MockServer::start().await;
        Mock::given(path("/orgs/test-org/teams/weird-team/members"))
            .and(query_param("role", "all"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"message": "nope"})),
            )
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let rosters = collect_team_rosters(
            &client,
            &[("@test-org/weird-team".to_string(), "weird-team".to_string())],
        )
        .await;

        assert_ne!(rosters[0].status, TeamRosterStatus::Complete);
    }

    #[test]
    fn malformed_org_member_entry_degrades_set_to_none() {
        let outcome = ApiOutcome::Success {
            status_code: 200,
            data: Some(serde_json::json!([{"login": "alice"}, {"id": 7}])),
            headers: None,
            truncated: false,
        };

        assert_eq!(
            org_members_from_outcome(&outcome),
            None,
            "a member list with an unreadable entry is not an authoritative org roster"
        );
    }

    #[test]
    fn well_formed_org_member_lists_stay_authoritative() {
        let populated = ApiOutcome::Success {
            status_code: 200,
            data: Some(serde_json::json!([{"login": "Alice"}])),
            headers: None,
            truncated: false,
        };
        let empty = ApiOutcome::Success {
            status_code: 200,
            data: Some(serde_json::json!([])),
            headers: None,
            truncated: false,
        };

        assert_eq!(
            org_members_from_outcome(&populated),
            Some(HashSet::from(["alice".to_string()]))
        );
        assert_eq!(org_members_from_outcome(&empty), Some(HashSet::new()));
    }

    #[tokio::test]
    async fn malformed_org_members_leave_in_org_unknown_through_enrichment() {
        let server = MockServer::start().await;
        Mock::given(path("/orgs/test-org/members"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"login": "alice"},
                ["bob"]
            ])))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let org_members = collect_org_members(&client).await;
        assert_eq!(org_members, None);

        let mut rosters = vec![TeamRoster {
            fetched_at: None,
            canonical_owner: "@test-org/team-a".to_string(),
            team_slug: "team-a".to_string(),
            status: TeamRosterStatus::Complete,
            members: vec![TeamMember {
                login: "bob".to_string(),
                role: TeamMemberRole::Member,
                in_org: None,
            }],
        }];
        enrich_team_rosters_with_org_membership(&mut rosters, org_members.as_ref());

        assert_eq!(
            rosters[0].members[0].in_org, None,
            "an unreadable org-members page must flag nobody as departed"
        );
    }
}
