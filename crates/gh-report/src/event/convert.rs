use jiff::Timestamp as JiffTimestamp;
use pardosa_schema::{EventString, EventVec, NonEmptyEventString, Timestamp};

use super::limits::{
    MAX_BRANCH_NAME, MAX_CODEOWNERS_ENTRIES, MAX_CODEOWNERS_OWNER, MAX_CODEOWNERS_OWNERS,
    MAX_CODEOWNERS_PATTERN, MAX_DESCRIPTION, MAX_DOMAIN_KEY, MAX_GITHUB_ID, MAX_LANGUAGE,
    MAX_LICENSE, MAX_LOGIN, MAX_NODE_ID, MAX_PATH, MAX_PERSON_NAME, MAX_REASON, MAX_REPO_NAME,
    MAX_TOPIC, MAX_TOPICS, MAX_URL,
};
use super::{
    BranchProtectionDetails, BranchProtectionResult, BranchProtectionStatus, CodeownersEntry,
    CodeownersResult, CodeownersStatus, CodeownersTruncationReason, DependabotResult,
    DependabotStatus, LastCommitInfo, ParsedCodeowners, Repository, RepositoryChecks,
    RepositoryEvidence, SecretScanningResult, SecretScanningStatus, SecurityPolicyEvidence,
    SecurityPolicyResult, SecurityPolicyStatus, Visibility,
};
use crate::domain::checks as s;
use crate::domain::codeowners as sc;
use crate::domain::evidence as se;
use crate::domain::repository as sr;
use crate::domain::time::parse_iso8601;

