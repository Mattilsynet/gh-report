#![forbid(unsafe_code)]

use cherry_pit_core::{DomainEvent as CherryDomainEvent, ScheduledDomainEvent};
use pardosa::store::HasEventSchemaSource;
use pardosa_schema::{EventString, EventVec, GenomeSafe, NonEmptyEventString, Timestamp, Validate};
use serde::{Deserialize, Serialize};

pub mod convert;

pub mod limits {
    pub const MAX_DOMAIN_KEY: usize = 128;
    pub const MAX_REPO_NAME: usize = 256;
    pub const MAX_GITHUB_ID: usize = 128;
    pub const MAX_NODE_ID: usize = 256;
    pub const MAX_BRANCH_NAME: usize = 256;
    pub const MAX_LANGUAGE: usize = 128;
    pub const MAX_DESCRIPTION: usize = 4096;
    pub const MAX_URL: usize = 2048;
    pub const MAX_TOPIC: usize = 128;
    pub const MAX_TOPICS: usize = 128;
    pub const MAX_LICENSE: usize = 128;
    pub const MAX_LOGIN: usize = 256;
    pub const MAX_PERSON_NAME: usize = 256;
    pub const MAX_PATH: usize = 4096;
    pub const MAX_REASON: usize = 4096;
    pub const MAX_CODEOWNERS_PATTERN: usize = 4096;
    pub const MAX_CODEOWNERS_OWNER: usize = 256;
    pub const MAX_CODEOWNERS_OWNERS: usize = 256;
    pub const MAX_CODEOWNERS_ENTRIES: usize = 4096;
    pub const MAX_ASSESSMENT_DATE: usize = 64;
    pub const MAX_SCHEMA_VERSION: usize = 64;
    pub const MAX_RUN_ID: usize = 256;
    pub const MAX_TOKEN_SCOPES: usize = 8192;
    pub const MAX_TIMESTAMP_TEXT: usize = 128;
    pub const MAX_SWEEP_TIMEOUT_ERROR: usize = 256;
    pub const MAX_UNAVAILABLE_CAPABILITIES: usize = 32;
    pub const MAX_ORG_ALERT_REPOS: usize = 1_000_000;
    pub const MAX_ALERT_BUCKET: usize = 128;
    pub const MAX_ALERT_BUCKETS: usize = 128;
}

use limits::{
    MAX_ALERT_BUCKET, MAX_ALERT_BUCKETS, MAX_ASSESSMENT_DATE, MAX_BRANCH_NAME,
    MAX_CODEOWNERS_ENTRIES, MAX_CODEOWNERS_OWNER, MAX_CODEOWNERS_OWNERS, MAX_CODEOWNERS_PATTERN,
    MAX_DESCRIPTION, MAX_DOMAIN_KEY, MAX_GITHUB_ID, MAX_LANGUAGE, MAX_LICENSE, MAX_LOGIN,
    MAX_NODE_ID, MAX_ORG_ALERT_REPOS, MAX_PATH, MAX_PERSON_NAME, MAX_REASON, MAX_REPO_NAME,
    MAX_RUN_ID, MAX_SCHEMA_VERSION, MAX_SWEEP_TIMEOUT_ERROR, MAX_TIMESTAMP_TEXT, MAX_TOKEN_SCOPES,
    MAX_TOPIC, MAX_TOPICS, MAX_UNAVAILABLE_CAPABILITIES, MAX_URL,
};

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
#[repr(u8)]
pub(crate) enum SweepTimeoutEvent {
    TargetOpened {
        event_id: uuid::Uuid,
    } = 0,
    TimeoutFired {
        event_id: uuid::Uuid,
        run_id: EventString<MAX_RUN_ID>,
        error: EventString<MAX_SWEEP_TIMEOUT_ERROR>,
        elapsed_ms: u64,
    } = 1,
}

impl SweepTimeoutEvent {
    pub(crate) fn try_timeout_fired(
        event_id: uuid::Uuid,
        run_id: String,
        error: &str,
        elapsed_ms: u64,
    ) -> Result<Self, pardosa_schema::DomainError> {
        Ok(Self::TimeoutFired {
            event_id,
            run_id: EventString::try_from(run_id)?,
            error: EventString::try_from(error.to_string())?,
            elapsed_ms,
        })
    }

