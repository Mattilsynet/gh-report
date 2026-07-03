use jiff::Timestamp as JiffTimestamp;
use pardosa_schema::{EventString, EventVec, NonEmptyEventString, Timestamp};

use super::limits::{
    MAX_ALERT_BUCKET, MAX_ALERT_BUCKETS, MAX_ASSESSMENT_DATE, MAX_BRANCH_NAME,
    MAX_CODEOWNERS_OWNER, MAX_CODEOWNERS_PATTERN, MAX_DESCRIPTION, MAX_DOMAIN_KEY, MAX_GITHUB_ID,
    MAX_LANGUAGE, MAX_LICENSE, MAX_LOGIN, MAX_NODE_ID, MAX_ORG_ALERT_REPOS, MAX_PATH,
    MAX_PERSON_NAME, MAX_REASON, MAX_REPO_NAME, MAX_RUN_ID, MAX_SCHEMA_VERSION, MAX_TIMESTAMP_TEXT,
    MAX_TOKEN_SCOPES, MAX_TOPIC, MAX_URL,
};
use super::{
    AssessmentMetadata, AuthMode, BranchProtectionDetails, BranchProtectionResult,
    BranchProtectionStatus, Capability, CodeownersEntry, CodeownersResult, CodeownersStatus,
    CodeownersTruncationReason, CollectionFailureReason, CollectionStatus, DependabotResult,
    DependabotStatus, LastCommitInfo, OrgAlertSummary, OrgStateCaptured, ParsedCodeowners,
    RepoAlertSummary, RepoAlertSummaryEntry, Repository, RepositoryChecks, RepositoryEvidence,
    SecretScanningResult, SecretScanningStatus, SecurityPolicyEvidence, SecurityPolicyResult,
    SecurityPolicyStatus, StringU64Entry, TokenTier, Visibility,
};
use crate::domain::auth as sa;
use crate::domain::checks as s;
use crate::domain::codeowners as sc;
use crate::domain::evidence as se;
use crate::domain::metrics as sm;
use crate::domain::repository as sr;
use crate::domain::status as ss;
use crate::domain::time::parse_iso8601;

/// Failure converting a serde domain value into its native pardosa
/// counterpart.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
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

