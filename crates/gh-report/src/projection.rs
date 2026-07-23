//! Read-side projection for repository evidence.

use std::collections::BTreeMap;

use cherry_pit_core::{DomainEvent, EventEnvelope, Projection, ReadPort};
use serde::{Deserialize, Serialize};

use crate::domain::evidence::{AssessmentMetadata, OrgStateSnapshot, RepositoryEvidence};
use crate::domain::metrics::{OrgAlertSummary, TeamRoster, TeamRosterStatus};

/// Org-level read-model part folded from the latest org event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgReadModel {
    /// Number of archived repositories observed at org scope.
    pub archived_repos: u32,
    /// Metadata for the collection run that produced this org snapshot.
    pub assessment_metadata: AssessmentMetadata,
    /// Organization-level secret-scanning alert summary.
    pub alert_summary: OrgAlertSummary,
}

/// Pruned read-model row for a repository detected as deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletedRepoRecord {
    /// Repository name at deletion detection time.
    pub repo_name: String,
    /// ISO 8601 timestamp when deletion was detected.
    pub detected_at: String,
}

impl From<OrgStateSnapshot> for OrgReadModel {
    fn from(value: OrgStateSnapshot) -> Self {
        Self {
            archived_repos: value.archived_repos,
            assessment_metadata: value.assessment_metadata,
            alert_summary: value.alert_summary,
        }
    }
}

impl OrgReadModel {
    /// True when the org-level alert collection that produced this
    /// snapshot succeeded (own-scope completeness concept for the org
    /// fold, CHE-0092:R1/R4 — this fold's analogue of team's
    /// `TeamRosterStatus::Complete`).
    #[must_use]
    fn is_complete(&self) -> bool {
        self.alert_summary.collection_status == crate::domain::status::CollectionStatus::Success
    }
}

/// Read-side projection materialising governance evidence from
/// native pardosa events.
///
/// Replaces the v1 bespoke `EvidenceStore`. Stores per-repository evidence
/// keyed by `domain_key` (the `Repository::inventory_key` of the form
/// `"id-<repo-name>"`) plus run-level [`AssessmentMetadata`].
///
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct EvidenceProjection {
    /// Per-repository evidence keyed by `domain_key`.
    ///
    /// `BTreeMap` for deterministic iteration order — required by
    /// snapshot serialisation (BC-v2-6 idempotency: byte-identical
    /// snapshots for byte-identical event streams) and HTML render
    /// stability (B8').
    pub repositories: BTreeMap<String, RepositoryEvidence>,

    /// Repositories absent from a successful inventory sweep.
    pub deleted: BTreeMap<String, DeletedRepoRecord>,

    /// Last-known org-level state folded from the org event stream.
    pub org_state: Option<OrgReadModel>,

    /// Latest-per-fiber team rosters folded from the team event stream
    /// (CHE-0089:R4), keyed by `team_domain_key`. `BTreeMap` for the same
    /// deterministic-iteration rationale as [`Self::repositories`].
    pub team_rosters: BTreeMap<String, TeamRoster>,

    /// Detached-but-still-`Deleted` team rosters, keyed by
    /// `team_domain_key` (CHE-0093:R2/R4). A detach event whose roster
    /// carries [`TeamRosterStatus::Deleted`] removes the fiber from
    /// [`Self::team_rosters`] (the live view) but retains it here so the
    /// ghost-teams render surface (`build_deleted_view_model`) survives
    /// detach — derived from the same event stream, never a new
    /// persisted schema fact. A later non-`Deleted` record for the same
    /// key clears the ghost entry (the team is live again).
    pub team_ghost_rosters: BTreeMap<String, TeamRoster>,
}

