//! Team-refresh writer: a dedicated per-team collector that persists
//! `TeamStateCaptured` events on a cadence decoupled from the repo
//! collect cycle (adr-fmt-ewc1i, roadmap adr-fmt-se2xh §C(3)/§E Phase 3).
//!
//! This severs the repo-snapshot↔roster-fetch coupling that was the
//! root of the unresolved-by-timing raciness (adr-fmt-se2xh §A): rosters
//! are fetched and durably recorded on their own timer, independent of
//! whether a repo collect cycle is in flight. Render (P5, adr-fmt-47ljf)
//! will read the persisted, folded projection instead of calling
//! [`crate::collector::team_membership::collect_team_rosters`]
//! synchronously inside the collect cycle.

use std::collections::BTreeSet;
use std::sync::Arc;

use tracing::{info, warn};

use crate::app::state::AppState;
use crate::app::write_policy::{
    WriteFailureContext, WriteFailureContextOwned, log_write_failure, write_with_policy,
};
use crate::collector::team_membership;
use crate::domain::metrics::{TeamRoster, TeamRosterStatus, team_owner_slugs};
use crate::error::AppError;
use crate::event::{OrgMembershipFetchStatus, team_domain_key};
use crate::github::client::GitHubClient;

/// A tick-level write failure paired with the [`WriteFailureContext`]
/// observed at the failing `write_team_event` call site, threaded up to
/// [`log_tick_failure`] one frame above.
#[derive(Debug)]
pub struct TickFailure {
    pub error: AppError,
    pub context: WriteFailureContextOwned,
}

/// Run one team-refresh tick: fetch current rosters for every team the
/// current repo projection references, persist each as a
/// `TeamStateCaptured` event (OCC-fenced, persist-then-fold — see
/// [`AppState::record_team`]), and detach any team the projection
/// previously recorded that no longer owns any repository.
///
/// A freshly-fetched roster whose status is
/// [`TeamRosterStatus::Deleted`] (the team itself no longer exists on
/// GitHub) routes to [`AppState::detach_team`] instead of
/// [`AppState::record_team`] even when it is still CODEOWNERS-referenced
/// (CHE-0092:R1/R2) — a `Deleted` roster observation is a no-op-on-
/// convergence signal, not a live upsert; re-recording it every tick is
/// a wasteful OCC fence write with no projection effect once anti-
/// downgrade guarding is in place.
///
/// # Errors
///
/// Returns the first fatal [`TickFailure`] — pairing the classified
/// [`AppError`] (a single-writer fence conflict, a structural store
/// invariant violation, or an unrecoverable store state) with the
/// [`WriteFailureContext`] observed at the failing write — classified
/// by the durable-write policy (CHE-0088). No in-band retry masks a
/// conflict (PGN-0016:R1/R2/R10); the caller (the decoupled cadence
/// loop) is responsible for logging and waiting for the next tick.
pub async fn run_team_refresh_tick(
    state: &Arc<AppState>,
    client: &GitHubClient,
    fetched_at: &str,
) -> Result<(), TickFailure> {
    let org = client.org_name.clone();
    let evidence_repos = state.projection_snapshot();
    let team_pairs = team_owner_slugs(&evidence_repos);

    let current_keys: BTreeSet<String> = team_pairs
        .iter()
        .filter_map(|(_, team_slug)| team_domain_key(&org, team_slug).ok())
        .collect();

    let mut rosters = team_membership::collect_team_rosters(client, &team_pairs).await;
    let org_members = team_membership::collect_org_members(client).await;
    let org_membership_fetch_status = if org_members.is_some() {
        OrgMembershipFetchStatus::Fetched
    } else {
        OrgMembershipFetchStatus::Degraded
    };
    team_membership::enrich_team_rosters_with_org_membership(&mut rosters, org_members.as_ref());

    for roster in &rosters {
        let detach = roster.status == TeamRosterStatus::Deleted;
        write_team_event(
            state,
            &org,
            roster,
            fetched_at,
            org_membership_fetch_status,
            detach,
        )
        .await?;
    }

    for (domain_key, stale_roster) in state.projection_team_rosters_snapshot() {
        if current_keys.contains(&domain_key) {
            continue;
        }
        info!(
            team_domain_key = domain_key.as_str(),
            team_slug = stale_roster.team_slug.as_str(),
            "team no longer owns any repository; detaching team roster fiber"
        );
        write_team_event(
            state,
            &org,
            &stale_roster,
            fetched_at,
            org_membership_fetch_status,
            true,
        )
        .await?;
    }

    Ok(())
}