fn to_event_vec<T, U, const MAX: usize>(
    field: &'static str,
    values: impl IntoIterator<Item = T>,
    convert: impl FnMut(T) -> Conv<U>,
) -> Conv<EventVec<U, MAX>> {
    let converted = values.into_iter().map(convert).collect::<Conv<Vec<_>>>()?;
    EventVec::try_from(converted).map_err(|_| EventConversionError::TooMany { field })
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
    let nanos =
        u64::try_from(parsed.as_nanosecond()).map_err(|_| EventConversionError::BadTimestamp {
            field,
            value: value.to_string(),
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

macro_rules! conversion_pair {
    (
        $domain:ty => $event:ty {
            try_from($source:ident) {
                $($event_field:ident: $event_value:expr),+ $(,)?
            }
            from($converted:ident) {
                $($domain_field:ident: $domain_value:expr),+ $(,)?
            }
        }
    ) => {
        impl TryFrom<$domain> for $event {
            type Error = EventConversionError;

            fn try_from($source: $domain) -> Conv<Self> {
                Ok(Self {
                    $($event_field: $event_value,)+
                })
            }
        }

        impl From<$event> for $domain {
            fn from($converted: $event) -> Self {
                Self {
                    $($domain_field: $domain_value,)+
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

bijective_enum!(s::CollectionFailureReason <=> CollectionFailureReason {
    PermissionDenied,
    PermissionSuspected,
    NotFoundAbsent,
    Transient,
    RateLimited,
    Invalid,
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

bijective_enum!(sa::TokenTier <=> TokenTier { Full, Limited, Unknown });

bijective_enum!(sa::Capability <=> Capability {
    OrgSecretScanningAlerts,
    PrivateBranchProtectionRead,
});

bijective_enum!(sa::AuthMode <=> AuthMode {
    Pat,
    GitHubApp,
    GhCliFallback,
    Unknown,
});

bijective_enum!(ss::CollectionStatus <=> CollectionStatus {
    Success,
    NotCollected,
    PermissionDenied,
    TransientError,
    Unavailable,
});

conversion_pair!(sr::Repository => Repository {
    try_from(r) {
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
        topics: to_event_vec("repository.topics", r.topics, |topic| {
            to_es::<MAX_TOPIC>("repository.topics", topic)
        })?,
        license_spdx: to_es_opt::<MAX_LICENSE>("repository.license_spdx", r.license_spdx)?,
    }
    from(r) {
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
});

conversion_pair!(s::SecurityPolicyResult => SecurityPolicyResult {
    try_from(v) {
        status: v.status.into(),
        evidence: v.evidence.into(),
        path: to_es_opt::<MAX_PATH>("security_policy.path", v.path)?,
        timestamp: ts_required("security_policy.timestamp", &v.timestamp)?,
    }
    from(v) {
        status: v.status.into(),
        evidence: v.evidence.into(),
        path: v.path.map(|p| p.as_str().to_string()),
        timestamp: ts_to_string(v.timestamp),
    }
});

conversion_pair!(s::SecretScanningResult => SecretScanningResult {
    try_from(v) {
        status: v.status.into(),
        has_open_alerts: v.has_open_alerts,
        alerts_observable: v.alerts_observable,
        reason: to_es_opt::<MAX_REASON>("secret_scanning.reason", v.reason)?,
        timestamp: ts_required("secret_scanning.timestamp", &v.timestamp)?,
    }
    from(v) {
        status: v.status.into(),
        has_open_alerts: v.has_open_alerts,
        alerts_observable: v.alerts_observable,
        reason: v.reason.map(|r| r.as_str().to_string()),
        timestamp: ts_to_string(v.timestamp),
    }
});

conversion_pair!(s::DependabotResult => DependabotResult {
    try_from(v) {
        status: v.status.into(),
        reason: to_es_opt::<MAX_REASON>("dependabot.reason", v.reason)?,
        timestamp: ts_required("dependabot.timestamp", &v.timestamp)?,
    }
    from(v) {
        status: v.status.into(),
        reason: v.reason.map(|r| r.as_str().to_string()),
        timestamp: ts_to_string(v.timestamp),
    }
});

conversion_pair!(s::BranchProtectionDetails => BranchProtectionDetails {
    try_from(v) {
        default_branch: to_nes::<MAX_BRANCH_NAME>(
            "branch_protection.default_branch",
            &v.default_branch,
        )?,
        has_pr: v.has_pr,
        required_reviewers: v.required_reviewers,
        has_status_checks: v.has_status_checks,
        admin_equivalent: v.admin_equivalent,
        has_broad_bypass: v.has_broad_bypass,
        reason: to_es_opt::<MAX_REASON>("branch_protection.reason", v.reason)?,
        reason_kind: v.reason_kind.map(Into::into),
        http_status: v.http_status,
        force_push_blocked: v.force_push_blocked,
        deletion_blocked: v.deletion_blocked,
    }
    from(v) {
        default_branch: v.default_branch.as_str().to_string(),
        has_pr: v.has_pr,
        required_reviewers: v.required_reviewers,
        has_status_checks: v.has_status_checks,
        admin_equivalent: v.admin_equivalent,
        has_broad_bypass: v.has_broad_bypass,
        reason: v.reason.map(|r| r.as_str().to_string()),
        reason_kind: v.reason_kind.map(Into::into),
        http_status: v.http_status,
        force_push_blocked: v.force_push_blocked,
        deletion_blocked: v.deletion_blocked,
    }
});

conversion_pair!(s::BranchProtectionResult => BranchProtectionResult {
    try_from(v) {
        status: v.status.into(),
        details: BranchProtectionDetails::try_from(v.details)?,
        timestamp: ts_required("branch_protection.timestamp", &v.timestamp)?,
    }
    from(v) {
        status: v.status.into(),
        details: v.details.into(),
        timestamp: ts_to_string(v.timestamp),
    }
});

conversion_pair!(sc::CodeownersEntry => CodeownersEntry {
    try_from(v) {
        pattern: to_es::<MAX_CODEOWNERS_PATTERN>("codeowners.entry.pattern", v.pattern)?,
        owners: to_event_vec("codeowners.entry.owners", v.owners, |owner| {
            to_es::<MAX_CODEOWNERS_OWNER>("codeowners.entry.owners", owner)
        })?,
    }
    from(v) {
        pattern: v.pattern.as_str().to_string(),
        owners: v.owners.iter().map(|o| o.as_str().to_string()).collect(),
    }
});

conversion_pair!(sc::ParsedCodeowners => ParsedCodeowners {
    try_from(v) {
        entries: to_event_vec("codeowners.entries", v.entries, CodeownersEntry::try_from)?,
        unique_owners: to_event_vec("codeowners.unique_owners", v.unique_owners, |owner| {
            to_es::<MAX_CODEOWNERS_OWNER>("codeowners.unique_owners", owner)
        })?,
        skipped_lines: v.skipped_lines,
    }
    from(v) {
        entries: v.entries.iter().cloned().map(Into::into).collect(),
        unique_owners: v
            .unique_owners
            .iter()
            .map(|o| o.as_str().to_string())
            .collect(),
        skipped_lines: v.skipped_lines,
    }
});

conversion_pair!(s::CodeownersResult => CodeownersResult {
    try_from(v) {
        status: v.status.into(),
        path: to_es_opt::<MAX_PATH>("codeowners.path", v.path)?,
        timestamp: ts_required("codeowners.timestamp", &v.timestamp)?,
        parsed: v.parsed.map(ParsedCodeowners::try_from).transpose()?,
        truncation: v.truncation.map(Into::into),
    }
    from(v) {
        status: v.status.into(),
        path: v.path.map(|p| p.as_str().to_string()),
        timestamp: ts_to_string(v.timestamp),
        parsed: v.parsed.map(Into::into),
        truncation: v.truncation.map(Into::into),
    }
});

conversion_pair!(s::RepositoryChecks => RepositoryChecks {
    try_from(v) {
        security_policy: SecurityPolicyResult::try_from(v.security_policy)?,
        secret_scanning: SecretScanningResult::try_from(v.secret_scanning)?,
        dependabot_security_updates: DependabotResult::try_from(v.dependabot_security_updates)?,
        branch_protection: BranchProtectionResult::try_from(v.branch_protection)?,
        codeowners: CodeownersResult::try_from(v.codeowners)?,
    }
    from(v) {
        security_policy: v.security_policy.into(),
        secret_scanning: v.secret_scanning.into(),
        dependabot_security_updates: v.dependabot_security_updates.into(),
        branch_protection: v.branch_protection.into(),
        codeowners: v.codeowners.into(),
    }
});

conversion_pair!(se::LastCommitInfo => LastCommitInfo {
    try_from(v) {
        committer_login: to_es_opt::<MAX_LOGIN>(
            "last_commit.committer_login",
            v.committer_login,
        )?,
        committer_name: to_es_opt::<MAX_PERSON_NAME>(
            "last_commit.committer_name",
            v.committer_name,
        )?,
        commit_date: ts_opt("last_commit.commit_date", v.commit_date.as_deref())?,
    }
    from(v) {
        committer_login: v.committer_login.map(|c| c.as_str().to_string()),
        committer_name: v.committer_name.map(|c| c.as_str().to_string()),
        commit_date: ts_to_string_opt(v.commit_date),
    }
});

conversion_pair!(se::RepositoryEvidence => RepositoryEvidence {
    try_from(v) {
        repository: Repository::try_from(v.repository)?,
        checks: RepositoryChecks::try_from(v.checks)?,
        last_commit: v.last_commit.map(LastCommitInfo::try_from).transpose()?,
    }
    from(v) {
        repository: v.repository.into(),
        checks: v.checks.into(),
        last_commit: v.last_commit.map(Into::into),
    }
});

conversion_pair!(se::AssessmentMetadata => AssessmentMetadata {
    try_from(v) {
        date: to_es::<MAX_ASSESSMENT_DATE>("assessment_metadata.date", v.date)?,
        organization: to_es::<MAX_LOGIN>("assessment_metadata.organization", v.organization)?,
        schema_version: to_es::<MAX_SCHEMA_VERSION>(
            "assessment_metadata.schema_version",
            v.schema_version,
        )?,
        run_timestamp: to_es::<MAX_TIMESTAMP_TEXT>(
            "assessment_metadata.run_timestamp",
            v.run_timestamp,
        )?,
        run_id: to_es::<MAX_RUN_ID>("assessment_metadata.run_id", v.run_id)?,
        token_tier: v.token_tier.into(),
        token_scopes: to_es::<MAX_TOKEN_SCOPES>(
            "assessment_metadata.token_scopes",
            v.token_scopes,
        )?,
        auth_mode: v.auth_mode.into(),
        rate_limit_warnings: v.rate_limit_warnings,
        unavailable_capabilities: to_event_vec(
            "assessment_metadata.unavailable_capabilities",
            v.unavailable_capabilities,
            |capability| Ok(capability.into()),
        )?,
        inventory_fetched_at: to_es_opt::<MAX_TIMESTAMP_TEXT>(
            "assessment_metadata.inventory_fetched_at",
            v.inventory_fetched_at,
        )?,
        warm_start: v.warm_start,
    }
    from(v) {
        date: v.date.into_inner(),
        organization: v.organization.into_inner(),
        schema_version: v.schema_version.into_inner(),
        run_timestamp: v.run_timestamp.into_inner(),
        run_id: v.run_id.into_inner(),
        token_tier: v.token_tier.into(),
        token_scopes: v.token_scopes.into_inner(),
        auth_mode: v.auth_mode.into(),
        rate_limit_warnings: v.rate_limit_warnings,
        unavailable_capabilities: v
            .unavailable_capabilities
            .into_inner()
            .into_iter()
            .map(Into::into)
            .collect(),
        inventory_fetched_at: v.inventory_fetched_at.map(EventString::into_inner),
        warm_start: v.warm_start,
    }
});

conversion_pair!(sm::RepoAlertSummary => RepoAlertSummary {
    try_from(v) {
        open_alert_count: v.open_alert_count,
        oldest_open_alert_created_at: to_es_opt::<MAX_TIMESTAMP_TEXT>(
            "repo_alert_summary.oldest_open_alert_created_at",
            v.oldest_open_alert_created_at,
        )?,
        newest_open_alert_created_at: to_es_opt::<MAX_TIMESTAMP_TEXT>(
            "repo_alert_summary.newest_open_alert_created_at",
            v.newest_open_alert_created_at,
        )?,
    }
    from(v) {
        open_alert_count: v.open_alert_count,
        oldest_open_alert_created_at: v.oldest_open_alert_created_at.map(EventString::into_inner),
        newest_open_alert_created_at: v.newest_open_alert_created_at.map(EventString::into_inner),
    }
});

impl TryFrom<sm::OrgAlertSummary> for OrgAlertSummary {
    type Error = EventConversionError;

    fn try_from(v: sm::OrgAlertSummary) -> Conv<Self> {
        let mut per_repo = Vec::with_capacity(v.per_repo.len());
        for (repository_id, summary) in v.per_repo {
            per_repo.push(RepoAlertSummaryEntry {
                repository_id: to_es::<MAX_GITHUB_ID>(
                    "org_alert_summary.per_repo.key",
                    repository_id,
                )?,
                summary: RepoAlertSummary::try_from(summary)?,
            });
        }
        per_repo.sort_by(|left, right| {
            left.repository_id
                .as_str()
                .cmp(right.repository_id.as_str())
        });
        let per_repo = EventVec::<_, MAX_ORG_ALERT_REPOS>::try_from(per_repo).map_err(|_| {
            EventConversionError::TooMany {
                field: "org_alert_summary.per_repo",
            }
        })?;

        let mut open_secret_alert_age_buckets =
            Vec::with_capacity(v.open_secret_alert_age_buckets.len());
        for (key, value) in v.open_secret_alert_age_buckets {
            open_secret_alert_age_buckets.push(StringU64Entry {
                key: to_es::<MAX_ALERT_BUCKET>(
                    "org_alert_summary.open_secret_alert_age_buckets.key",
                    key,
                )?,
                value,
            });
        }
        open_secret_alert_age_buckets
            .sort_by(|left, right| left.key.as_str().cmp(right.key.as_str()));
        let open_secret_alert_age_buckets = EventVec::<_, MAX_ALERT_BUCKETS>::try_from(
            open_secret_alert_age_buckets,
        )
        .map_err(|_| EventConversionError::TooMany {
            field: "org_alert_summary.open_secret_alert_age_buckets",
        })?;

        Ok(Self {
            collection_status: v.collection_status.into(),
            collection_reason: to_es_opt::<MAX_REASON>(
                "org_alert_summary.collection_reason",
                v.collection_reason,
            )?,
            per_repo,
            open_secret_alert_age_buckets,
            total_open_secret_alerts: v.total_open_secret_alerts,
            oldest_open_secret_alert_created_at: to_es_opt::<MAX_TIMESTAMP_TEXT>(
                "org_alert_summary.oldest_open_secret_alert_created_at",
                v.oldest_open_secret_alert_created_at,
            )?,
            newest_open_secret_alert_created_at: to_es_opt::<MAX_TIMESTAMP_TEXT>(
                "org_alert_summary.newest_open_secret_alert_created_at",
                v.newest_open_secret_alert_created_at,
            )?,
        })
    }
}

impl From<OrgAlertSummary> for sm::OrgAlertSummary {
    fn from(v: OrgAlertSummary) -> Self {
        Self {
            collection_status: v.collection_status.into(),
            collection_reason: v.collection_reason.map(EventString::into_inner),
            per_repo: v
                .per_repo
                .into_inner()
                .into_iter()
                .map(|entry| (entry.repository_id.into_inner(), entry.summary.into()))
                .collect(),
            open_secret_alert_age_buckets: v
                .open_secret_alert_age_buckets
                .into_inner()
                .into_iter()
                .map(|entry| (entry.key.into_inner(), entry.value))
                .collect(),
            total_open_secret_alerts: v.total_open_secret_alerts,
            oldest_open_secret_alert_created_at: v
                .oldest_open_secret_alert_created_at
                .map(EventString::into_inner),
            newest_open_secret_alert_created_at: v
                .newest_open_secret_alert_created_at
                .map(EventString::into_inner),
        }
    }
}

impl TryFrom<se::OrgStateSnapshot> for OrgStateCaptured {
    type Error = EventConversionError;

    fn try_from(v: se::OrgStateSnapshot) -> Conv<Self> {
        Ok(Self {
            archived_repos: v.archived_repos,
            assessment_metadata: AssessmentMetadata::try_from(v.assessment_metadata)?,
            alert_summary: OrgAlertSummary::try_from(v.alert_summary)?,
        })
    }
}

impl From<OrgStateCaptured> for se::OrgStateSnapshot {
    fn from(v: OrgStateCaptured) -> Self {
        Self {
            archived_repos: v.archived_repos,
            assessment_metadata: v.assessment_metadata.into(),
            alert_summary: v.alert_summary.into(),
        }
    }
}