/// Failure converting a serde domain value into its native pardosa
/// counterpart.
#[derive(Debug, thiserror::Error)]
pub enum EventConversionError {
    #[error("field {field}: value exceeds bounded length")]
    TooLong { field: &'static str },
    #[error("field {field}: empty value where non-empty required")]
    Empty { field: &'static str },
    #[error("field {field}: unparseable or out-of-range timestamp {value:?}")]
    BadTimestamp { field: &'static str, value: String },
    #[error("field {field}: collection exceeds bounded capacity")]
    TooMany { field: &'static str },
}

type Conv<T> = Result<T, EventConversionError>;

fn to_es<const MAX: usize>(field: &'static str, value: String) -> Conv<EventString<MAX>> {
    EventString::try_from(value).map_err(|_| EventConversionError::TooLong { field })
}

fn to_es_opt<const MAX: usize>(
    field: &'static str,
    value: Option<String>,
) -> Conv<Option<EventString<MAX>>> {
    value.map(|v| to_es(field, v)).transpose()
}

fn to_nes<const MAX: usize>(field: &'static str, value: &str) -> Conv<NonEmptyEventString<MAX>> {
    NonEmptyEventString::try_new(value).map_err(|_| {
        if value.is_empty() {
            EventConversionError::Empty { field }
        } else {
            EventConversionError::TooLong { field }
        }
    })
}

fn ts_required(field: &'static str, value: &str) -> Conv<Timestamp> {
    let parsed = parse_iso8601(value).ok_or_else(|| EventConversionError::BadTimestamp {
        field,
        value: value.to_string(),
    })?;
    let nanos = u64::try_from(parsed.as_nanosecond()).map_err(|_| {
        EventConversionError::BadTimestamp {
            field,
            value: value.to_string(),
        }
    })?;
    Timestamp::from_nanos(nanos).ok_or_else(|| EventConversionError::BadTimestamp {
        field,
        value: value.to_string(),
    })
}

fn ts_opt(field: &'static str, value: Option<&str>) -> Conv<Option<Timestamp>> {
    match value {
        None => Ok(None),
        Some(v) => ts_required(field, v).map(Some),
    }
}

fn ts_to_string(ts: Timestamp) -> String {
    JiffTimestamp::from_nanosecond(i128::from(ts.as_nanos()))
        .map_or_else(|_| String::new(), |t| t.to_string())
}

fn ts_to_string_opt(ts: Option<Timestamp>) -> Option<String> {
    ts.map(ts_to_string)
}

macro_rules! bijective_enum {
    ($domain_mod:ident::$domain_enum:ident <=> $event:ident { $($variant:ident),+ $(,)? }) => {
        impl From<$domain_mod::$domain_enum> for $event {
            fn from(v: $domain_mod::$domain_enum) -> Self {
                match v {
                    $(
                        $domain_mod::$domain_enum::$variant => Self::$variant,
                    )+
                }
            }
        }

        impl From<$event> for $domain_mod::$domain_enum {
            fn from(v: $event) -> Self {
                match v {
                    $(
                        $event::$variant => Self::$variant,
                    )+
                }
            }
        }
    };
}

bijective_enum!(sr::Visibility <=> Visibility { Public, Internal, Private });

bijective_enum!(s::SecurityPolicyStatus <=> SecurityPolicyStatus {
    Pass,
    Fail,
    Unknown,
    NotApplicable,
});

bijective_enum!(s::SecurityPolicyEvidence <=> SecurityPolicyEvidence {
    Setting,
    File,
    Absent,
    PermissionDenied,
    TransientError,
    CollectionError,
    NotApplicable,
});

bijective_enum!(s::SecretScanningStatus <=> SecretScanningStatus {
    Enabled,
    Disabled,
    PermissionDenied,
    Unknown,
});

bijective_enum!(s::DependabotStatus <=> DependabotStatus {
    Enabled,
    Paused,
    Disabled,
    Unknown,
});

bijective_enum!(s::BranchProtectionStatus <=> BranchProtectionStatus {
    Pass,
    Partial,
    Fail,
    Unknown,
});

bijective_enum!(s::CodeownersStatus <=> CodeownersStatus {
    Conforming,
    NonConforming,
    Absent,
    Unknown,
});

bijective_enum!(sc::CodeownersTruncationReason <=> CodeownersTruncationReason {
    NotBase64Encoded,
    OversizedBase64,
    ContentMissing,
    DecodeFailed,
    InvalidUtf8,
});

impl TryFrom<sr::Repository> for Repository {
    type Error = EventConversionError;

    fn try_from(r: sr::Repository) -> Conv<Self> {
        let mut topics = Vec::with_capacity(r.topics.len());
        for topic in r.topics {
            topics.push(to_es::<MAX_TOPIC>("repository.topics", topic)?);
        }
        let topics = EventVec::<_, MAX_TOPICS>::try_from(topics)
            .map_err(|_| EventConversionError::TooMany { field: "repository.topics" })?;
        Ok(Self {
            id: to_nes::<MAX_GITHUB_ID>("repository.id", &r.id)?,
            node_id: to_es_opt::<MAX_NODE_ID>("repository.node_id", r.node_id)?,
            name: to_nes::<MAX_REPO_NAME>("repository.name", &r.name)?,
            visibility: r.visibility.into(),
            language: to_es_opt::<MAX_LANGUAGE>("repository.language", r.language)?,
            default_branch: to_nes::<MAX_BRANCH_NAME>("repository.default_branch", &r.default_branch)?,
            archived: r.archived,
            inventory_key: to_nes::<MAX_DOMAIN_KEY>("repository.inventory_key", &r.inventory_key)?,
            updated_at: ts_opt("repository.updated_at", r.updated_at.as_deref())?,
            has_issues: r.has_issues,
            pushed_at: ts_opt("repository.pushed_at", r.pushed_at.as_deref())?,
            created_at: ts_opt("repository.created_at", r.created_at.as_deref())?,
            description: to_es_opt::<MAX_DESCRIPTION>("repository.description", r.description)?,
            fork: r.fork,
            html_url: to_es_opt::<MAX_URL>("repository.html_url", r.html_url)?,
            topics,
            license_spdx: to_es_opt::<MAX_LICENSE>("repository.license_spdx", r.license_spdx)?,
        })
    }
}

impl From<Repository> for sr::Repository {
    fn from(r: Repository) -> Self {
        Self {
            id: r.id.as_str().to_string(),
            node_id: r.node_id.map(|v| v.as_str().to_string()),
            name: r.name.as_str().to_string(),
            visibility: r.visibility.into(),
            language: r.language.map(|v| v.as_str().to_string()),
            default_branch: r.default_branch.as_str().to_string(),
            archived: r.archived,
            inventory_key: r.inventory_key.as_str().to_string(),
            updated_at: ts_to_string_opt(r.updated_at),
            has_issues: r.has_issues,
            pushed_at: ts_to_string_opt(r.pushed_at),
            created_at: ts_to_string_opt(r.created_at),
            description: r.description.map(|v| v.as_str().to_string()),
            fork: r.fork,
            html_url: r.html_url.map(|v| v.as_str().to_string()),
            topics: r.topics.iter().map(|t| t.as_str().to_string()).collect(),
            license_spdx: r.license_spdx.map(|v| v.as_str().to_string()),
        }
    }
}

impl TryFrom<s::SecurityPolicyResult> for SecurityPolicyResult {
    type Error = EventConversionError;

    fn try_from(v: s::SecurityPolicyResult) -> Conv<Self> {
        Ok(Self {
            status: v.status.into(),
            evidence: v.evidence.into(),
            path: to_es_opt::<MAX_PATH>("security_policy.path", v.path)?,
            timestamp: ts_required("security_policy.timestamp", &v.timestamp)?,
        })
    }
}

impl From<SecurityPolicyResult> for s::SecurityPolicyResult {
    fn from(v: SecurityPolicyResult) -> Self {
        Self {
            status: v.status.into(),
            evidence: v.evidence.into(),
            path: v.path.map(|p| p.as_str().to_string()),
            timestamp: ts_to_string(v.timestamp),
        }
    }
}

impl TryFrom<s::SecretScanningResult> for SecretScanningResult {
    type Error = EventConversionError;

    fn try_from(v: s::SecretScanningResult) -> Conv<Self> {
        Ok(Self {
            status: v.status.into(),
            has_open_alerts: v.has_open_alerts,
            alerts_observable: v.alerts_observable,
            reason: to_es_opt::<MAX_REASON>("secret_scanning.reason", v.reason)?,
            timestamp: ts_required("secret_scanning.timestamp", &v.timestamp)?,
        })
    }
}

impl From<SecretScanningResult> for s::SecretScanningResult {
    fn from(v: SecretScanningResult) -> Self {
        Self {
            status: v.status.into(),
            has_open_alerts: v.has_open_alerts,
            alerts_observable: v.alerts_observable,
            reason: v.reason.map(|r| r.as_str().to_string()),
            timestamp: ts_to_string(v.timestamp),
        }
    }
}

impl TryFrom<s::DependabotResult> for DependabotResult {
    type Error = EventConversionError;

    fn try_from(v: s::DependabotResult) -> Conv<Self> {
        Ok(Self {
            status: v.status.into(),
            reason: to_es_opt::<MAX_REASON>("dependabot.reason", v.reason)?,
            timestamp: ts_required("dependabot.timestamp", &v.timestamp)?,
        })
    }
}

impl From<DependabotResult> for s::DependabotResult {
    fn from(v: DependabotResult) -> Self {
        Self {
            status: v.status.into(),
            reason: v.reason.map(|r| r.as_str().to_string()),
            timestamp: ts_to_string(v.timestamp),
        }
    }
}

impl TryFrom<s::BranchProtectionDetails> for BranchProtectionDetails {
    type Error = EventConversionError;

    fn try_from(v: s::BranchProtectionDetails) -> Conv<Self> {
        Ok(Self {
            default_branch: to_nes::<MAX_BRANCH_NAME>("branch_protection.default_branch", &v.default_branch)?,
            has_pr: v.has_pr,
            required_reviewers: v.required_reviewers,
            has_status_checks: v.has_status_checks,
            admin_equivalent: v.admin_equivalent,
            has_broad_bypass: v.has_broad_bypass,
            reason: to_es_opt::<MAX_REASON>("branch_protection.reason", v.reason)?,
        })
    }
}

impl From<BranchProtectionDetails> for s::BranchProtectionDetails {
    fn from(v: BranchProtectionDetails) -> Self {
        Self {
            default_branch: v.default_branch.as_str().to_string(),
            has_pr: v.has_pr,
            required_reviewers: v.required_reviewers,
            has_status_checks: v.has_status_checks,
            admin_equivalent: v.admin_equivalent,
            has_broad_bypass: v.has_broad_bypass,
            reason: v.reason.map(|r| r.as_str().to_string()),
        }
    }
}

impl TryFrom<s::BranchProtectionResult> for BranchProtectionResult {
    type Error = EventConversionError;

    fn try_from(v: s::BranchProtectionResult) -> Conv<Self> {
        Ok(Self {
            status: v.status.into(),
            details: BranchProtectionDetails::try_from(v.details)?,
            timestamp: ts_required("branch_protection.timestamp", &v.timestamp)?,
        })
    }
}

impl From<BranchProtectionResult> for s::BranchProtectionResult {
    fn from(v: BranchProtectionResult) -> Self {
        Self {
            status: v.status.into(),
            details: v.details.into(),
            timestamp: ts_to_string(v.timestamp),
        }
    }
}

impl TryFrom<sc::CodeownersEntry> for CodeownersEntry {
    type Error = EventConversionError;

    fn try_from(v: sc::CodeownersEntry) -> Conv<Self> {
        let mut owners = Vec::with_capacity(v.owners.len());
        for owner in v.owners {
            owners.push(to_es::<MAX_CODEOWNERS_OWNER>("codeowners.entry.owners", owner)?);
        }
        let owners = EventVec::<_, MAX_CODEOWNERS_OWNERS>::try_from(owners)
            .map_err(|_| EventConversionError::TooMany { field: "codeowners.entry.owners" })?;
        Ok(Self {
            pattern: to_es::<MAX_CODEOWNERS_PATTERN>("codeowners.entry.pattern", v.pattern)?,
            owners,
        })
    }
}

impl From<CodeownersEntry> for sc::CodeownersEntry {
    fn from(v: CodeownersEntry) -> Self {
        Self {
            pattern: v.pattern.as_str().to_string(),
            owners: v.owners.iter().map(|o| o.as_str().to_string()).collect(),
        }
    }
}

impl TryFrom<sc::ParsedCodeowners> for ParsedCodeowners {
    type Error = EventConversionError;

    fn try_from(v: sc::ParsedCodeowners) -> Conv<Self> {
        let mut entries = Vec::with_capacity(v.entries.len());
        for entry in v.entries {
            entries.push(CodeownersEntry::try_from(entry)?);
        }
        let entries = EventVec::<_, MAX_CODEOWNERS_ENTRIES>::try_from(entries)
            .map_err(|_| EventConversionError::TooMany { field: "codeowners.entries" })?;
        let mut unique_owners = Vec::with_capacity(v.unique_owners.len());
        for owner in v.unique_owners {
            unique_owners.push(to_es::<MAX_CODEOWNERS_OWNER>("codeowners.unique_owners", owner)?);
        }
        let unique_owners = EventVec::<_, MAX_CODEOWNERS_OWNERS>::try_from(unique_owners)
            .map_err(|_| EventConversionError::TooMany { field: "codeowners.unique_owners" })?;
        Ok(Self {
            entries,
            unique_owners,
            skipped_lines: v.skipped_lines,
        })
    }
}

impl From<ParsedCodeowners> for sc::ParsedCodeowners {
    fn from(v: ParsedCodeowners) -> Self {
        Self {
            entries: v.entries.iter().cloned().map(Into::into).collect(),
            unique_owners: v.unique_owners.iter().map(|o| o.as_str().to_string()).collect(),
            skipped_lines: v.skipped_lines,
        }
    }
}

impl TryFrom<s::CodeownersResult> for CodeownersResult {
    type Error = EventConversionError;

    fn try_from(v: s::CodeownersResult) -> Conv<Self> {
        Ok(Self {
            status: v.status.into(),
            path: to_es_opt::<MAX_PATH>("codeowners.path", v.path)?,
            timestamp: ts_required("codeowners.timestamp", &v.timestamp)?,
            parsed: v.parsed.map(ParsedCodeowners::try_from).transpose()?,
            truncation: v.truncation.map(Into::into),
        })
    }
}

impl From<CodeownersResult> for s::CodeownersResult {
    fn from(v: CodeownersResult) -> Self {
        Self {
            status: v.status.into(),
            path: v.path.map(|p| p.as_str().to_string()),
            timestamp: ts_to_string(v.timestamp),
            parsed: v.parsed.map(Into::into),
            truncation: v.truncation.map(Into::into),
        }
    }
}

impl TryFrom<s::RepositoryChecks> for RepositoryChecks {
    type Error = EventConversionError;

    fn try_from(v: s::RepositoryChecks) -> Conv<Self> {
        Ok(Self {
            security_policy: SecurityPolicyResult::try_from(v.security_policy)?,
            secret_scanning: SecretScanningResult::try_from(v.secret_scanning)?,
            dependabot_security_updates: DependabotResult::try_from(v.dependabot_security_updates)?,
            branch_protection: BranchProtectionResult::try_from(v.branch_protection)?,
            codeowners: CodeownersResult::try_from(v.codeowners)?,
        })
    }
}

impl From<RepositoryChecks> for s::RepositoryChecks {
    fn from(v: RepositoryChecks) -> Self {
        Self {
            security_policy: v.security_policy.into(),
            secret_scanning: v.secret_scanning.into(),
            dependabot_security_updates: v.dependabot_security_updates.into(),
            branch_protection: v.branch_protection.into(),
            codeowners: v.codeowners.into(),
        }
    }
}

impl TryFrom<se::LastCommitInfo> for LastCommitInfo {
    type Error = EventConversionError;

    fn try_from(v: se::LastCommitInfo) -> Conv<Self> {
        Ok(Self {
            committer_login: to_es_opt::<MAX_LOGIN>("last_commit.committer_login", v.committer_login)?,
            committer_name: to_es_opt::<MAX_PERSON_NAME>("last_commit.committer_name", v.committer_name)?,
            commit_date: ts_opt("last_commit.commit_date", v.commit_date.as_deref())?,
        })
    }
}

impl From<LastCommitInfo> for se::LastCommitInfo {
    fn from(v: LastCommitInfo) -> Self {
        Self {
            committer_login: v.committer_login.map(|c| c.as_str().to_string()),
            committer_name: v.committer_name.map(|c| c.as_str().to_string()),
            commit_date: ts_to_string_opt(v.commit_date),
        }
    }
}

impl TryFrom<se::RepositoryEvidence> for RepositoryEvidence {
    type Error = EventConversionError;

    fn try_from(v: se::RepositoryEvidence) -> Conv<Self> {
        Ok(Self {
            repository: Repository::try_from(v.repository)?,
            checks: RepositoryChecks::try_from(v.checks)?,
            last_commit: v.last_commit.map(LastCommitInfo::try_from).transpose()?,
        })
    }
}

impl From<RepositoryEvidence> for se::RepositoryEvidence {
    fn from(v: RepositoryEvidence) -> Self {
        Self {
            repository: v.repository.into(),
            checks: v.checks.into(),
            last_commit: v.last_commit.map(Into::into),
        }
    }
}