async fn write_team_event(
    state: &Arc<AppState>,
    org: &str,
    roster: &TeamRoster,
    fetched_at: &str,
    org_membership_fetch_status: OrgMembershipFetchStatus,
    detach: bool,
) -> Result<(), TickFailure> {
    let outcome = if detach {
        write_with_policy(|| {
            state.detach_team(org, roster, fetched_at, org_membership_fetch_status)
        })
        .await
    } else {
        write_with_policy(|| {
            state.record_team(org, roster, fetched_at, org_membership_fetch_status)
        })
        .await
    };
    outcome.map_err(|write_failure| {
        let context = WriteFailureContext {
            org: Some(org),
            team_slug: Some(roster.team_slug.as_str()),
            domain_key: None,
            writer_id: None,
        };
        log_write_failure(&write_failure, context);
        TickFailure {
            error: AppError::Persistence(write_failure.error),
            context: WriteFailureContextOwned::from(context),
        }
    })
}

/// Warn-log a team-refresh tick failure without propagating it into the
/// caller's control flow. The team-refresh cadence is decoupled from the
/// repo collect cycle (adr-fmt-ewc1i): a failed tick does not abort the
/// daemon or the next repo collection, it is retried on the next
/// scheduled team-refresh tick.
pub fn log_tick_failure(error: &AppError, context: &WriteFailureContextOwned) {
    let (expected_seq, actual_seq) = conflict_seq_fields(error);
    warn!(
        error = %error,
        org = context.org.as_deref(),
        team_slug = context.team_slug.as_deref(),
        domain_key = context.domain_key.as_deref(),
        writer_id = context.writer_id.as_deref(),
        expected_seq,
        actual_seq,
        "team-refresh tick failed; will retry on the next scheduled tick"
    );
}