/// Projection-input event consumed by the core [`cherry_pit_core::Projection`] impl.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvidenceProjectionEvent {
    /// Repository state observed from the gh-report native repository store.
    RepositoryStateCaptured {
        /// Pardosa detached-envelope flag for repository soft-delete replay.
        detached: bool,
        /// Repository projection key.
        domain_key: String,
        /// Repository evidence when the fiber is live.
        evidence: Option<Box<RepositoryEvidence>>,
    },
    /// Repository deletion observed during successful inventory reconciliation.
    RepositoryDeleted {
        /// Repository projection key.
        domain_key: String,
        /// Repository name at deletion detection time.
        repo_name: String,
        /// ISO 8601 deletion detection timestamp.
        detected_at: String,
    },
    /// Org-level read model observed from the gh-report native org store.
    OrgStateCaptured(Box<OrgStateSnapshot>),
    /// Team roster state observed from the gh-report native team store
    /// (CHE-0089:R4). `detached` mirrors the repository-side non-detached-
    /// upsert / detached-remove pattern (CHE-0073:R7).
    TeamStateCaptured {
        /// Pardosa detached-envelope flag for team soft-delete replay.
        detached: bool,
        /// Team projection key (`team_domain_key`).
        domain_key: String,
        /// The roster payload, carried on both a live upsert and a
        /// detach — the detach tombstone's status feeds the ghost-teams
        /// retention path (see [`EvidenceProjection::team_ghost_rosters`]).
        roster: Option<Box<TeamRoster>>,
    },
}

impl DomainEvent for EvidenceProjectionEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::RepositoryStateCaptured { .. } => {
                "gh-report.projection.repository_state_captured"
            }
            Self::RepositoryDeleted { .. } => "gh-report.projection.repository_deleted",
            Self::OrgStateCaptured(_) => "gh-report.projection.org_state_captured",
            Self::TeamStateCaptured { .. } => "gh-report.projection.team_state_captured",
        }
    }
}

impl Projection for EvidenceProjection {
    type Event = EvidenceProjectionEvent;

    fn apply(&mut self, envelope: &EventEnvelope<Self::Event>) {
        match envelope.payload() {
            EvidenceProjectionEvent::RepositoryStateCaptured {
                detached,
                domain_key,
                evidence,
            } => {
                if *detached {
                    self.repositories.remove(domain_key);
                } else if let Some(evidence) = evidence.as_ref() {
                    let existing_is_complete = self
                        .repositories
                        .get(domain_key)
                        .is_some_and(RepositoryEvidence::is_complete);
                    let incoming_is_complete = evidence.is_complete();
                    if !existing_is_complete || incoming_is_complete {
                        self.deleted.remove(domain_key);
                        self.repositories
                            .insert(domain_key.clone(), evidence.as_ref().clone());
                    }
                }
            }
            EvidenceProjectionEvent::RepositoryDeleted {
                domain_key,
                repo_name,
                detected_at,
            } => {
                self.repositories.remove(domain_key);
                self.deleted.insert(
                    domain_key.clone(),
                    DeletedRepoRecord {
                        repo_name: repo_name.clone(),
                        detected_at: detected_at.clone(),
                    },
                );
            }
            EvidenceProjectionEvent::OrgStateCaptured(snapshot) => {
                self.apply_org_state(snapshot.as_ref().clone());
            }
            EvidenceProjectionEvent::TeamStateCaptured {
                detached,
                domain_key,
                roster,
            } => {
                if *detached {
                    self.team_rosters.remove(domain_key);
                    match roster.as_ref() {
                        Some(roster) if roster.status == TeamRosterStatus::Deleted => {
                            self.team_ghost_rosters
                                .insert(domain_key.clone(), roster.as_ref().clone());
                        }
                        _ => {
                            self.team_ghost_rosters.remove(domain_key);
                        }
                    }
                } else if let Some(roster) = roster.as_ref() {
                    let existing_is_complete = self
                        .team_rosters
                        .get(domain_key)
                        .is_some_and(|existing| existing.status == TeamRosterStatus::Complete);
                    let incoming_is_complete = roster.status == TeamRosterStatus::Complete;
                    if !existing_is_complete || incoming_is_complete {
                        self.team_rosters
                            .insert(domain_key.clone(), roster.as_ref().clone());
                    }
                    self.team_ghost_rosters.remove(domain_key);
                }
            }
        }
    }
}

/// Typed read query for the governance evidence projection.
#[derive(Debug, Clone)]
pub enum EvidenceProjectionQuery {
    /// Return repository evidence for one domain key.
    ByKey(String),
    /// Return the number of materialised repositories.
    Len,
    /// Return whether one domain key is materialised.
    Contains(String),
    /// Return all repository evidence in render-stable order.
    SortedSnapshot,
    /// Return deleted repository rows in key order.
    DeletedSnapshot,
    /// Return `(inventory_key, name)` pairs for all materialised
    /// repositories, without cloning full repository evidence.
    KeyNameSnapshot,
    /// Return the latest org read-model part.
    OrgState,
    /// Return the team roster for one `team_domain_key`.
    TeamRoster(String),
    /// Return all team rosters in `team_domain_key` order.
    TeamRostersSnapshot,
    /// Return all detached-but-`Deleted` ghost team rosters in
    /// `team_domain_key` order (CHE-0093:R2/R4).
    TeamGhostRostersSnapshot,
}

