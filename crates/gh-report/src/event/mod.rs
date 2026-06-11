#![forbid(unsafe_code)]

use pardosa::store::HasEventSchemaSource;
use pardosa_schema::{EventString, EventVec, GenomeSafe, NonEmptyEventString, Timestamp, Validate};

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
}

use limits::{
    MAX_BRANCH_NAME, MAX_CODEOWNERS_ENTRIES, MAX_CODEOWNERS_OWNER, MAX_CODEOWNERS_OWNERS,
    MAX_CODEOWNERS_PATTERN, MAX_DESCRIPTION, MAX_DOMAIN_KEY, MAX_GITHUB_ID, MAX_LANGUAGE,
    MAX_LICENSE, MAX_LOGIN, MAX_NODE_ID, MAX_PATH, MAX_PERSON_NAME, MAX_REASON, MAX_REPO_NAME,
    MAX_TOPIC, MAX_TOPICS, MAX_URL,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GenomeSafe)]
#[repr(u8)]
pub enum RepoPresence {
    Active = 0,
    Removed = 1,
}

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
#[repr(u8)]
pub enum DomainEvent {
    RepositoryStateCaptured {
        domain_key: NonEmptyEventString<MAX_DOMAIN_KEY>,
        repo_name: NonEmptyEventString<MAX_REPO_NAME>,
        timestamp: Timestamp,
        evidence: Option<Box<RepositoryEvidence>>,
        presence: RepoPresence,
    } = 0,
}

impl DomainEvent {
    #[must_use]
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::RepositoryStateCaptured { .. } => "RepositoryStateCaptured",
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

    #[test]
    fn native_repository_state_round_trips_with_removed_presence() {
        let event = DomainEvent::RepositoryStateCaptured {
            domain_key: nes("id-repo-1"),
            repo_name: nes("repo-1"),
            timestamp: ts(40),
            evidence: Some(Box::new(full_evidence())),
            presence: RepoPresence::Removed,
        };
        let wire = to_vec(&event);
        let decoded: DomainEvent = from_bytes(&wire).expect("decode native event");
        assert_eq!(decoded, event);
        assert_eq!(decoded.event_type(), "RepositoryStateCaptured");
    }

    #[test]
    fn schema_hash_is_stable_across_reads() {
        let first = <DomainEvent as GenomeSafe>::SCHEMA_HASH;
        let second = <DomainEvent as GenomeSafe>::SCHEMA_HASH;
        assert_eq!(first, second);
    }

    #[test]
    fn repo_presence_retains_active_and_removed_discriminants() {
        assert_eq!(RepoPresence::Active as u8, 0);
        assert_eq!(RepoPresence::Removed as u8, 1);
    }

    #[test]
    fn oversized_topic_rejected_at_construction() {
        let too_long = "x".repeat(MAX_TOPIC + 1);
        let err = EventString::<MAX_TOPIC>::try_from(too_long).expect_err("over-MAX rejects");
        assert!(matches!(err, DomainError::TooLong { .. }));
    }
}
