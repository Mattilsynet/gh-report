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
use crate::domain::metrics::{TeamRoster, team_owner_slugs};
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
        write_team_event(
            state,
            &org,
            roster,
            fetched_at,
            org_membership_fetch_status,
            false,
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
    warn!(
        error = %error,
        org = context.org.as_deref(),
        team_slug = context.team_slug.as_deref(),
        domain_key = context.domain_key.as_deref(),
        writer_id = context.writer_id.as_deref(),
        "team-refresh tick failed; will retry on the next scheduled tick"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::metrics::{TeamMember, TeamMemberRole, TeamRosterStatus};

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
            source: Box::new(std::io::Error::other("wrong last sequence")),
        });
        let json = capture_tracing(|| log_tick_failure(&error, &context));
        let parsed: serde_json::Value =
            serde_json::from_str(json.lines().next().expect("one log line")).expect("valid json");
        assert_eq!(parsed["fields"]["org"].as_str(), Some("acme"));
        assert_eq!(parsed["fields"]["team_slug"].as_str(), Some("platform"));
    }
}