/// Typed read response for the governance evidence projection.
#[derive(Debug, Clone)]
pub enum EvidenceProjectionResponse {
    /// Optional repository evidence result.
    One(Box<Option<RepositoryEvidence>>),
    /// Repository count result.
    Len(usize),
    /// Boolean membership result.
    Contains(bool),
    /// Ordered repository evidence result.
    Many(Vec<RepositoryEvidence>),
    /// Ordered deleted repository rows.
    Deleted(Vec<(String, DeletedRepoRecord)>),
    /// Ordered `(inventory_key, name)` pairs.
    KeyNamePairs(Vec<(String, String)>),
    /// Optional org read-model result.
    OrgState(Box<Option<OrgReadModel>>),
    /// Optional team roster result.
    TeamRoster(Box<Option<TeamRoster>>),
    /// Ordered `(team_domain_key, TeamRoster)` pairs.
    TeamRostersSnapshot(Vec<(String, TeamRoster)>),
    /// Ordered `(team_domain_key, TeamRoster)` ghost-roster pairs.
    TeamGhostRostersSnapshot(Vec<(String, TeamRoster)>),
}

/// Static read port for [`EvidenceProjection`].
pub struct EvidenceProjectionReadPort;

impl EvidenceProjection {
    /// Look up evidence for a single repository by `domain_key`.
    ///
    /// Returns an owned clone so read sites stay a literal call-site
    /// rename across the M2.c cutover.
    ///
    /// Per CHE-0048:R2 this projection is the sole reader/writer pair
    /// of its read-model; direct access via this method is the
    /// authorised read path at v0.1 (CHE-0054:R10 — no
    /// `CommandGateway`).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<RepositoryEvidence> {
        self.repositories.get(key).cloned()
    }

    /// Number of repositories currently materialised in the projection.
    ///
    /// Per CHE-0048:R2 the projection owns this count as the sole
    /// reader of its read-model.
    #[must_use]
    pub fn len(&self) -> usize {
        self.repositories.len()
    }

    /// True when no repositories are materialised. Pairs with [`Self::len`].
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.repositories.is_empty()
            && self.deleted.is_empty()
            && self.org_state.is_none()
            && self.team_rosters.is_empty()
            && self.team_ghost_rosters.is_empty()
    }

    /// Apply an org snapshot as latest-event-read state.
    ///
    /// Anti-downgrade guarded (CHE-0092:R1): a degraded/transient
    /// observation does not overwrite an existing complete org
    /// snapshot; a fresh complete observation always overwrites.
    pub fn apply_org_state(&mut self, snapshot: OrgStateSnapshot) {
        let incoming = OrgReadModel::from(snapshot);
        let existing_is_complete = self
            .org_state
            .as_ref()
            .is_some_and(OrgReadModel::is_complete);
        let incoming_is_complete = incoming.is_complete();
        if !existing_is_complete || incoming_is_complete {
            self.org_state = Some(incoming);
        }
    }

    /// Look up the team roster for a single `team_domain_key`.
    ///
    /// Returns an owned clone, mirroring [`Self::get`]. Per CHE-0048:R2
    /// this projection is the sole reader/writer pair of its read-model.
    #[must_use]
    pub fn team_roster(&self, team_domain_key: &str) -> Option<TeamRoster> {
        self.team_rosters.get(team_domain_key).cloned()
    }

    /// Snapshot of all team rosters in `team_domain_key` order.
    ///
    /// Underlying storage is already a `BTreeMap<String, _>`, so
    /// iteration order is deterministic without an explicit re-sort
    /// (unlike [`Self::sorted_snapshot`], which re-sorts by
    /// `(repository.id, repository.name)`).
    #[must_use]
    pub fn team_rosters_snapshot(&self) -> Vec<(String, TeamRoster)> {
        self.team_rosters
            .iter()
            .map(|(key, roster)| (key.clone(), roster.clone()))
            .collect()
    }

    /// Snapshot of all ghost (detached-but-`Deleted`) team rosters in
    /// `team_domain_key` order (CHE-0093:R2/R4).
    #[must_use]
    pub fn team_ghost_rosters_snapshot(&self) -> Vec<(String, TeamRoster)> {
        self.team_ghost_rosters
            .iter()
            .map(|(key, roster)| (key.clone(), roster.clone()))
            .collect()
    }

    /// Snapshot of all repositories, sorted by `(repository.id,
    /// repository.name)`.
    ///
    /// Per CHE-0048:R2 the projection owns the sort discipline of its
    /// read-model; ordering is `(repository.id, repository.name)` —
    /// required for snapshot byte-identity (BC-v2-6) and HTML render
    /// stability. Underlying storage is already a `BTreeMap<String, _>`
    /// ordered by `domain_key`, but we re-sort by `(id, name)`
    /// explicitly: callers may not rely on `domain_key == id-name`
    /// always agreeing with `(id, name)` lexicographic order.
    ///
    /// Cost: O(n log n) per call; clones the underlying entries.
    #[must_use]
    pub fn sorted_snapshot(&self) -> Vec<RepositoryEvidence> {
        let mut entries: Vec<RepositoryEvidence> = self.repositories.values().cloned().collect();
        entries.sort_by(|a, b| {
            a.repository
                .id
                .cmp(&b.repository.id)
                .then_with(|| a.repository.name.cmp(&b.repository.name))
        });
        entries
    }

    /// Snapshot of deleted repository rows in deterministic key order.
    #[must_use]
    pub fn deleted_snapshot(&self) -> Vec<(String, DeletedRepoRecord)> {
        self.deleted
            .iter()
            .map(|(key, record)| (key.clone(), record.clone()))
            .collect()
    }

    /// Snapshot of `(inventory_key, name)` pairs for all materialised
    /// repositories.
    ///
    /// Clones only the two `String` fields per entry rather than the
    /// full `RepositoryEvidence` — for read sites that need repository
    /// identity but not the rest of the evidence (e.g. reconcile's
    /// disappeared-repo detection).
    #[must_use]
    pub fn key_name_snapshot(&self) -> Vec<(String, String)> {
        self.repositories
            .values()
            .map(|evidence| {
                (
                    evidence.repository.inventory_key.clone(),
                    evidence.repository.name.clone(),
                )
            })
            .collect()
    }

    /// Bulk-load baseline evidence.
    ///
    /// Merges into existing entries; entries with the same
    /// `inventory_key` overwrite the earlier value (last-writer-wins).
    /// May be called sequentially with [`Self::load_resumed_checkpoint`]
    /// (saga warm-load is W4-then-W3 per `app/collect.rs:537,543`);
    /// the second call adds to rather than replaces the first call's
    /// entries.
    ///
    /// Keyed by `repository.inventory_key`.
    pub fn load_baseline(&mut self, entries: Vec<RepositoryEvidence>) {
        self.bulk_load(entries);
    }

    /// Bulk-load resumed-checkpoint evidence at startup.
    ///
    /// Same authorisation and merge-semantics as
    /// [`Self::load_baseline`]; separate method to keep call-site
    /// intent visible (W4 vs W3 per `app/collect.rs:1044-1048` vs
    /// `app/collect.rs:618-621`). Bodies are intentionally identical
    /// at v0.1 — the distinction is documentary, mirroring the M2
    /// parent brief D2.
    ///
    /// May be called sequentially with [`Self::load_baseline`]
    /// (saga warm-load calls this first, then `load_baseline`);
    /// the second call adds to rather than replaces this call's
    /// entries.
    pub fn load_resumed_checkpoint(&mut self, entries: Vec<RepositoryEvidence>) {
        self.bulk_load(entries);
    }

    /// Private helper: merge-by-key bulk load.
    ///
    /// Extends the existing map; on `inventory_key` collision the
    /// incoming entry overwrites the existing one (last-writer-wins,
    /// per `BTreeMap::extend` semantics). Used by both
    /// [`Self::load_baseline`] and [`Self::load_resumed_checkpoint`]
    /// so the saga can call them sequentially without one evicting
    /// the other's entries.
    fn bulk_load(&mut self, entries: Vec<RepositoryEvidence>) {
        self.repositories.extend(
            entries
                .into_iter()
                .map(|ev| (ev.repository.inventory_key.clone(), ev)),
        );
    }
}