fn conflict_seq_fields(error: &AppError) -> (Option<u64>, Option<u64>) {
    match error {
        AppError::Persistence(cherry_pit_storage::PersistenceError::FencedConflict {
            expected_seq,
            actual_seq,
            ..
        }) => (*expected_seq, *actual_seq),
        _ => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::AppState;
    use crate::config::runtime::{NatsStoreConfig, PardosaBackend};
    use crate::domain::metrics::{TeamMember, TeamMemberRole, TeamRosterStatus};
    use crate::github::auth::GitHubCredential;
    use crate::github::budget::BudgetGate;
    use crate::github::client::GitHubClient;
    use std::sync::Arc as StdArc;
    use std::time::Duration;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn roster(canonical_owner: &str, team_slug: &str) -> TeamRoster {
        TeamRoster {
            canonical_owner: canonical_owner.to_string(),
            team_slug: team_slug.to_string(),
            status: TeamRosterStatus::Complete,
            members: vec![TeamMember {
                login: "octocat".to_string(),
                role: TeamMemberRole::Member,
                in_org: None,
            }],
        }
    }

    #[test]
    fn current_keys_derivation_matches_team_domain_key() {
        let team_pairs = [("@acme/platform".to_string(), "platform".to_string())];
        let current_keys: BTreeSet<String> = team_pairs
            .iter()
            .filter_map(|(_, team_slug)| team_domain_key("acme", team_slug).ok())
            .collect();
        let expected = team_domain_key("acme", "platform").expect("derive key");
        assert!(current_keys.contains(&expected));
    }

    #[test]
    fn roster_fixture_has_stable_shape_for_writer_tests() {
        let r = roster("@acme/platform", "platform");
        assert_eq!(r.team_slug, "platform");
        assert_eq!(r.members.len(), 1);
    }

    fn test_client(base_url: &str) -> GitHubClient {
        let credential = GitHubCredential {
            mode: crate::domain::auth::AuthMode::Pat,
            token: secrecy::SecretString::from("test-token"),
            expires_at: None,
        };
        let budget = StdArc::new(BudgetGate::new(
            crate::config::API_BUDGET_LIMIT,
            Duration::from_secs(crate::config::API_BUDGET_WAIT_SECS),
        ));
        let rate_limit = StdArc::new(crate::github::rate_limit::new_default());
        GitHubClient::new(credential, base_url, "test-org", None, budget, rate_limit)
            .expect("test client construction should succeed")
    }

    async fn test_state() -> (std::sync::Arc<AppState>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let events_dir = dir.path().join("events");
        let nats = NatsStoreConfig::for_org("test-org", crate::config::runtime::DEFAULT_NATS_URL)
            .expect("nats config");
        let state = AppState::with_stores(&events_dir, PardosaBackend::Pgno, nats)
            .await
            .expect("with stores");
        (state, dir)
    }

    /// Mount the org-members and both team-role endpoints so a tick's
    /// full fetch sequence resolves without a real network call.
    /// `team_status` selects a `200` complete-member response or a
    /// `404` (team deleted) response for the given `team_slug`.
    async fn mount_team_and_org_endpoints(server: &MockServer, team_slug: &str, deleted: bool) {
        Mock::given(path("/orgs/test-org/members"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(server)
            .await;

        let members_path = format!("/orgs/test-org/teams/{team_slug}/members");
        let response = if deleted {
            ResponseTemplate::new(404).set_body_json(serde_json::json!({"message": "Not Found"}))
        } else {
            ResponseTemplate::new(200).set_body_json(serde_json::json!([{"login": "octocat"}]))
        };
        Mock::given(path(members_path))
            .respond_with(response)
            .mount(server)
            .await;
    }

    /// (a) A refresh tick given a roster whose GitHub-side fetch classifies
    /// `Deleted` (404), for a team slug still referenced in CODEOWNERS
    /// (i.e. still present in `team_pairs` derived from the repo
    /// projection), routes to the detach path — not a `record_team`
    /// live-write. Pins CHE-0092:R1/R2: a `Deleted` observation converges
    /// to no-op, it does not re-record a live roster every tick.
    ///
    /// Falsified against the pre-fix routing (which always calls
    /// `record_team`): seeding an existing `Complete` roster first means
    /// a `record_team` write on the next tick is anti-downgrade-guarded
    /// and leaves the stale `Complete` entry untouched (`Some`), whereas
    /// a `detach_team` write removes the fiber entirely (`None`).
    #[tokio::test]
    async fn deleted_roster_still_in_codeowners_routes_to_detach_not_record() {
        let (state, _dir) = test_state().await;
        let evidence = crate::test_fixtures::make_repository_evidence(
            "repo-a",
            crate::domain::repository::Visibility::Public,
            false,
            crate::test_fixtures::make_checks(
                crate::test_fixtures::policy_pass_setting(),
                crate::test_fixtures::secret_enabled_observable(false),
                crate::test_fixtures::dependabot_enabled(),
                crate::test_fixtures::branch_pass(),
                crate::test_fixtures::codeowners_with_owners(&["@test-org/platform"]),
            ),
        );
        let domain_key = evidence.repository.inventory_key.clone();
        let repo_name = evidence.repository.name.clone();
        state
            .record_repo(&domain_key, evidence, &repo_name, "2026-07-23T00:00:00Z")
            .expect("seed repo evidence");

        let team_key = team_domain_key("test-org", "platform").expect("derive team key");
        state
            .record_team(
                "test-org",
                &roster("@test-org/platform", "platform"),
                "2026-07-22T00:00:00Z",
                crate::event::OrgMembershipFetchStatus::Fetched,
            )
            .expect("seed existing complete roster");
        assert!(
            state.lock_projection().team_rosters.contains_key(&team_key),
            "seeded roster must be live before the tick under test"
        );

        let server = MockServer::start().await;
        mount_team_and_org_endpoints(&server, "platform", true).await;
        let client = test_client(&server.uri());

        run_team_refresh_tick(&state, &client, "2026-07-23T01:00:00Z")
            .await
            .expect("tick succeeds");

        assert!(
            !state.lock_projection().team_rosters.contains_key(&team_key),
            "a Deleted roster still in CODEOWNERS must route to detach_team, \
             removing the live fiber — not record_team, which would leave the \
             stale Complete entry anti-downgrade-guarded in place"
        );
    }

    /// (b) A second identical tick over an already-detached team is a
    /// no-op: no new live-write, no fence churn (idempotent convergence,
    /// CHE-0091:R4). The ghost roster observed after the first tick must
    /// be unchanged (same content) after the second tick.
    #[tokio::test]
    async fn second_identical_tick_is_idempotent_no_new_write() {
        let (state, _dir) = test_state().await;
        let evidence = crate::test_fixtures::make_repository_evidence(
            "repo-a",
            crate::domain::repository::Visibility::Public,
            false,
            crate::test_fixtures::make_checks(
                crate::test_fixtures::policy_pass_setting(),
                crate::test_fixtures::secret_enabled_observable(false),
                crate::test_fixtures::dependabot_enabled(),
                crate::test_fixtures::branch_pass(),
                crate::test_fixtures::codeowners_with_owners(&["@test-org/platform"]),
            ),
        );
        let domain_key = evidence.repository.inventory_key.clone();
        let repo_name = evidence.repository.name.clone();
        state
            .record_repo(&domain_key, evidence, &repo_name, "2026-07-23T00:00:00Z")
            .expect("seed repo evidence");

        let server = MockServer::start().await;
        mount_team_and_org_endpoints(&server, "platform", true).await;
        let client = test_client(&server.uri());

        run_team_refresh_tick(&state, &client, "2026-07-23T01:00:00Z")
            .await
            .expect("first tick succeeds");
        let after_first = state.projection_team_ghost_rosters_snapshot();

        run_team_refresh_tick(&state, &client, "2026-07-23T02:00:00Z")
            .await
            .expect("second identical tick succeeds");
        let after_second = state.projection_team_ghost_rosters_snapshot();

        assert_eq!(
            after_first, after_second,
            "a second identical tick over an already-detached team must be a \
             no-op convergence, not a fresh write that churns the ghost roster"
        );
    }

    #[derive(Clone, Default)]
    struct VecWriter {
        buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl VecWriter {
        fn snapshot(&self) -> String {
            String::from_utf8(self.buf.lock().expect("buffer mutex").clone()).expect("utf-8")
        }
    }

    impl std::io::Write for VecWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.buf
                .lock()
                .expect("buffer mutex")
                .extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for VecWriter {
        type Writer = VecWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn capture_tracing(f: impl FnOnce()) -> String {
        let writer = VecWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_writer(writer.clone())
            .with_ansi(false)
            .with_target(false)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        writer.snapshot()
    }

    #[test]
    fn log_tick_failure_carries_write_failure_context() {
        let context = WriteFailureContextOwned {
            org: Some("acme".to_string()),
            team_slug: Some("platform".to_string()),
            domain_key: None,
            writer_id: None,
        };
        let error = AppError::Persistence(cherry_pit_storage::PersistenceError::FencedConflict {
            expected_seq: None,
            actual_seq: None,
            source: Box::new(std::io::Error::other("wrong last sequence")),
        });
        let json = capture_tracing(|| log_tick_failure(&error, &context));
        let parsed: serde_json::Value =
            serde_json::from_str(json.lines().next().expect("one log line")).expect("valid json");
        assert_eq!(parsed["fields"]["org"].as_str(), Some("acme"));
        assert_eq!(parsed["fields"]["team_slug"].as_str(), Some("platform"));
    }

    #[test]
    fn log_tick_failure_surfaces_discrete_seq_fields_for_conflict() {
        let context = WriteFailureContextOwned::default();
        let error = AppError::Persistence(cherry_pit_storage::PersistenceError::FencedConflict {
            expected_seq: Some(7),
            actual_seq: Some(9),
            source: Box::new(std::io::Error::other("wrong last sequence")),
        });
        let json = capture_tracing(|| log_tick_failure(&error, &context));
        let parsed: serde_json::Value =
            serde_json::from_str(json.lines().next().expect("one log line")).expect("valid json");
        assert_eq!(
            parsed["fields"]["expected_seq"].as_u64(),
            Some(7),
            "expected_seq must ride as a discrete typed field: {parsed}"
        );
        assert_eq!(
            parsed["fields"]["actual_seq"].as_u64(),
            Some(9),
            "actual_seq must ride as a discrete typed field: {parsed}"
        );
    }

    #[test]
    fn log_tick_failure_emits_none_seq_fields_for_non_conflict_error() {
        let context = WriteFailureContextOwned::default();
        let error =
            AppError::Persistence(cherry_pit_storage::PersistenceError::BackendUnavailable {
                reason: "nats down".to_string(),
            });
        let json = capture_tracing(|| log_tick_failure(&error, &context));
        let parsed: serde_json::Value =
            serde_json::from_str(json.lines().next().expect("one log line")).expect("valid json");
        assert!(
            parsed["fields"]["expected_seq"].is_null(),
            "non-conflict error must not fabricate a seq value: {parsed}"
        );
        assert!(
            parsed["fields"]["actual_seq"].is_null(),
            "non-conflict error must not fabricate a seq value: {parsed}"
        );
    }
}