    #[must_use]
    pub(crate) fn target_opened(event_id: uuid::Uuid) -> Self {
        Self::TargetOpened { event_id }
    }

    #[must_use]
    fn event_type(&self) -> &'static str {
        match self {
            Self::TargetOpened { .. } => "gh-report.sweep_timeout_target_opened",
            Self::TimeoutFired { .. } => "gh-report.sweep_timeout_fired",
        }
    }
}

#[derive(Serialize, Deserialize)]
enum SweepTimeoutEventWire {
    TargetOpened {
        event_id: uuid::Uuid,
    },
    TimeoutFired {
        event_id: uuid::Uuid,
        run_id: String,
        error: String,
        elapsed_ms: u64,
    },
}

impl Serialize for SweepTimeoutEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let wire = match self {
            Self::TargetOpened { event_id } => SweepTimeoutEventWire::TargetOpened {
                event_id: *event_id,
            },
            Self::TimeoutFired {
                event_id,
                run_id,
                error,
                elapsed_ms,
            } => SweepTimeoutEventWire::TimeoutFired {
                event_id: *event_id,
                run_id: run_id.as_str().to_string(),
                error: error.as_str().to_string(),
                elapsed_ms: *elapsed_ms,
            },
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SweepTimeoutEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = SweepTimeoutEventWire::deserialize(deserializer)?;
        match wire {
            SweepTimeoutEventWire::TargetOpened { event_id } => Ok(Self::TargetOpened { event_id }),
            SweepTimeoutEventWire::TimeoutFired {
                event_id,
                run_id,
                error,
                elapsed_ms,
            } => Self::try_timeout_fired(event_id, run_id, &error, elapsed_ms)
                .map_err(serde::de::Error::custom),
        }
    }
}

impl CherryDomainEvent for SweepTimeoutEvent {
    fn event_type(&self) -> &'static str {
        Self::event_type(self)
    }
}