impl ReadPort for EvidenceProjectionReadPort {
    type Projection = EvidenceProjection;
    type Query = EvidenceProjectionQuery;
    type Response = EvidenceProjectionResponse;

    fn resolve(projection: &Self::Projection, query: Self::Query) -> Self::Response {
        match query {
            EvidenceProjectionQuery::ByKey(key) => {
                EvidenceProjectionResponse::One(Box::new(projection.get(&key)))
            }
            EvidenceProjectionQuery::Len => EvidenceProjectionResponse::Len(projection.len()),
            EvidenceProjectionQuery::Contains(key) => {
                EvidenceProjectionResponse::Contains(projection.repositories.contains_key(&key))
            }
            EvidenceProjectionQuery::SortedSnapshot => {
                EvidenceProjectionResponse::Many(projection.sorted_snapshot())
            }
            EvidenceProjectionQuery::DeletedSnapshot => {
                EvidenceProjectionResponse::Deleted(projection.deleted_snapshot())
            }
            EvidenceProjectionQuery::KeyNameSnapshot => {
                EvidenceProjectionResponse::KeyNamePairs(projection.key_name_snapshot())
            }
            EvidenceProjectionQuery::OrgState => {
                EvidenceProjectionResponse::OrgState(Box::new(projection.org_state.clone()))
            }
            EvidenceProjectionQuery::TeamRoster(key) => {
                EvidenceProjectionResponse::TeamRoster(Box::new(projection.team_roster(&key)))
            }
            EvidenceProjectionQuery::TeamRostersSnapshot => {
                EvidenceProjectionResponse::TeamRostersSnapshot(projection.team_rosters_snapshot())
            }
            EvidenceProjectionQuery::TeamGhostRostersSnapshot => {
                EvidenceProjectionResponse::TeamGhostRostersSnapshot(
                    projection.team_ghost_rosters_snapshot(),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_projection_is_empty() {
        let p = EvidenceProjection::default();
        assert!(p.repositories.is_empty());
        assert!(p.deleted.is_empty());
        assert!(p.org_state.is_none());
        assert!(p.team_rosters.is_empty());
    }

    #[test]
    fn is_empty_reflects_deleted_entries() {
        let mut p = EvidenceProjection::default();
        assert!(p.is_empty());
        p.deleted.insert(
            "id-deleted".to_string(),
            DeletedRepoRecord {
                repo_name: "deleted".to_string(),
                detected_at: "2026-06-24T00:00:00Z".to_string(),
            },
        );
        assert!(!p.is_empty());
    }

    fn ev(domain_key: &str, name: &str) -> RepositoryEvidence {
        use crate::test_fixtures;
        let mut evidence = test_fixtures::all_passing_evidence(name);
        evidence.repository.inventory_key = domain_key.to_string();
        evidence
    }

    fn team_roster(canonical_owner: &str, team_slug: &str) -> TeamRoster {
        use crate::domain::metrics::{TeamMember, TeamMemberRole, TeamRosterStatus};
        TeamRoster {
            canonical_owner: canonical_owner.to_string(),
            team_slug: team_slug.to_string(),
            status: TeamRosterStatus::Complete,
            members: vec![TeamMember {
                login: "octocat".to_string(),
                role: TeamMemberRole::Member,
                in_org: Some(true),
            }],
        }
    }

    fn apply_team_event(
        projection: &mut EvidenceProjection,
        detached: bool,
        domain_key: &str,
        roster: Option<TeamRoster>,
    ) {
        use cherry_pit_core::AggregateId;
        use std::num::NonZeroU64;
        let event = EvidenceProjectionEvent::TeamStateCaptured {
            detached,
            domain_key: domain_key.to_string(),
            roster: roster.map(Box::new),
        };
        let envelope = EventEnvelope::new(
            uuid::Uuid::now_v7(),
            AggregateId::new(NonZeroU64::MIN),
            NonZeroU64::MIN,
            jiff::Timestamp::now(),
            None,
            None,
            event,
        )
        .expect("test envelope invariant holds");
        projection.apply(&envelope);
    }

    #[test]
    fn team_state_captured_upserts_team_rosters() {
        let mut p = EvidenceProjection::default();
        assert!(p.team_roster("team_abc").is_none());
        let roster = team_roster("@acme/platform", "platform");
        apply_team_event(&mut p, false, "team_abc", Some(roster.clone()));
        assert_eq!(p.team_roster("team_abc"), Some(roster));
    }

    #[test]
    fn team_state_captured_detached_removes_team_roster() {
        let mut p = EvidenceProjection::default();
        let roster = team_roster("@acme/platform", "platform");
        apply_team_event(&mut p, false, "team_abc", Some(roster));
        assert!(p.team_roster("team_abc").is_some());
        apply_team_event(&mut p, true, "team_abc", None);
        assert!(p.team_roster("team_abc").is_none());
    }

    fn degraded_team_roster(
        canonical_owner: &str,
        team_slug: &str,
        status: crate::domain::metrics::TeamRosterStatus,
    ) -> TeamRoster {
        use crate::domain::metrics::{TeamMember, TeamMemberRole};
        TeamRoster {
            canonical_owner: canonical_owner.to_string(),
            team_slug: team_slug.to_string(),
            status,
            members: vec![TeamMember {
                login: "octocat".to_string(),
                role: TeamMemberRole::Member,
                in_org: Some(true),
            }],
        }
    }

    #[test]
    fn team_state_captured_transient_does_not_downgrade_complete_roster() {
        use crate::domain::metrics::TeamRosterStatus;
        let mut p = EvidenceProjection::default();
        let complete = team_roster("@acme/platform", "platform");
        apply_team_event(&mut p, false, "team_abc", Some(complete.clone()));
        assert_eq!(p.team_roster("team_abc"), Some(complete.clone()));

        let transient = degraded_team_roster(
            "@acme/platform",
            "platform",
            TeamRosterStatus::TransientError,
        );
        apply_team_event(&mut p, false, "team_abc", Some(transient));
        assert_eq!(
            p.team_roster("team_abc"),
            Some(complete.clone()),
            "a transient-error roster must not overwrite an existing Complete roster"
        );

        let denied = degraded_team_roster(
            "@acme/platform",
            "platform",
            TeamRosterStatus::PermissionDenied,
        );
        apply_team_event(&mut p, false, "team_abc", Some(denied));
        assert_eq!(
            p.team_roster("team_abc"),
            Some(complete),
            "a permission-denied roster must not overwrite an existing Complete roster"
        );
    }

    #[test]
    fn team_state_captured_deleted_does_not_downgrade_complete_roster() {
        use crate::domain::metrics::TeamRosterStatus;
        let mut p = EvidenceProjection::default();
        let complete = team_roster("@acme/platform", "platform");
        apply_team_event(&mut p, false, "team_abc", Some(complete.clone()));
        assert_eq!(p.team_roster("team_abc"), Some(complete.clone()));

        let deleted_status =
            degraded_team_roster("@acme/platform", "platform", TeamRosterStatus::Deleted);
        apply_team_event(&mut p, false, "team_abc", Some(deleted_status));
        assert_eq!(
            p.team_roster("team_abc"),
            Some(complete),
            "a non-detached Deleted-status roster (synthesized from a bare 404) must not \
             overwrite an existing Complete roster; only the envelope `detached` flag removes"
        );
    }

    #[test]
    fn team_state_captured_detached_still_removes_after_complete() {
        let mut p = EvidenceProjection::default();
        let complete = team_roster("@acme/platform", "platform");
        apply_team_event(&mut p, false, "team_abc", Some(complete));
        assert!(p.team_roster("team_abc").is_some());

        apply_team_event(&mut p, true, "team_abc", None);
        assert!(
            p.team_roster("team_abc").is_none(),
            "detached must remain the sole removal signal (CHE-0089:R4) even after a Complete roster"
        );
    }

    #[test]
    fn team_state_captured_fresh_complete_overwrites_existing_complete() {
        use crate::domain::metrics::{TeamMember, TeamMemberRole};
        let mut p = EvidenceProjection::default();
        let first = team_roster("@acme/platform", "platform");
        apply_team_event(&mut p, false, "team_abc", Some(first));

        let mut second = team_roster("@acme/platform", "platform");
        second.members = vec![TeamMember {
            login: "hubot".to_string(),
            role: TeamMemberRole::Maintainer,
            in_org: Some(true),
        }];
        apply_team_event(&mut p, false, "team_abc", Some(second.clone()));
        assert_eq!(
            p.team_roster("team_abc"),
            Some(second),
            "a fresh Complete roster must still overwrite the prior Complete roster"
        );
    }

    #[test]
    fn team_state_captured_apply_is_idempotent_under_replay() {
        let mut p = EvidenceProjection::default();
        let roster = team_roster("@acme/platform", "platform");
        apply_team_event(&mut p, false, "team_abc", Some(roster.clone()));
        let first = p.team_rosters_snapshot();
        apply_team_event(&mut p, false, "team_abc", Some(roster));
        let second = p.team_rosters_snapshot();
        assert_eq!(first, second);
    }

    #[test]
    fn team_rosters_snapshot_orders_by_domain_key() {
        let mut p = EvidenceProjection::default();
        apply_team_event(&mut p, false, "team_b", Some(team_roster("@acme/b", "b")));
        apply_team_event(&mut p, false, "team_a", Some(team_roster("@acme/a", "a")));
        let snapshot = p.team_rosters_snapshot();
        let keys: Vec<&str> = snapshot.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, vec!["team_a", "team_b"]);
    }

    #[test]
    fn get_returns_some_after_apply_and_none_otherwise() {
        let mut p = EvidenceProjection::default();
        assert!(p.get("id-repo-1").is_none());
        p.load_baseline(vec![ev("id-repo-1", "repo-1")]);
        let got = p.get("id-repo-1").expect("present after apply");
        assert_eq!(got.repository.name, "repo-1");
        assert!(p.get("id-missing").is_none());
    }

    #[test]
    fn len_matches_inserted_count() {
        let mut p = EvidenceProjection::default();
        assert_eq!(p.len(), 0);
        p.load_baseline(vec![ev("id-a", "a"), ev("id-b", "b")]);
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn sorted_snapshot_orders_by_id_then_name() {
        let mut p = EvidenceProjection::default();
        p.load_baseline(vec![ev("id-b", "b"), ev("id-a", "a"), ev("id-c", "c")]);
        let snap = p.sorted_snapshot();
        let ids: Vec<&str> = snap.iter().map(|e| e.repository.id.as_str()).collect();
        assert_eq!(ids, vec!["id-a", "id-b", "id-c"]);
    }

    #[test]
    fn sorted_snapshot_of_empty_projection_is_empty() {
        let p = EvidenceProjection::default();
        assert!(p.sorted_snapshot().is_empty());
    }

    #[test]
    fn load_baseline_merges_into_existing_entries() {
        use crate::test_fixtures;
        let mut p = EvidenceProjection::default();
        p.load_baseline(vec![ev("id-prior", "prior")]);
        let entries = vec![
            test_fixtures::all_passing_evidence("a"),
            test_fixtures::all_passing_evidence("b"),
        ];
        p.load_baseline(entries);
        assert_eq!(p.len(), 3);
        assert!(p.get("id-prior").is_some());
        assert!(p.get("id-a").is_some());
        assert!(p.get("id-b").is_some());
    }

    #[test]
    fn load_baseline_overwrites_same_key_last_writer_wins() {
        use crate::test_fixtures;
        let mut p = EvidenceProjection::default();
        p.load_baseline(vec![test_fixtures::all_passing_evidence("a")]);
        let updated = test_fixtures::all_passing_evidence("a");
        p.load_baseline(vec![updated.clone()]);
        assert_eq!(p.len(), 1);
        assert_eq!(p.get("id-a").as_ref(), Some(&updated));
    }

    #[test]
    fn load_baseline_is_idempotent() {
        use crate::test_fixtures;
        let mut p = EvidenceProjection::default();
        let entries = vec![
            test_fixtures::all_passing_evidence("a"),
            test_fixtures::all_passing_evidence("b"),
        ];
        p.load_baseline(entries.clone());
        let first = p.sorted_snapshot();
        p.load_baseline(entries);
        let second = p.sorted_snapshot();
        assert_eq!(first, second);
    }

    #[test]
    fn load_resumed_checkpoint_matches_load_baseline_semantics() {
        use crate::test_fixtures;
        let mut p1 = EvidenceProjection::default();
        let mut p2 = EvidenceProjection::default();
        let entries = vec![
            test_fixtures::all_passing_evidence("a"),
            test_fixtures::all_passing_evidence("b"),
        ];
        p1.load_baseline(entries.clone());
        p2.load_resumed_checkpoint(entries);
        assert_eq!(p1.sorted_snapshot(), p2.sorted_snapshot());
    }

    fn apply_repo_event(
        projection: &mut EvidenceProjection,
        detached: bool,
        domain_key: &str,
        evidence: Option<RepositoryEvidence>,
    ) {
        use cherry_pit_core::AggregateId;
        use std::num::NonZeroU64;
        let event = EvidenceProjectionEvent::RepositoryStateCaptured {
            detached,
            domain_key: domain_key.to_string(),
            evidence: evidence.map(Box::new),
        };
        let envelope = EventEnvelope::new(
            uuid::Uuid::now_v7(),
            AggregateId::new(NonZeroU64::MIN),
            NonZeroU64::MIN,
            jiff::Timestamp::now(),
            None,
            None,
            event,
        )
        .expect("test envelope invariant holds");
        projection.apply(&envelope);
    }

    fn degraded_evidence(name: &str) -> RepositoryEvidence {
        use crate::domain::checks::SecurityPolicyStatus;
        let mut evidence = test_fixtures_all_passing(name);
        evidence.checks.security_policy.status = SecurityPolicyStatus::Unknown;
        evidence
    }

    fn test_fixtures_all_passing(name: &str) -> RepositoryEvidence {
        use crate::test_fixtures;
        test_fixtures::all_passing_evidence(name)
    }

    #[test]
    fn repository_state_captured_degraded_does_not_downgrade_complete_evidence() {
        let mut p = EvidenceProjection::default();
        let complete = test_fixtures_all_passing("repo-x");
        apply_repo_event(&mut p, false, "id-repo-x", Some(complete.clone()));
        assert_eq!(p.get("id-repo-x"), Some(complete.clone()));

        let mut degraded = degraded_evidence("repo-x");
        degraded.repository.inventory_key = "id-repo-x".to_string();
        apply_repo_event(&mut p, false, "id-repo-x", Some(degraded));
        assert_eq!(
            p.get("id-repo-x"),
            Some(complete),
            "a degraded (Unknown-status) observation must not overwrite existing complete evidence"
        );
    }

    #[test]
    fn repository_state_captured_fresh_complete_overwrites_existing_complete() {
        use crate::domain::checks::SecurityPolicyStatus;
        let mut p = EvidenceProjection::default();
        let first = test_fixtures_all_passing("repo-y");
        apply_repo_event(&mut p, false, "id-repo-y", Some(first));

        let mut second = test_fixtures_all_passing("repo-y");
        second.checks.security_policy.status = SecurityPolicyStatus::Fail;
        apply_repo_event(&mut p, false, "id-repo-y", Some(second.clone()));
        assert_eq!(
            p.get("id-repo-y"),
            Some(second),
            "a fresh complete observation must still overwrite the prior complete evidence"
        );
    }

    fn org_snapshot(
        collection_status: crate::domain::status::CollectionStatus,
    ) -> OrgStateSnapshot {
        use crate::domain::metrics::OrgAlertSummary;
        use crate::test_fixtures;
        use std::collections::HashMap;
        OrgStateSnapshot {
            archived_repos: 0,
            assessment_metadata: test_fixtures::make_metadata(),
            alert_summary: OrgAlertSummary {
                collection_status,
                collection_reason: None,
                per_repo: HashMap::new(),
                open_secret_alert_age_buckets: HashMap::new(),
                total_open_secret_alerts: 0,
                oldest_open_secret_alert_created_at: None,
                newest_open_secret_alert_created_at: None,
            },
        }
    }

    #[test]
    fn org_state_captured_degraded_does_not_downgrade_complete_org_state() {
        use crate::domain::status::CollectionStatus;
        let mut p = EvidenceProjection::default();
        p.apply_org_state(org_snapshot(CollectionStatus::Success));
        let complete_reason = p
            .org_state
            .clone()
            .expect("applied")
            .alert_summary
            .collection_status;
        assert_eq!(complete_reason, CollectionStatus::Success);

        p.apply_org_state(org_snapshot(CollectionStatus::TransientError));
        assert_eq!(
            p.org_state
                .expect("still present")
                .alert_summary
                .collection_status,
            CollectionStatus::Success,
            "a transient-error org observation must not overwrite an existing complete org state"
        );
    }

    #[test]
    fn org_state_captured_fresh_complete_overwrites_existing_complete() {
        use crate::domain::status::CollectionStatus;
        let mut p = EvidenceProjection::default();
        let mut first = org_snapshot(CollectionStatus::Success);
        first.archived_repos = 3;
        p.apply_org_state(first);

        let mut second = org_snapshot(CollectionStatus::Success);
        second.archived_repos = 7;
        p.apply_org_state(second);
        assert_eq!(
            p.org_state.expect("present").archived_repos,
            7,
            "a fresh complete org observation must still overwrite the prior complete org state"
        );
    }
}