impl ScheduledDomainEvent for SweepTimeoutEvent {
    fn scheduled_event_id(&self) -> uuid::Uuid {
        match self {
            Self::TargetOpened { event_id } | Self::TimeoutFired { event_id, .. } => *event_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct RepositoryEvidence {
    pub repository: Repository,
    pub checks: RepositoryChecks,
    pub last_commit: Option<LastCommitInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct LastCommitInfo {
    pub committer_login: Option<EventString<MAX_LOGIN>>,
    pub committer_name: Option<EventString<MAX_PERSON_NAME>>,
    pub commit_date: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct Repository {
    pub id: NonEmptyEventString<MAX_GITHUB_ID>,
    pub node_id: Option<EventString<MAX_NODE_ID>>,
    pub name: NonEmptyEventString<MAX_REPO_NAME>,
    pub visibility: Visibility,
    pub language: Option<EventString<MAX_LANGUAGE>>,
    pub default_branch: NonEmptyEventString<MAX_BRANCH_NAME>,
    pub archived: bool,
    pub inventory_key: NonEmptyEventString<MAX_DOMAIN_KEY>,
    pub updated_at: Option<Timestamp>,
    pub has_issues: bool,
    pub pushed_at: Option<Timestamp>,
    pub created_at: Option<Timestamp>,
    pub description: Option<EventString<MAX_DESCRIPTION>>,
    pub fork: bool,
    pub html_url: Option<EventString<MAX_URL>>,
    pub topics: EventVec<EventString<MAX_TOPIC>, MAX_TOPICS>,
    pub license_spdx: Option<EventString<MAX_LICENSE>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GenomeSafe)]
#[repr(u8)]
pub enum Visibility {
    Public = 0,
    Internal = 1,
    Private = 2,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct RepositoryChecks {
    pub security_policy: SecurityPolicyResult,
    pub secret_scanning: SecretScanningResult,
    pub dependabot_security_updates: DependabotResult,
    pub branch_protection: BranchProtectionResult,
    pub codeowners: CodeownersResult,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct SecurityPolicyResult {
    pub status: SecurityPolicyStatus,
    pub evidence: SecurityPolicyEvidence,
    pub path: Option<EventString<MAX_PATH>>,
    pub timestamp: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GenomeSafe)]
#[repr(u8)]
pub enum SecurityPolicyStatus {
    Pass = 0,
    Fail = 1,
    Unknown = 2,
    NotApplicable = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GenomeSafe)]
#[repr(u8)]
pub enum SecurityPolicyEvidence {
    Setting = 0,
    File = 1,
    Absent = 2,
    PermissionDenied = 3,
    TransientError = 4,
    CollectionError = 5,
    NotApplicable = 6,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct SecretScanningResult {
    pub status: SecretScanningStatus,
    pub has_open_alerts: Option<bool>,
    pub alerts_observable: bool,
    pub reason: Option<EventString<MAX_REASON>>,
    pub timestamp: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GenomeSafe)]
#[repr(u8)]
pub enum SecretScanningStatus {
    Enabled = 0,
    Disabled = 1,
    PermissionDenied = 2,
    Unknown = 3,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct DependabotResult {
    pub status: DependabotStatus,
    pub reason: Option<EventString<MAX_REASON>>,
    pub timestamp: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GenomeSafe)]
#[repr(u8)]
pub enum DependabotStatus {
    Enabled = 0,
    Paused = 1,
    Disabled = 2,
    Unknown = 3,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct BranchProtectionResult {
    pub status: BranchProtectionStatus,
    pub details: BranchProtectionDetails,
    pub timestamp: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GenomeSafe)]
#[repr(u8)]
pub enum BranchProtectionStatus {
    Pass = 0,
    Partial = 1,
    Fail = 2,
    Unknown = 3,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct BranchProtectionDetails {
    pub default_branch: NonEmptyEventString<MAX_BRANCH_NAME>,
    pub has_pr: Option<bool>,
    pub required_reviewers: Option<u32>,
    pub has_status_checks: Option<bool>,
    pub admin_equivalent: Option<bool>,
    pub has_broad_bypass: Option<bool>,
    pub reason: Option<EventString<MAX_REASON>>,
    pub reason_kind: Option<CollectionFailureReason>,
    pub http_status: Option<u16>,
    pub force_push_blocked: Option<bool>,
    pub deletion_blocked: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GenomeSafe)]
#[repr(u8)]
pub enum CollectionFailureReason {
    PermissionDenied = 0,
    PermissionSuspected = 1,
    NotFoundAbsent = 2,
    Transient = 3,
    RateLimited = 4,
    Invalid = 5,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct CodeownersResult {
    pub status: CodeownersStatus,
    pub path: Option<EventString<MAX_PATH>>,
    pub timestamp: Timestamp,
    pub parsed: Option<ParsedCodeowners>,
    pub truncation: Option<CodeownersTruncationReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GenomeSafe)]
#[repr(u8)]
pub enum CodeownersStatus {
    Conforming = 0,
    NonConforming = 1,
    Absent = 2,
    Unknown = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GenomeSafe)]
#[repr(u8)]
pub enum CodeownersTruncationReason {
    NotBase64Encoded = 0,
    OversizedBase64 = 1,
    ContentMissing = 2,
    DecodeFailed = 3,
    InvalidUtf8 = 4,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct ParsedCodeowners {
    pub entries: EventVec<CodeownersEntry, MAX_CODEOWNERS_ENTRIES>,
    pub unique_owners: EventVec<EventString<MAX_CODEOWNERS_OWNER>, MAX_CODEOWNERS_OWNERS>,
    pub skipped_lines: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct CodeownersEntry {
    pub pattern: EventString<MAX_CODEOWNERS_PATTERN>,
    pub owners: EventVec<EventString<MAX_CODEOWNERS_OWNER>, MAX_CODEOWNERS_OWNERS>,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct OrgStateCaptured {
    pub archived_repos: u32,
    pub assessment_metadata: AssessmentMetadata,
    pub alert_summary: OrgAlertSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct AssessmentMetadata {
    pub date: EventString<MAX_ASSESSMENT_DATE>,
    pub organization: EventString<MAX_LOGIN>,
    pub schema_version: EventString<MAX_SCHEMA_VERSION>,
    pub run_timestamp: EventString<MAX_TIMESTAMP_TEXT>,
    pub run_id: EventString<MAX_RUN_ID>,
    pub token_tier: TokenTier,
    pub token_scopes: EventString<MAX_TOKEN_SCOPES>,
    pub auth_mode: AuthMode,
    pub rate_limit_warnings: u32,
    pub unavailable_capabilities: EventVec<Capability, MAX_UNAVAILABLE_CAPABILITIES>,
    pub inventory_fetched_at: Option<EventString<MAX_TIMESTAMP_TEXT>>,
    pub warm_start: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GenomeSafe)]
#[repr(u8)]
pub enum TokenTier {
    Full = 0,
    Limited = 1,
    Unknown = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GenomeSafe)]
#[repr(u8)]
pub enum Capability {
    OrgSecretScanningAlerts = 0,
    PrivateBranchProtectionRead = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GenomeSafe)]
#[repr(u8)]
pub enum AuthMode {
    Pat = 0,
    GitHubApp = 1,
    GhCliFallback = 2,
    Unknown = 3,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct OrgAlertSummary {
    pub collection_status: CollectionStatus,
    pub collection_reason: Option<EventString<MAX_REASON>>,
    pub per_repo: EventVec<RepoAlertSummaryEntry, MAX_ORG_ALERT_REPOS>,
    pub open_secret_alert_age_buckets: EventVec<StringU64Entry, MAX_ALERT_BUCKETS>,
    pub total_open_secret_alerts: u64,
    pub oldest_open_secret_alert_created_at: Option<EventString<MAX_TIMESTAMP_TEXT>>,
    pub newest_open_secret_alert_created_at: Option<EventString<MAX_TIMESTAMP_TEXT>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GenomeSafe)]
#[repr(u8)]
pub enum CollectionStatus {
    Success = 0,
    NotCollected = 1,
    PermissionDenied = 2,
    TransientError = 3,
    Unavailable = 4,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct RepoAlertSummaryEntry {
    pub repository_id: EventString<MAX_GITHUB_ID>,
    pub summary: RepoAlertSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct RepoAlertSummary {
    pub open_alert_count: u64,
    pub oldest_open_alert_created_at: Option<EventString<MAX_TIMESTAMP_TEXT>>,
    pub newest_open_alert_created_at: Option<EventString<MAX_TIMESTAMP_TEXT>>,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct StringU64Entry {
    pub key: EventString<MAX_ALERT_BUCKET>,
    pub value: u64,
}

impl OrgStateCaptured {
    #[must_use]
    pub fn event_type(&self) -> &'static str {
        "OrgStateCaptured"
    }
}

impl Validate for OrgStateCaptured {
    type Error = std::convert::Infallible;

    fn validate(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl HasEventSchemaSource for OrgStateCaptured {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = Some("gh-report/OrgEvent");
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
#[expect(
    clippy::large_enum_variant,
    reason = "durable event schema keeps full repository snapshot inline; boxing would reshape persisted bytes"
)]
#[repr(u8)]
pub enum DomainEvent {
    RepositoryStateCaptured {
        domain_key: NonEmptyEventString<MAX_DOMAIN_KEY>,
        repo_name: NonEmptyEventString<MAX_REPO_NAME>,
        timestamp: Timestamp,
        evidence: Option<RepositoryEvidence>,
    } = 0,
    RepositoryDeleted {
        domain_key: NonEmptyEventString<MAX_DOMAIN_KEY>,
        repo_name: NonEmptyEventString<MAX_REPO_NAME>,
        detected_at: Timestamp,
    } = 1,
}

impl DomainEvent {
    #[must_use]
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::RepositoryStateCaptured { .. } => "RepositoryStateCaptured",
            Self::RepositoryDeleted { .. } => "RepositoryDeleted",
        }
    }
}

impl Validate for DomainEvent {
    type Error = std::convert::Infallible;

    fn validate(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl HasEventSchemaSource for DomainEvent {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = Some("gh-report/DomainEvent");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use pardosa_schema::{DomainError, from_bytes, to_vec};

    fn ts(nanos: u64) -> Timestamp {
        Timestamp::from_nanos(nanos).expect("nonzero nanos")
    }

    fn nes<const MAX: usize>(s: &str) -> NonEmptyEventString<MAX> {
        NonEmptyEventString::try_new(s).expect("fits MAX, nonempty")
    }

    fn es<const MAX: usize>(s: &str) -> EventString<MAX> {
        EventString::try_from(s.to_string()).expect("fits MAX")
    }

    fn ev<const MAX: usize>(items: Vec<EventString<MAX>>) -> EventVec<EventString<MAX>, 128> {
        EventVec::try_from(items).expect("fits MAX")
    }

    fn codeowners_entries(
        items: Vec<CodeownersEntry>,
    ) -> EventVec<CodeownersEntry, MAX_CODEOWNERS_ENTRIES> {
        EventVec::try_from(items).expect("fits MAX")
    }

    fn owner_vec(
        items: Vec<EventString<MAX_CODEOWNERS_OWNER>>,
    ) -> EventVec<EventString<MAX_CODEOWNERS_OWNER>, MAX_CODEOWNERS_OWNERS> {
        EventVec::try_from(items).expect("fits MAX")
    }

    fn repository() -> Repository {
        Repository {
            id: nes("id-repo-1"),
            node_id: Some(es("node-1")),
            name: nes("repo-1"),
            visibility: Visibility::Public,
            language: Some(es("Rust")),
            default_branch: nes("main"),
            archived: false,
            inventory_key: nes("id-repo-1"),
            updated_at: Some(ts(10)),
            has_issues: true,
            pushed_at: Some(ts(11)),
            created_at: Some(ts(12)),
            description: Some(es("repository description")),
            fork: false,
            html_url: Some(es("https://github.com/acme/repo-1")),
            topics: ev(vec![es("security"), es("rust")]),
            license_spdx: Some(es("MIT")),
        }
    }

    fn parsed_codeowners() -> ParsedCodeowners {
        ParsedCodeowners {
            entries: codeowners_entries(vec![CodeownersEntry {
                pattern: es("/src/"),
                owners: owner_vec(vec![es("@acme/security")]),
            }]),
            unique_owners: owner_vec(vec![es("@acme/security")]),
            skipped_lines: 0,
        }
    }

    fn checks() -> RepositoryChecks {
        RepositoryChecks {
            security_policy: SecurityPolicyResult {
                status: SecurityPolicyStatus::Pass,
                evidence: SecurityPolicyEvidence::Setting,
                path: Some(es("SECURITY.md")),
                timestamp: ts(20),
            },
            secret_scanning: SecretScanningResult {
                status: SecretScanningStatus::Enabled,
                has_open_alerts: Some(false),
                alerts_observable: true,
                reason: Some(es("enabled")),
                timestamp: ts(21),
            },
            dependabot_security_updates: DependabotResult {
                status: DependabotStatus::Enabled,
                reason: Some(es("enabled")),
                timestamp: ts(22),
            },
            branch_protection: BranchProtectionResult {
                status: BranchProtectionStatus::Pass,
                details: BranchProtectionDetails {
                    default_branch: nes("main"),
                    has_pr: Some(true),
                    required_reviewers: Some(2),
                    has_status_checks: Some(true),
                    admin_equivalent: Some(true),
                    has_broad_bypass: Some(false),
                    reason: None,
                    reason_kind: None,
                    http_status: None,
                    force_push_blocked: Some(true),
                    deletion_blocked: Some(true),
                },
                timestamp: ts(23),
            },
            codeowners: CodeownersResult {
                status: CodeownersStatus::Conforming,
                path: Some(es(".github/CODEOWNERS")),
                timestamp: ts(24),
                parsed: Some(parsed_codeowners()),
                truncation: Some(CodeownersTruncationReason::OversizedBase64),
            },
        }
    }

    fn full_evidence() -> RepositoryEvidence {
        RepositoryEvidence {
            repository: repository(),
            checks: checks(),
            last_commit: Some(LastCommitInfo {
                committer_login: Some(es("octocat")),
                committer_name: Some(es("Mona Octocat")),
                commit_date: Some(ts(30)),
            }),
        }
    }

    fn domain_org_snapshot() -> crate::domain::evidence::OrgStateSnapshot {
        let mut per_repo = HashMap::new();
        per_repo.insert(
            "repo-1".to_string(),
            crate::domain::metrics::RepoAlertSummary {
                open_alert_count: 7,
                oldest_open_alert_created_at: Some("2026-06-13T08:00:00Z".to_string()),
                newest_open_alert_created_at: Some("2026-06-14T08:00:00Z".to_string()),
            },
        );
        let mut open_secret_alert_age_buckets = HashMap::new();
        open_secret_alert_age_buckets.insert("0_7_days".to_string(), 3);
        open_secret_alert_age_buckets.insert("8_30_days".to_string(), 4);

        crate::domain::evidence::OrgStateSnapshot {
            archived_repos: 2,
            assessment_metadata: crate::domain::evidence::AssessmentMetadata {
                date: "2026-06-14".to_string(),
                organization: "acme".to_string(),
                schema_version: "1.0".to_string(),
                run_timestamp: "2026-06-14T12:00:00Z".to_string(),
                run_id: "run-123".to_string(),
                token_tier: crate::domain::auth::TokenTier::Full,
                token_scopes: "repo,read:org,security_events".to_string(),
                auth_mode: crate::domain::auth::AuthMode::Pat,
                rate_limit_warnings: 1,
                unavailable_capabilities: vec![
                    crate::domain::auth::Capability::OrgSecretScanningAlerts,
                ],
                inventory_fetched_at: Some("2026-06-14T12:01:00Z".to_string()),
                warm_start: true,
            },
            alert_summary: crate::domain::metrics::OrgAlertSummary {
                collection_status: crate::domain::status::CollectionStatus::Success,
                collection_reason: Some("collected".to_string()),
                per_repo,
                open_secret_alert_age_buckets,
                total_open_secret_alerts: 7,
                oldest_open_secret_alert_created_at: Some("2026-06-13T08:00:00Z".to_string()),
                newest_open_secret_alert_created_at: Some("2026-06-14T08:00:00Z".to_string()),
            },
        }
    }

    fn assert_org_snapshot_eq(
        actual: &crate::domain::evidence::OrgStateSnapshot,
        expected: &crate::domain::evidence::OrgStateSnapshot,
    ) {
        assert_eq!(actual.archived_repos, expected.archived_repos);
        assert_eq!(actual.assessment_metadata, expected.assessment_metadata);
        assert_eq!(
            actual.alert_summary.collection_status,
            expected.alert_summary.collection_status
        );
        assert_eq!(
            actual.alert_summary.collection_reason,
            expected.alert_summary.collection_reason
        );
        assert_eq!(
            actual.alert_summary.per_repo.len(),
            expected.alert_summary.per_repo.len()
        );
        for (repo, expected_summary) in &expected.alert_summary.per_repo {
            let actual_summary = actual
                .alert_summary
                .per_repo
                .get(repo)
                .expect("repo alert summary round-trips");
            assert_eq!(
                actual_summary.open_alert_count,
                expected_summary.open_alert_count
            );
            assert_eq!(
                actual_summary.oldest_open_alert_created_at,
                expected_summary.oldest_open_alert_created_at
            );
            assert_eq!(
                actual_summary.newest_open_alert_created_at,
                expected_summary.newest_open_alert_created_at
            );
        }
        assert_eq!(
            actual.alert_summary.open_secret_alert_age_buckets,
            expected.alert_summary.open_secret_alert_age_buckets
        );
        assert_eq!(
            actual.alert_summary.total_open_secret_alerts,
            expected.alert_summary.total_open_secret_alerts
        );
        assert_eq!(
            actual.alert_summary.oldest_open_secret_alert_created_at,
            expected.alert_summary.oldest_open_secret_alert_created_at
        );
        assert_eq!(
            actual.alert_summary.newest_open_secret_alert_created_at,
            expected.alert_summary.newest_open_secret_alert_created_at
        );
    }

    #[test]
    fn native_repository_state_round_trips() {
        let event = DomainEvent::RepositoryStateCaptured {
            domain_key: nes("id-repo-1"),
            repo_name: nes("repo-1"),
            timestamp: ts(40),
            evidence: Some(full_evidence()),
        };
        let wire = to_vec(&event);
        let decoded: DomainEvent = from_bytes(&wire).expect("decode native event");
        assert_eq!(decoded, event);
        assert_eq!(decoded.event_type(), "RepositoryStateCaptured");
    }

    #[test]
    fn native_repository_deleted_round_trips() {
        let event = DomainEvent::RepositoryDeleted {
            domain_key: nes("id-repo-1"),
            repo_name: nes("repo-1"),
            detected_at: ts(50),
        };
        let wire = to_vec(&event);
        let decoded: DomainEvent = from_bytes(&wire).expect("decode native event");
        assert_eq!(decoded, event);
        assert_eq!(decoded.event_type(), "RepositoryDeleted");
    }

    #[test]
    fn org_state_captured_round_trips_domain_snapshot() {
        let domain = domain_org_snapshot();
        let event = OrgStateCaptured::try_from(domain.clone()).expect("org snapshot fits event");
        let decoded_domain: crate::domain::evidence::OrgStateSnapshot = event.clone().into();

        assert_org_snapshot_eq(&decoded_domain, &domain);
        assert_eq!(event.event_type(), "OrgStateCaptured");
        assert_eq!(
            <OrgStateCaptured as HasEventSchemaSource>::EVENT_SCHEMA_SOURCE,
            Some("gh-report/OrgEvent")
        );
        assert_ne!(
            <OrgStateCaptured as GenomeSafe>::SCHEMA_HASH,
            <DomainEvent as GenomeSafe>::SCHEMA_HASH
        );
    }

    #[test]
    fn native_org_state_round_trips() {
        let event =
            OrgStateCaptured::try_from(domain_org_snapshot()).expect("org snapshot fits event");
        let wire = to_vec(&event);
        let decoded: OrgStateCaptured = from_bytes(&wire).expect("decode native org event");
        assert_eq!(decoded, event);
        assert_eq!(decoded.event_type(), "OrgStateCaptured");
    }

    #[test]
    fn org_state_schema_identity_is_stable() {
        assert_eq!(
            <OrgStateCaptured as GenomeSafe>::SCHEMA_HASH,
            220_908_143_069_358_905_578_364_172_905_019_209_814_u128
        );
        assert_eq!(
            pardosa::store::Event::<OrgStateCaptured>::ENVELOPE_HASH,
            330_380_791_181_709_376_046_586_033_837_479_802_840_u128
        );
        assert_eq!(
            <OrgStateCaptured as HasEventSchemaSource>::EVENT_SCHEMA_SOURCE,
            Some("gh-report/OrgEvent")
        );
        assert_ne!(
            <OrgStateCaptured as HasEventSchemaSource>::EVENT_SCHEMA_SOURCE,
            <DomainEvent as HasEventSchemaSource>::EVENT_SCHEMA_SOURCE
        );
    }

    #[test]
    fn schema_hash_is_stable_across_reads() {
        let first = <DomainEvent as GenomeSafe>::SCHEMA_HASH;
        let second = <DomainEvent as GenomeSafe>::SCHEMA_HASH;
        assert_eq!(first, second);
        assert_eq!(
            first,
            275_195_719_777_709_701_441_897_251_379_147_148_878_u128
        );
        assert_ne!(
            first, 19_710_905_809_486_475_925_592_730_934_028_496_282_u128,
            "current schema hash must differ from the prior P4b value"
        );
    }

    #[test]
    fn sweep_timeout_event_schema_identity_is_stable() {
        assert_eq!(
            <SweepTimeoutEvent as GenomeSafe>::SCHEMA_HASH,
            301_696_112_480_366_676_711_246_767_551_761_140_277_u128
        );
        assert_ne!(
            <SweepTimeoutEvent as GenomeSafe>::SCHEMA_HASH,
            <DomainEvent as GenomeSafe>::SCHEMA_HASH
        );
        assert_eq!(
            SweepTimeoutEvent::target_opened(uuid::Uuid::from_u128(1)).event_type(),
            "gh-report.sweep_timeout_target_opened"
        );
    }

    #[test]
    fn repository_event_envelope_identity_is_stable() {
        assert_eq!(
            pardosa::store::Event::<DomainEvent>::ENVELOPE_HASH,
            115_262_504_534_011_819_886_868_485_259_975_564_459_u128
        );
        assert_eq!(
            <DomainEvent as HasEventSchemaSource>::EVENT_SCHEMA_SOURCE,
            Some("gh-report/DomainEvent")
        );
    }

    #[test]
    fn oversized_topic_rejected_at_construction() {
        let too_long = "x".repeat(MAX_TOPIC + 1);
        let err = EventString::<MAX_TOPIC>::try_from(too_long).expect_err("over-MAX rejects");
        assert!(matches!(err, DomainError::TooLong { .. }));
    }
}
