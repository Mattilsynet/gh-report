#![forbid(unsafe_code)]

use cherry_pit_core::{DomainEvent as CherryDomainEvent, ScheduledDomainEvent};
use pardosa::prelude::*;
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
    pub const MAX_TEAM_MEMBERS: usize = 16_384;
}

use limits::{
    MAX_ALERT_BUCKET, MAX_ALERT_BUCKETS, MAX_ASSESSMENT_DATE, MAX_BRANCH_NAME,
    MAX_CODEOWNERS_ENTRIES, MAX_CODEOWNERS_OWNER, MAX_CODEOWNERS_OWNERS, MAX_CODEOWNERS_PATTERN,
    MAX_DESCRIPTION, MAX_DOMAIN_KEY, MAX_GITHUB_ID, MAX_LANGUAGE, MAX_LICENSE, MAX_LOGIN,
    MAX_NODE_ID, MAX_ORG_ALERT_REPOS, MAX_PATH, MAX_PERSON_NAME, MAX_REASON, MAX_REPO_NAME,
    MAX_RUN_ID, MAX_SCHEMA_VERSION, MAX_SWEEP_TIMEOUT_ERROR, MAX_TEAM_MEMBERS, MAX_TIMESTAMP_TEXT,
    MAX_TOKEN_SCOPES, MAX_TOPIC, MAX_TOPICS, MAX_UNAVAILABLE_CAPABILITIES, MAX_URL,
};

/// Native schema version for [`DomainEvent`].
pub const DOMAIN_EVENT_SCHEMA_VERSION: u32 = 2;

/// Native schema version for [`OrgStateCaptured`].
pub const ORG_STATE_SCHEMA_VERSION: u32 = 2;

/// Native schema version for [`TeamStateCaptured`].
pub const TEAM_STATE_SCHEMA_VERSION: u32 = 2;

macro_rules! impl_pardosa_enum {
    ($ty:ident { $($variant:ident = $val:expr),* $(,)? }) => {
        impl PardosaType for $ty {
            fn descriptor_node() -> DescriptorNode {
                DescriptorNode::Enum {
                    name: stringify!($ty).to_string(),
                    discriminant_width: 1,
                    variants: vec![
                        $(
                            VariantDescriptor {
                                discriminant: $val,
                                name: stringify!($variant).to_string(),
                                payload: None,
                            },
                        )*
                    ],
                }
            }
            fn encode_type(&self, buf: &mut Vec<u8>) -> Result<(), EncodeError> {
                buf.push(*self as u8);
                Ok(())
            }
            fn decode_type(buf: &[u8]) -> Result<(Self, usize), DecodeError> {
                if buf.is_empty() {
                    return Err(DecodeError::TruncatedPayload { expected: 1, available: 0 });
                }
                let val = match buf[0] {
                    $($val => Self::$variant,)*
                    other => return Err(DecodeError::UnknownVariantDiscriminant { discriminant: u32::from(other) }),
                };
                Ok((val, 1))
            }
        }
    };
}

macro_rules! impl_pardosa_struct {
    ($ty:ident { $($field:ident : $fty:ty),* $(,)? }) => {
        impl PardosaType for $ty {
            fn descriptor_node() -> DescriptorNode {
                DescriptorNode::Struct {
                    name: stringify!($ty).to_string(),
                    fields: vec![
                        $(
                            FieldDescriptor {
                                name: stringify!($field).to_string(),
                                node: <$fty as PardosaType>::descriptor_node(),
                            },
                        )*
                    ],
                }
            }
            fn encode_type(&self, buf: &mut Vec<u8>) -> Result<(), EncodeError> {
                $(self.$field.encode_type(buf)?;)*
                Ok(())
            }
            fn decode_type(buf: &[u8]) -> Result<(Self, usize), DecodeError> {
                let mut cursor = 0;
                $(
                    let ($field, c) = <$fty as PardosaType>::decode_type(&buf[cursor..])?;
                    cursor += c;
                )*
                Ok((Self { $($field),* }, cursor))
            }
        }
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SweepTimeoutEvent {
    TargetOpened {
        event_id: uuid::Uuid,
    },
    TimeoutFired {
        event_id: uuid::Uuid,
        run_id: EventString<MAX_RUN_ID>,
        error: EventString<MAX_SWEEP_TIMEOUT_ERROR>,
        elapsed_ms: u64,
    },
}

impl PardosaType for SweepTimeoutEvent {
    fn descriptor_node() -> DescriptorNode {
        DescriptorNode::Enum {
            name: "SweepTimeoutEvent".to_string(),
            discriminant_width: 1,
            variants: vec![
                VariantDescriptor {
                    discriminant: 0,
                    name: "TargetOpened".to_string(),
                    payload: Some(DescriptorNode::Struct {
                        name: "SweepTimeoutEvent_TargetOpened".to_string(),
                        fields: vec![FieldDescriptor {
                            name: "event_id".to_string(),
                            node: <Uuid as PardosaType>::descriptor_node(),
                        }],
                    }),
                },
                VariantDescriptor {
                    discriminant: 1,
                    name: "TimeoutFired".to_string(),
                    payload: Some(DescriptorNode::Struct {
                        name: "SweepTimeoutEvent_TimeoutFired".to_string(),
                        fields: vec![
                            FieldDescriptor {
                                name: "event_id".to_string(),
                                node: <Uuid as PardosaType>::descriptor_node(),
                            },
                            FieldDescriptor {
                                name: "run_id".to_string(),
                                node: <EventString<MAX_RUN_ID> as PardosaType>::descriptor_node(),
                            },
                            FieldDescriptor {
                                name: "error".to_string(),
                                node: <EventString<MAX_SWEEP_TIMEOUT_ERROR> as PardosaType>::descriptor_node(),
                            },
                            FieldDescriptor {
                                name: "elapsed_ms".to_string(),
                                node: <u64 as PardosaType>::descriptor_node(),
                            },
                        ],
                    }),
                },
            ],
        }
    }
    fn encode_type(&self, buf: &mut Vec<u8>) -> Result<(), EncodeError> {
        match self {
            Self::TargetOpened { event_id } => {
                buf.push(0);
                Uuid::from(*event_id).encode_type(buf)?;
                Ok(())
            }
            Self::TimeoutFired {
                event_id,
                run_id,
                error,
                elapsed_ms,
            } => {
                buf.push(1);
                Uuid::from(*event_id).encode_type(buf)?;
                run_id.encode_type(buf)?;
                error.encode_type(buf)?;
                elapsed_ms.encode_type(buf)?;
                Ok(())
            }
        }
    }
    fn decode_type(buf: &[u8]) -> Result<(Self, usize), DecodeError> {
        if buf.is_empty() {
            return Err(DecodeError::TruncatedPayload {
                expected: 1,
                available: 0,
            });
        }
        let tag = buf[0];
        let mut cursor = 1;
        match tag {
            0 => {
                let (event_id, c) = Uuid::decode_type(&buf[cursor..])?;
                cursor += c;
                Ok((
                    Self::TargetOpened {
                        event_id: uuid::Uuid::from(event_id),
                    },
                    cursor,
                ))
            }
            1 => {
                let (event_id, c) = Uuid::decode_type(&buf[cursor..])?;
                cursor += c;
                let (run_id, c) = EventString::decode_type(&buf[cursor..])?;
                cursor += c;
                let (error, c) = EventString::decode_type(&buf[cursor..])?;
                cursor += c;
                let (elapsed_ms, c) = u64::decode_type(&buf[cursor..])?;
                cursor += c;
                Ok((
                    Self::TimeoutFired {
                        event_id: uuid::Uuid::from(event_id),
                        run_id,
                        error,
                        elapsed_ms,
                    },
                    cursor,
                ))
            }
            other => Err(DecodeError::UnknownVariantDiscriminant {
                discriminant: u32::from(other),
            }),
        }
    }
}

impl PardosaSchema for SweepTimeoutEvent {
    fn schema_version() -> u32 {
        1
    }
    fn schema_descriptor() -> DescriptorNode {
        Self::descriptor_node()
    }
    fn encode_payload(&self, buf: &mut Vec<u8>) -> Result<(), EncodeError> {
        self.encode_type(buf)
    }
    fn decode_payload(buf: &[u8]) -> Result<Self, DecodeError> {
        let (val, _) = Self::decode_type(buf)?;
        Ok(val)
    }
}

impl SweepTimeoutEvent {
    pub(crate) fn try_timeout_fired(
        event_id: uuid::Uuid,
        run_id: String,
        error: &str,
        elapsed_ms: u64,
    ) -> Result<Self, DecodeError> {
        Ok(Self::TimeoutFired {
            event_id,
            run_id: EventString::new(run_id)?,
            error: EventString::new(error.to_string())?,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryEvidence {
    pub repository: Repository,
    pub checks: RepositoryChecks,
    pub last_commit: Option<LastCommitInfo>,
}
impl_pardosa_struct!(RepositoryEvidence {
    repository: Repository,
    checks: RepositoryChecks,
    last_commit: Option<LastCommitInfo>,
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastCommitInfo {
    pub committer_login: Option<EventString<MAX_LOGIN>>,
    pub committer_name: Option<EventString<MAX_PERSON_NAME>>,
    pub commit_date: Option<Timestamp>,
}
impl_pardosa_struct!(LastCommitInfo {
    committer_login: Option<EventString<MAX_LOGIN>>,
    committer_name: Option<EventString<MAX_PERSON_NAME>>,
    commit_date: Option<Timestamp>,
});

#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent GitHub repository attribute (archived, has_issues, fork, is_empty)"
)]
pub struct Repository {
    pub id: NonEmptyEventString<MAX_GITHUB_ID>,
    pub node_id: Option<EventString<MAX_NODE_ID>>,
    pub name: NonEmptyEventString<MAX_REPO_NAME>,
    pub visibility: Visibility,
    pub language: Option<EventString<MAX_LANGUAGE>>,
    pub default_branch: NonEmptyEventString<MAX_BRANCH_NAME>,
    pub archived: bool,
    pub inventory_key: NonEmptyEventString<MAX_DOMAIN_KEY>,
    pub updated_at: Option<NonEmptyEventString<MAX_TIMESTAMP_TEXT>>,
    pub has_issues: bool,
    pub pushed_at: Option<Timestamp>,
    pub created_at: Option<Timestamp>,
    pub description: Option<EventString<MAX_DESCRIPTION>>,
    pub fork: bool,
    pub is_empty: bool,
    pub html_url: Option<EventString<MAX_URL>>,
    pub topics: EventVec<EventString<MAX_TOPIC>, MAX_TOPICS>,
    pub license_spdx: Option<EventString<MAX_LICENSE>>,
}
impl_pardosa_struct!(Repository {
    id: NonEmptyEventString<MAX_GITHUB_ID>,
    node_id: Option<EventString<MAX_NODE_ID>>,
    name: NonEmptyEventString<MAX_REPO_NAME>,
    visibility: Visibility,
    language: Option<EventString<MAX_LANGUAGE>>,
    default_branch: NonEmptyEventString<MAX_BRANCH_NAME>,
    archived: bool,
    inventory_key: NonEmptyEventString<MAX_DOMAIN_KEY>,
    updated_at: Option<NonEmptyEventString<MAX_TIMESTAMP_TEXT>>,
    has_issues: bool,
    pushed_at: Option<Timestamp>,
    created_at: Option<Timestamp>,
    description: Option<EventString<MAX_DESCRIPTION>>,
    fork: bool,
    is_empty: bool,
    html_url: Option<EventString<MAX_URL>>,
    topics: EventVec<EventString<MAX_TOPIC>, MAX_TOPICS>,
    license_spdx: Option<EventString<MAX_LICENSE>>,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Visibility {
    Public = 0,
    Internal = 1,
    Private = 2,
}
impl_pardosa_enum!(Visibility {
    Public = 0,
    Internal = 1,
    Private = 2
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryChecks {
    pub security_policy: SecurityPolicyResult,
    pub secret_scanning: SecretScanningResult,
    pub dependabot_security_updates: DependabotResult,
    pub branch_protection: BranchProtectionResult,
    pub codeowners: CodeownersResult,
}
impl_pardosa_struct!(RepositoryChecks {
    security_policy: SecurityPolicyResult,
    secret_scanning: SecretScanningResult,
    dependabot_security_updates: DependabotResult,
    branch_protection: BranchProtectionResult,
    codeowners: CodeownersResult,
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityPolicyResult {
    pub status: SecurityPolicyStatus,
    pub evidence: SecurityPolicyEvidence,
    pub path: Option<EventString<MAX_PATH>>,
    pub timestamp: Timestamp,
}
impl_pardosa_struct!(SecurityPolicyResult {
    status: SecurityPolicyStatus,
    evidence: SecurityPolicyEvidence,
    path: Option<EventString<MAX_PATH>>,
    timestamp: Timestamp,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SecurityPolicyStatus {
    Pass = 0,
    Fail = 1,
    Unknown = 2,
    NotApplicable = 3,
}
impl_pardosa_enum!(SecurityPolicyStatus {
    Pass = 0,
    Fail = 1,
    Unknown = 2,
    NotApplicable = 3
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
impl_pardosa_enum!(SecurityPolicyEvidence {
    Setting = 0,
    File = 1,
    Absent = 2,
    PermissionDenied = 3,
    TransientError = 4,
    CollectionError = 5,
    NotApplicable = 6
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretScanningResult {
    pub status: SecretScanningStatus,
    pub has_open_alerts: Option<bool>,
    pub alerts_observable: bool,
    pub reason: Option<EventString<MAX_REASON>>,
    pub timestamp: Timestamp,
}
impl_pardosa_struct!(SecretScanningResult {
    status: SecretScanningStatus,
    has_open_alerts: Option<bool>,
    alerts_observable: bool,
    reason: Option<EventString<MAX_REASON>>,
    timestamp: Timestamp,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SecretScanningStatus {
    Enabled = 0,
    Disabled = 1,
    PermissionDenied = 2,
    Unknown = 3,
}
impl_pardosa_enum!(SecretScanningStatus {
    Enabled = 0,
    Disabled = 1,
    PermissionDenied = 2,
    Unknown = 3
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependabotResult {
    pub status: DependabotStatus,
    pub reason: Option<EventString<MAX_REASON>>,
    pub timestamp: Timestamp,
}
impl_pardosa_struct!(DependabotResult {
    status: DependabotStatus,
    reason: Option<EventString<MAX_REASON>>,
    timestamp: Timestamp,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DependabotStatus {
    Enabled = 0,
    Paused = 1,
    Disabled = 2,
    Unknown = 3,
}
impl_pardosa_enum!(DependabotStatus {
    Enabled = 0,
    Paused = 1,
    Disabled = 2,
    Unknown = 3
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchProtectionResult {
    pub status: BranchProtectionStatus,
    pub details: BranchProtectionDetails,
    pub timestamp: Timestamp,
}
impl_pardosa_struct!(BranchProtectionResult {
    status: BranchProtectionStatus,
    details: BranchProtectionDetails,
    timestamp: Timestamp,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BranchProtectionStatus {
    Pass = 0,
    Partial = 1,
    Fail = 2,
    Unknown = 3,
}
impl_pardosa_enum!(BranchProtectionStatus {
    Pass = 0,
    Partial = 1,
    Fail = 2,
    Unknown = 3
});

#[derive(Debug, Clone, PartialEq, Eq)]
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
impl_pardosa_struct!(BranchProtectionDetails {
    default_branch: NonEmptyEventString<MAX_BRANCH_NAME>,
    has_pr: Option<bool>,
    required_reviewers: Option<u32>,
    has_status_checks: Option<bool>,
    admin_equivalent: Option<bool>,
    has_broad_bypass: Option<bool>,
    reason: Option<EventString<MAX_REASON>>,
    reason_kind: Option<CollectionFailureReason>,
    http_status: Option<u16>,
    force_push_blocked: Option<bool>,
    deletion_blocked: Option<bool>,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CollectionFailureReason {
    PermissionDenied = 0,
    PermissionSuspected = 1,
    NotFoundAbsent = 2,
    Transient = 3,
    RateLimited = 4,
    Invalid = 5,
}
impl_pardosa_enum!(CollectionFailureReason {
    PermissionDenied = 0,
    PermissionSuspected = 1,
    NotFoundAbsent = 2,
    Transient = 3,
    RateLimited = 4,
    Invalid = 5
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeownersResult {
    pub status: CodeownersStatus,
    pub path: Option<EventString<MAX_PATH>>,
    pub timestamp: Timestamp,
    pub parsed: Option<ParsedCodeowners>,
    pub truncation: Option<CodeownersTruncationReason>,
}
impl_pardosa_struct!(CodeownersResult {
    status: CodeownersStatus,
    path: Option<EventString<MAX_PATH>>,
    timestamp: Timestamp,
    parsed: Option<ParsedCodeowners>,
    truncation: Option<CodeownersTruncationReason>,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CodeownersStatus {
    Conforming = 0,
    NonConforming = 1,
    Absent = 2,
    Unknown = 3,
}
impl_pardosa_enum!(CodeownersStatus {
    Conforming = 0,
    NonConforming = 1,
    Absent = 2,
    Unknown = 3
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CodeownersTruncationReason {
    NotBase64Encoded = 0,
    OversizedBase64 = 1,
    ContentMissing = 2,
    DecodeFailed = 3,
    InvalidUtf8 = 4,
}
impl_pardosa_enum!(CodeownersTruncationReason {
    NotBase64Encoded = 0,
    OversizedBase64 = 1,
    ContentMissing = 2,
    DecodeFailed = 3,
    InvalidUtf8 = 4
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCodeowners {
    pub entries: EventVec<CodeownersEntry, MAX_CODEOWNERS_ENTRIES>,
    pub unique_owners: EventVec<EventString<MAX_CODEOWNERS_OWNER>, MAX_CODEOWNERS_OWNERS>,
    pub skipped_lines: u32,
}
impl_pardosa_struct!(ParsedCodeowners {
    entries: EventVec<CodeownersEntry, MAX_CODEOWNERS_ENTRIES>,
    unique_owners: EventVec<EventString<MAX_CODEOWNERS_OWNER>, MAX_CODEOWNERS_OWNERS>,
    skipped_lines: u32,
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeownersEntry {
    pub pattern: EventString<MAX_CODEOWNERS_PATTERN>,
    pub owners: EventVec<EventString<MAX_CODEOWNERS_OWNER>, MAX_CODEOWNERS_OWNERS>,
}
impl_pardosa_struct!(CodeownersEntry {
    pattern: EventString<MAX_CODEOWNERS_PATTERN>,
    owners: EventVec<EventString<MAX_CODEOWNERS_OWNER>, MAX_CODEOWNERS_OWNERS>,
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgStateCaptured {
    pub archived_repos: u32,
    pub assessment_metadata: AssessmentMetadata,
    pub alert_summary: OrgAlertSummary,
}
impl_pardosa_struct!(OrgStateCaptured {
    archived_repos: u32,
    assessment_metadata: AssessmentMetadata,
    alert_summary: OrgAlertSummary,
});

impl PardosaSchema for OrgStateCaptured {
    fn schema_version() -> u32 {
        ORG_STATE_SCHEMA_VERSION
    }
    fn schema_descriptor() -> DescriptorNode {
        Self::descriptor_node()
    }
    fn encode_payload(&self, buf: &mut Vec<u8>) -> Result<(), EncodeError> {
        self.encode_type(buf)
    }
    fn decode_payload(buf: &[u8]) -> Result<Self, DecodeError> {
        let (val, _) = Self::decode_type(buf)?;
        Ok(val)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
impl_pardosa_struct!(AssessmentMetadata {
    date: EventString<MAX_ASSESSMENT_DATE>,
    organization: EventString<MAX_LOGIN>,
    schema_version: EventString<MAX_SCHEMA_VERSION>,
    run_timestamp: EventString<MAX_TIMESTAMP_TEXT>,
    run_id: EventString<MAX_RUN_ID>,
    token_tier: TokenTier,
    token_scopes: EventString<MAX_TOKEN_SCOPES>,
    auth_mode: AuthMode,
    rate_limit_warnings: u32,
    unavailable_capabilities: EventVec<Capability, MAX_UNAVAILABLE_CAPABILITIES>,
    inventory_fetched_at: Option<EventString<MAX_TIMESTAMP_TEXT>>,
    warm_start: bool,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TokenTier {
    Full = 0,
    Limited = 1,
    Unknown = 2,
}
impl_pardosa_enum!(TokenTier {
    Full = 0,
    Limited = 1,
    Unknown = 2
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Capability {
    OrgSecretScanningAlerts = 0,
    PrivateBranchProtectionRead = 1,
}
impl_pardosa_enum!(Capability {
    OrgSecretScanningAlerts = 0,
    PrivateBranchProtectionRead = 1
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum AuthMode {
    Pat = 0,
    GitHubApp = 1,
    GhCliFallback = 2,
    Unknown = 3,
}
impl_pardosa_enum!(AuthMode {
    Pat = 0,
    GitHubApp = 1,
    GhCliFallback = 2,
    Unknown = 3
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgAlertSummary {
    pub collection_status: CollectionStatus,
    pub collection_reason: Option<EventString<MAX_REASON>>,
    pub per_repo: EventVec<RepoAlertSummaryEntry, MAX_ORG_ALERT_REPOS>,
    pub open_secret_alert_age_buckets: EventVec<StringU64Entry, MAX_ALERT_BUCKETS>,
    pub total_open_secret_alerts: u64,
    pub oldest_open_secret_alert_created_at: Option<EventString<MAX_TIMESTAMP_TEXT>>,
    pub newest_open_secret_alert_created_at: Option<EventString<MAX_TIMESTAMP_TEXT>>,
}
impl_pardosa_struct!(OrgAlertSummary {
    collection_status: CollectionStatus,
    collection_reason: Option<EventString<MAX_REASON>>,
    per_repo: EventVec<RepoAlertSummaryEntry, MAX_ORG_ALERT_REPOS>,
    open_secret_alert_age_buckets: EventVec<StringU64Entry, MAX_ALERT_BUCKETS>,
    total_open_secret_alerts: u64,
    oldest_open_secret_alert_created_at: Option<EventString<MAX_TIMESTAMP_TEXT>>,
    newest_open_secret_alert_created_at: Option<EventString<MAX_TIMESTAMP_TEXT>>,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CollectionStatus {
    Success = 0,
    NotCollected = 1,
    PermissionDenied = 2,
    TransientError = 3,
    Unavailable = 4,
}
impl_pardosa_enum!(CollectionStatus {
    Success = 0,
    NotCollected = 1,
    PermissionDenied = 2,
    TransientError = 3,
    Unavailable = 4
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoAlertSummaryEntry {
    pub repository_id: EventString<MAX_GITHUB_ID>,
    pub summary: RepoAlertSummary,
}
impl_pardosa_struct!(RepoAlertSummaryEntry {
    repository_id: EventString<MAX_GITHUB_ID>,
    summary: RepoAlertSummary,
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoAlertSummary {
    pub open_alert_count: u64,
    pub oldest_open_alert_created_at: Option<EventString<MAX_TIMESTAMP_TEXT>>,
    pub newest_open_alert_created_at: Option<EventString<MAX_TIMESTAMP_TEXT>>,
}
impl_pardosa_struct!(RepoAlertSummary {
    open_alert_count: u64,
    oldest_open_alert_created_at: Option<EventString<MAX_TIMESTAMP_TEXT>>,
    newest_open_alert_created_at: Option<EventString<MAX_TIMESTAMP_TEXT>>,
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringU64Entry {
    pub key: EventString<MAX_ALERT_BUCKET>,
    pub value: u64,
}
impl_pardosa_struct!(StringU64Entry {
    key: EventString<MAX_ALERT_BUCKET>,
    value: u64,
});

impl OrgStateCaptured {
    #[must_use]
    pub fn event_type(&self) -> &'static str {
        "OrgStateCaptured"
    }
}

/// Durable per-team roster snapshot (CHE-0089:R1), routed on its own
/// per-team fiber (CHE-0089:R2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamStateCaptured {
    pub org: NonEmptyEventString<MAX_LOGIN>,
    pub team_slug: NonEmptyEventString<MAX_LOGIN>,
    pub members: EventVec<TeamMemberEvent, MAX_TEAM_MEMBERS>,
    pub orphan_attribution_inputs: OrphanAttributionInputs,
    pub fetched_at: EventString<MAX_TIMESTAMP_TEXT>,
    pub status: TeamRosterStatusEvent,
}
impl_pardosa_struct!(TeamStateCaptured {
    org: NonEmptyEventString<MAX_LOGIN>,
    team_slug: NonEmptyEventString<MAX_LOGIN>,
    members: EventVec<TeamMemberEvent, MAX_TEAM_MEMBERS>,
    orphan_attribution_inputs: OrphanAttributionInputs,
    fetched_at: EventString<MAX_TIMESTAMP_TEXT>,
    status: TeamRosterStatusEvent,
});

impl PardosaSchema for TeamStateCaptured {
    fn schema_version() -> u32 {
        TEAM_STATE_SCHEMA_VERSION
    }
    fn schema_descriptor() -> DescriptorNode {
        Self::descriptor_node()
    }
    fn encode_payload(&self, buf: &mut Vec<u8>) -> Result<(), EncodeError> {
        self.encode_type(buf)
    }
    fn decode_payload(buf: &[u8]) -> Result<Self, DecodeError> {
        let (val, _) = Self::decode_type(buf)?;
        Ok(val)
    }
}

/// One team member's durable roster entry, mirroring
/// [`crate::domain::metrics::TeamMember`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamMemberEvent {
    pub login: NonEmptyEventString<MAX_LOGIN>,
    pub role: TeamMemberRoleEvent,
    pub in_org: Option<bool>,
}
impl_pardosa_struct!(TeamMemberEvent {
    login: NonEmptyEventString<MAX_LOGIN>,
    role: TeamMemberRoleEvent,
    in_org: Option<bool>,
});

/// Durable mirror of [`crate::domain::metrics::TeamMemberRole`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TeamMemberRoleEvent {
    Maintainer = 0,
    Member = 1,
    Unknown = 2,
}
impl_pardosa_enum!(TeamMemberRoleEvent {
    Maintainer = 0,
    Member = 1,
    Unknown = 2
});

/// Fetch-completeness status of the underlying team roster fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TeamRosterStatusEvent {
    Complete = 0,
    Deleted = 1,
    PermissionDenied = 2,
    TransientError = 3,
}
impl_pardosa_enum!(TeamRosterStatusEvent {
    Complete = 0,
    Deleted = 1,
    PermissionDenied = 2,
    TransientError = 3
});

/// Durable inputs the render-time orphan-attribution join depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OrphanAttributionInputs {
    pub org_membership_fetch_status: OrgMembershipFetchStatus,
}
impl_pardosa_struct!(OrphanAttributionInputs {
    org_membership_fetch_status: OrgMembershipFetchStatus,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum OrgMembershipFetchStatus {
    Fetched = 0,
    Degraded = 1,
}
impl_pardosa_enum!(OrgMembershipFetchStatus {
    Fetched = 0,
    Degraded = 1
});

impl TeamStateCaptured {
    #[must_use]
    pub fn event_type(&self) -> &'static str {
        "TeamStateCaptured"
    }
}

/// Errors rejected by [`team_domain_key`] (CHE-0089:R2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TeamDomainKeyError {
    #[error("team_domain_key: org must not be empty")]
    EmptyOrg,
    #[error("team_domain_key: team_slug must not be empty")]
    EmptyTeamSlug,
}

/// Derive the NATS-safe, injective `team_domain_key` token for `(org, team_slug)`.
///
/// # Errors
/// Returns [`TeamDomainKeyError::EmptyOrg`] if `org` is empty, or
/// [`TeamDomainKeyError::EmptyTeamSlug`] if `team_slug` is empty.
pub fn team_domain_key(org: &str, team_slug: &str) -> Result<String, TeamDomainKeyError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    if org.is_empty() {
        return Err(TeamDomainKeyError::EmptyOrg);
    }
    if team_slug.is_empty() {
        return Err(TeamDomainKeyError::EmptyTeamSlug);
    }
    let mut joined = Vec::with_capacity(org.len() + 1 + team_slug.len());
    joined.extend_from_slice(org.as_bytes());
    joined.push(0x1F);
    joined.extend_from_slice(team_slug.as_bytes());
    let mut token = String::with_capacity(5 + joined.len() * 2);
    token.push_str("team_");
    for byte in joined {
        token.push(char::from(HEX[usize::from(byte >> 4)]));
        token.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(token)
}

#[expect(
    clippy::large_enum_variant,
    reason = "event enum carries full capture variants"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainEvent {
    RepositoryStateCaptured {
        domain_key: NonEmptyEventString<MAX_DOMAIN_KEY>,
        repo_name: NonEmptyEventString<MAX_REPO_NAME>,
        timestamp: Timestamp,
        evidence: Option<RepositoryEvidence>,
    },
    RepositoryDeleted {
        domain_key: NonEmptyEventString<MAX_DOMAIN_KEY>,
        repo_name: NonEmptyEventString<MAX_REPO_NAME>,
        detected_at: Timestamp,
    },
    OrgStateCaptured(OrgStateCaptured),
    TeamStateCaptured(TeamStateCaptured),
}

impl PardosaType for DomainEvent {
    fn descriptor_node() -> DescriptorNode {
        DescriptorNode::Enum {
            name: "DomainEvent".to_string(),
            discriminant_width: 1,
            variants: vec![
                VariantDescriptor {
                    discriminant: 0,
                    name: "RepositoryStateCaptured".to_string(),
                    payload: Some(DescriptorNode::Struct {
                        name: "DomainEvent_RepositoryStateCaptured".to_string(),
                        fields: vec![
                            FieldDescriptor {
                                name: "domain_key".to_string(),
                                node: <NonEmptyEventString<MAX_DOMAIN_KEY> as PardosaType>::descriptor_node(),
                            },
                            FieldDescriptor {
                                name: "repo_name".to_string(),
                                node: <NonEmptyEventString<MAX_REPO_NAME> as PardosaType>::descriptor_node(),
                            },
                            FieldDescriptor {
                                name: "timestamp".to_string(),
                                node: <Timestamp as PardosaType>::descriptor_node(),
                            },
                            FieldDescriptor {
                                name: "evidence".to_string(),
                                node: <Option<RepositoryEvidence> as PardosaType>::descriptor_node(),
                            },
                        ],
                    }),
                },
                VariantDescriptor {
                    discriminant: 1,
                    name: "RepositoryDeleted".to_string(),
                    payload: Some(DescriptorNode::Struct {
                        name: "DomainEvent_RepositoryDeleted".to_string(),
                        fields: vec![
                            FieldDescriptor {
                                name: "domain_key".to_string(),
                                node: <NonEmptyEventString<MAX_DOMAIN_KEY> as PardosaType>::descriptor_node(),
                            },
                            FieldDescriptor {
                                name: "repo_name".to_string(),
                                node: <NonEmptyEventString<MAX_REPO_NAME> as PardosaType>::descriptor_node(),
                            },
                            FieldDescriptor {
                                name: "detected_at".to_string(),
                                node: <Timestamp as PardosaType>::descriptor_node(),
                            },
                        ],
                    }),
                },
                VariantDescriptor {
                    discriminant: 2,
                    name: "OrgStateCaptured".to_string(),
                    payload: Some(<OrgStateCaptured as PardosaType>::descriptor_node()),
                },
                VariantDescriptor {
                    discriminant: 3,
                    name: "TeamStateCaptured".to_string(),
                    payload: Some(<TeamStateCaptured as PardosaType>::descriptor_node()),
                },
            ],
        }
    }
    fn encode_type(&self, buf: &mut Vec<u8>) -> Result<(), EncodeError> {
        match self {
            Self::RepositoryStateCaptured {
                domain_key,
                repo_name,
                timestamp,
                evidence,
            } => {
                buf.push(0);
                domain_key.encode_type(buf)?;
                repo_name.encode_type(buf)?;
                timestamp.encode_type(buf)?;
                evidence.encode_type(buf)?;
                Ok(())
            }
            Self::RepositoryDeleted {
                domain_key,
                repo_name,
                detected_at,
            } => {
                buf.push(1);
                domain_key.encode_type(buf)?;
                repo_name.encode_type(buf)?;
                detected_at.encode_type(buf)?;
                Ok(())
            }
            Self::OrgStateCaptured(org) => {
                buf.push(2);
                org.encode_type(buf)?;
                Ok(())
            }
            Self::TeamStateCaptured(team) => {
                buf.push(3);
                team.encode_type(buf)?;
                Ok(())
            }
        }
    }
    fn decode_type(buf: &[u8]) -> Result<(Self, usize), DecodeError> {
        if buf.is_empty() {
            return Err(DecodeError::TruncatedPayload {
                expected: 1,
                available: 0,
            });
        }
        let tag = buf[0];
        let mut cursor = 1;
        match tag {
            0 => {
                let (domain_key, c) = NonEmptyEventString::decode_type(&buf[cursor..])?;
                cursor += c;
                let (repo_name, c) = NonEmptyEventString::decode_type(&buf[cursor..])?;
                cursor += c;
                let (timestamp, c) = Timestamp::decode_type(&buf[cursor..])?;
                cursor += c;
                let (evidence, c) = Option::<RepositoryEvidence>::decode_type(&buf[cursor..])?;
                cursor += c;
                Ok((
                    Self::RepositoryStateCaptured {
                        domain_key,
                        repo_name,
                        timestamp,
                        evidence,
                    },
                    cursor,
                ))
            }
            1 => {
                let (domain_key, c) = NonEmptyEventString::decode_type(&buf[cursor..])?;
                cursor += c;
                let (repo_name, c) = NonEmptyEventString::decode_type(&buf[cursor..])?;
                cursor += c;
                let (detected_at, c) = Timestamp::decode_type(&buf[cursor..])?;
                cursor += c;
                Ok((
                    Self::RepositoryDeleted {
                        domain_key,
                        repo_name,
                        detected_at,
                    },
                    cursor,
                ))
            }
            2 => {
                let (org, c) = OrgStateCaptured::decode_type(&buf[cursor..])?;
                cursor += c;
                Ok((Self::OrgStateCaptured(org), cursor))
            }
            3 => {
                let (team, c) = TeamStateCaptured::decode_type(&buf[cursor..])?;
                cursor += c;
                Ok((Self::TeamStateCaptured(team), cursor))
            }
            other => Err(DecodeError::UnknownVariantDiscriminant {
                discriminant: u32::from(other),
            }),
        }
    }
}

impl PardosaSchema for DomainEvent {
    fn schema_version() -> u32 {
        DOMAIN_EVENT_SCHEMA_VERSION
    }
    fn schema_descriptor() -> DescriptorNode {
        Self::descriptor_node()
    }
    fn encode_payload(&self, buf: &mut Vec<u8>) -> Result<(), EncodeError> {
        self.encode_type(buf)
    }
    fn decode_payload(buf: &[u8]) -> Result<Self, DecodeError> {
        let (val, _) = Self::decode_type(buf)?;
        Ok(val)
    }
}

impl DomainEvent {
    #[must_use]
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::RepositoryStateCaptured { .. } => "RepositoryStateCaptured",
            Self::RepositoryDeleted { .. } => "RepositoryDeleted",
            Self::OrgStateCaptured(_) => "OrgStateCaptured",
            Self::TeamStateCaptured(_) => "TeamStateCaptured",
        }
    }
}

#[cfg(test)]
pub(crate) fn to_vec<T: PardosaSchema>(event: &T) -> Vec<u8> {
    let mut buf = Vec::new();
    event.encode_payload(&mut buf).expect("encode payload");
    buf
}

#[cfg(test)]
pub(crate) fn from_bytes<T: PardosaSchema>(buf: &[u8]) -> Result<T, DecodeError> {
    T::decode_payload(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn ts(nanos: u64) -> Timestamp {
        Timestamp::new(nanos).expect("nonzero nanos")
    }

    fn nes<const MAX: usize>(s: &str) -> NonEmptyEventString<MAX> {
        NonEmptyEventString::new(s).expect("fits MAX, nonempty")
    }

    fn es<const MAX: usize>(s: &str) -> EventString<MAX> {
        EventString::new(s).expect("fits MAX")
    }

    fn ev<const MAX: usize>(items: Vec<EventString<MAX>>) -> EventVec<EventString<MAX>, 128> {
        EventVec::new(items).expect("fits MAX")
    }

    fn codeowners_entries(
        items: Vec<CodeownersEntry>,
    ) -> EventVec<CodeownersEntry, MAX_CODEOWNERS_ENTRIES> {
        EventVec::new(items).expect("fits MAX")
    }

    fn owner_vec(
        items: Vec<EventString<MAX_CODEOWNERS_OWNER>>,
    ) -> EventVec<EventString<MAX_CODEOWNERS_OWNER>, MAX_CODEOWNERS_OWNERS> {
        EventVec::new(items).expect("fits MAX")
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
            updated_at: Some(nes("10")),
            has_issues: true,
            pushed_at: Some(ts(11)),
            created_at: Some(ts(12)),
            description: Some(es("repository description")),
            fork: false,
            is_empty: false,
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
        let first = OrgStateCaptured::schema_identity();
        let second = OrgStateCaptured::schema_identity();
        assert_eq!(first, second);
        assert_ne!(
            OrgStateCaptured::schema_identity(),
            DomainEvent::schema_identity()
        );
    }

    fn team_member(
        login: &str,
        role: TeamMemberRoleEvent,
        in_org: Option<bool>,
    ) -> TeamMemberEvent {
        TeamMemberEvent {
            login: nes(login),
            role,
            in_org,
        }
    }

    fn team_members(items: Vec<TeamMemberEvent>) -> EventVec<TeamMemberEvent, MAX_TEAM_MEMBERS> {
        EventVec::new(items).expect("fits MAX")
    }

    fn team_state_captured() -> TeamStateCaptured {
        TeamStateCaptured {
            org: nes("acme"),
            team_slug: nes("platform"),
            members: team_members(vec![
                team_member("alice", TeamMemberRoleEvent::Maintainer, Some(true)),
                team_member("bob", TeamMemberRoleEvent::Member, None),
            ]),
            orphan_attribution_inputs: OrphanAttributionInputs {
                org_membership_fetch_status: OrgMembershipFetchStatus::Fetched,
            },
            fetched_at: es("2026-07-16T00:00:00Z"),
            status: TeamRosterStatusEvent::Complete,
        }
    }

    #[test]
    fn team_state_captured_carries_fetched_at_into_the_read_model_roster() {
        let event = team_state_captured();
        let roster = crate::domain::metrics::TeamRoster::from(event);
        assert_eq!(
            roster.fetched_at.as_deref(),
            Some("2026-07-16T00:00:00Z"),
            "the persisted fetch instant must survive the fold; dropping it is \
             what made the roster's age unobservable at render"
        );
    }

    #[test]
    fn empty_durable_fetched_at_becomes_unknown_not_an_empty_instant() {
        let mut event = team_state_captured();
        event.fetched_at = es("");
        let roster = crate::domain::metrics::TeamRoster::from(event);
        assert_eq!(roster.fetched_at, None);
    }

    #[test]
    fn team_member_role_mapping_is_total_in_both_directions() {
        use crate::domain::metrics::TeamMemberRole as Domain;

        for durable in [
            TeamMemberRoleEvent::Maintainer,
            TeamMemberRoleEvent::Member,
            TeamMemberRoleEvent::Unknown,
        ] {
            let domain = Domain::from(durable);
            let expected = match durable {
                TeamMemberRoleEvent::Maintainer => Domain::Maintainer,
                TeamMemberRoleEvent::Member => Domain::Member,
                TeamMemberRoleEvent::Unknown => Domain::Unknown,
            };
            assert_eq!(domain, expected);
            assert_eq!(
                crate::app::state::team_member_role_event(domain),
                durable,
                "durable -> domain -> durable must be the identity; a variant \
                 that collapses onto another loses the distinction the durable \
                 enum exists to keep"
            );
        }

        for domain in [Domain::Maintainer, Domain::Member, Domain::Unknown] {
            let durable = crate::app::state::team_member_role_event(domain);
            let expected = match domain {
                Domain::Maintainer => TeamMemberRoleEvent::Maintainer,
                Domain::Member => TeamMemberRoleEvent::Member,
                Domain::Unknown => TeamMemberRoleEvent::Unknown,
            };
            assert_eq!(durable, expected);
        }
    }

    #[test]
    fn unknown_role_renders_as_unknown_not_as_member() {
        assert_eq!(
            crate::domain::metrics::TeamMemberRole::Unknown.to_string(),
            "Unknown"
        );
        assert_ne!(
            crate::domain::metrics::TeamMemberRole::Unknown.to_string(),
            crate::domain::metrics::TeamMemberRole::Member.to_string()
        );
    }

    #[test]
    fn native_team_state_round_trips() {
        let event = team_state_captured();
        let wire = to_vec(&event);
        let decoded: TeamStateCaptured = from_bytes(&wire).expect("decode native team event");
        assert_eq!(decoded, event);
        assert_eq!(decoded.event_type(), "TeamStateCaptured");
    }

    #[test]
    fn team_state_schema_identity_is_stable() {
        let first = TeamStateCaptured::schema_identity();
        let second = TeamStateCaptured::schema_identity();
        assert_eq!(first, second);
        assert_ne!(
            TeamStateCaptured::schema_identity(),
            DomainEvent::schema_identity()
        );
        assert_ne!(
            TeamStateCaptured::schema_identity(),
            OrgStateCaptured::schema_identity()
        );
    }

    #[test]
    fn team_domain_key_is_injective_over_org_and_team_slug_pair() {
        let a = team_domain_key("ab", "c").expect("derives");
        let b = team_domain_key("a", "bc").expect("derives");
        assert_ne!(
            a, b,
            "0x1F unit separator must prevent (org,team_slug) collisions"
        );
        assert!(a.starts_with("team_"));
        assert!(b.starts_with("team_"));
    }

    #[test]
    fn team_domain_key_is_deterministic_and_hex_only() {
        let first = team_domain_key("acme", "platform").expect("derives");
        let second = team_domain_key("acme", "platform").expect("derives");
        assert_eq!(first, second);
        assert!(
            first["team_".len()..]
                .chars()
                .all(|c| c.is_ascii_hexdigit()),
            "token body must be lower-hex only (NATS-safe, PGN-0010:R4)"
        );
    }

    #[test]
    fn team_domain_key_rejects_empty_org_or_team_slug() {
        assert_eq!(
            team_domain_key("", "platform"),
            Err(TeamDomainKeyError::EmptyOrg)
        );
        assert_eq!(
            team_domain_key("acme", ""),
            Err(TeamDomainKeyError::EmptyTeamSlug)
        );
    }

    #[test]
    fn schema_hash_is_stable_across_reads() {
        let first = DomainEvent::schema_identity();
        let second = DomainEvent::schema_identity();
        assert_eq!(first, second);
    }

    #[test]
    fn sweep_timeout_event_schema_identity_is_stable() {
        let first = SweepTimeoutEvent::schema_identity();
        let second = SweepTimeoutEvent::schema_identity();
        assert_eq!(first, second);
        assert_ne!(
            SweepTimeoutEvent::schema_identity(),
            DomainEvent::schema_identity()
        );
        assert_eq!(
            SweepTimeoutEvent::target_opened(uuid::Uuid::from_u128(1)).event_type(),
            "gh-report.sweep_timeout_target_opened"
        );
    }

    #[test]
    fn oversized_topic_rejected_at_construction() {
        let too_long = "x".repeat(MAX_TOPIC + 1);
        let err = EventString::<MAX_TOPIC>::new(too_long).expect_err("over-MAX rejects");
        assert!(matches!(err, DecodeError::LengthExceeded { .. }));
    }

    #[test]
    fn updated_at_native_empty_rejected() {
        let err = NonEmptyEventString::<MAX_TIMESTAMP_TEXT>::new("")
            .expect_err("empty native string must be rejected");
        assert!(matches!(err, DecodeError::EmptyNonEmptyString));
    }

    #[test]
    fn schema_structural_completeness_rejects_malformed_descriptor() {
        let malformed = SchemaDescriptor::new(1, DescriptorNode::EventString { max_bytes: 0 });
        assert!(malformed.validate_structural_completeness().is_err());

        let malformed_nes =
            SchemaDescriptor::new(1, DescriptorNode::NonEmptyEventString { max_bytes: 0 });
        assert!(malformed_nes.validate_structural_completeness().is_err());
    }

    const LEGACY_DOMAIN_EVENT_SCHEMA_IDENTITY: &str =
        "3b1d43cb4b22f0e89ebb6e59928c1bdf398cf3d7d904d8f0d767dc682a44e86f";
    const LEGACY_ORG_STATE_SCHEMA_IDENTITY: &str =
        "09ef6a4050c43a6f5d835cfcd7facc1035bf663f2346a1d86037b6da2b8d3153";
    const LEGACY_TEAM_STATE_SCHEMA_IDENTITY: &str =
        "d98958e86e928d94bd3cda3bb81d4e9e2b23d48f867b3383552caff6a3dc1300";
    const LEGACY_SWEEP_TIMEOUT_SCHEMA_IDENTITY: &str =
        "cc4812aa267f39c6d430fc32d7dacf8e6af78595e178569bf79ea14364f846d5";

    const TRUTHFUL_DOMAIN_EVENT_SCHEMA_IDENTITY: &str =
        "c41b252a6cff87dadf9df198575ad5af88559f1131bb8aee8b20a12067352458";
    const TRUTHFUL_ORG_STATE_SCHEMA_IDENTITY: &str =
        "2ec6b5d4f386afbe6a73fe4e0c962897e477316af3137591927bb18a31b4c38f";
    const TRUTHFUL_TEAM_STATE_SCHEMA_IDENTITY: &str =
        "fa78ebd335983b4db1d2cf12f0fd21deed7744dc1d9d1668aa6cc9721fc42321";
    const TRUTHFUL_SWEEP_TIMEOUT_SCHEMA_IDENTITY: &str =
        "55b9b99b6408ad5696d2e0ce5cc85c0f28ffd93c1f7ab230d524ae4d329ff44e";

    const CANONICAL_REPO_CAPTURED_BYTES: [u8; 464] = [
        0, 9, 0, 0, 0, 105, 100, 45, 114, 101, 112, 111, 45, 49, 6, 0, 0, 0, 114, 101, 112, 111,
        45, 49, 40, 0, 0, 0, 0, 0, 0, 0, 1, 9, 0, 0, 0, 105, 100, 45, 114, 101, 112, 111, 45, 49,
        1, 6, 0, 0, 0, 110, 111, 100, 101, 45, 49, 6, 0, 0, 0, 114, 101, 112, 111, 45, 49, 0, 1, 4,
        0, 0, 0, 82, 117, 115, 116, 4, 0, 0, 0, 109, 97, 105, 110, 0, 9, 0, 0, 0, 105, 100, 45,
        114, 101, 112, 111, 45, 49, 1, 2, 0, 0, 0, 49, 48, 1, 1, 11, 0, 0, 0, 0, 0, 0, 0, 1, 12, 0,
        0, 0, 0, 0, 0, 0, 1, 22, 0, 0, 0, 114, 101, 112, 111, 115, 105, 116, 111, 114, 121, 32,
        100, 101, 115, 99, 114, 105, 112, 116, 105, 111, 110, 0, 0, 1, 30, 0, 0, 0, 104, 116, 116,
        112, 115, 58, 47, 47, 103, 105, 116, 104, 117, 98, 46, 99, 111, 109, 47, 97, 99, 109, 101,
        47, 114, 101, 112, 111, 45, 49, 2, 0, 0, 0, 8, 0, 0, 0, 115, 101, 99, 117, 114, 105, 116,
        121, 4, 0, 0, 0, 114, 117, 115, 116, 1, 3, 0, 0, 0, 77, 73, 84, 0, 0, 1, 11, 0, 0, 0, 83,
        69, 67, 85, 82, 73, 84, 89, 46, 109, 100, 20, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 1, 7, 0, 0,
        0, 101, 110, 97, 98, 108, 101, 100, 21, 0, 0, 0, 0, 0, 0, 0, 0, 1, 7, 0, 0, 0, 101, 110,
        97, 98, 108, 101, 100, 22, 0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 109, 97, 105, 110, 1, 1, 1,
        2, 0, 0, 0, 1, 1, 1, 1, 1, 0, 0, 0, 0, 1, 1, 1, 1, 23, 0, 0, 0, 0, 0, 0, 0, 0, 1, 18, 0, 0,
        0, 46, 103, 105, 116, 104, 117, 98, 47, 67, 79, 68, 69, 79, 87, 78, 69, 82, 83, 24, 0, 0,
        0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 5, 0, 0, 0, 47, 115, 114, 99, 47, 1, 0, 0, 0, 14, 0, 0, 0,
        64, 97, 99, 109, 101, 47, 115, 101, 99, 117, 114, 105, 116, 121, 1, 0, 0, 0, 14, 0, 0, 0,
        64, 97, 99, 109, 101, 47, 115, 101, 99, 117, 114, 105, 116, 121, 0, 0, 0, 0, 1, 1, 1, 1, 7,
        0, 0, 0, 111, 99, 116, 111, 99, 97, 116, 1, 12, 0, 0, 0, 77, 111, 110, 97, 32, 79, 99, 116,
        111, 99, 97, 116, 1, 30, 0, 0, 0, 0, 0, 0, 0,
    ];
    const CANONICAL_REPO_DELETED_BYTES: [u8; 32] = [
        1, 9, 0, 0, 0, 105, 100, 45, 114, 101, 112, 111, 45, 49, 6, 0, 0, 0, 114, 101, 112, 111,
        45, 49, 50, 0, 0, 0, 0, 0, 0, 0,
    ];
    const CANONICAL_ORG_CAPTURED_BYTES: [u8; 328] = [
        2, 0, 0, 0, 10, 0, 0, 0, 50, 48, 50, 54, 45, 48, 54, 45, 49, 52, 4, 0, 0, 0, 97, 99, 109,
        101, 3, 0, 0, 0, 49, 46, 48, 20, 0, 0, 0, 50, 48, 50, 54, 45, 48, 54, 45, 49, 52, 84, 49,
        50, 58, 48, 48, 58, 48, 48, 90, 7, 0, 0, 0, 114, 117, 110, 45, 49, 50, 51, 0, 29, 0, 0, 0,
        114, 101, 112, 111, 44, 114, 101, 97, 100, 58, 111, 114, 103, 44, 115, 101, 99, 117, 114,
        105, 116, 121, 95, 101, 118, 101, 110, 116, 115, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 1, 20, 0, 0,
        0, 50, 48, 50, 54, 45, 48, 54, 45, 49, 52, 84, 49, 50, 58, 48, 49, 58, 48, 48, 90, 1, 0, 1,
        9, 0, 0, 0, 99, 111, 108, 108, 101, 99, 116, 101, 100, 1, 0, 0, 0, 6, 0, 0, 0, 114, 101,
        112, 111, 45, 49, 7, 0, 0, 0, 0, 0, 0, 0, 1, 20, 0, 0, 0, 50, 48, 50, 54, 45, 48, 54, 45,
        49, 51, 84, 48, 56, 58, 48, 48, 58, 48, 48, 90, 1, 20, 0, 0, 0, 50, 48, 50, 54, 45, 48, 54,
        45, 49, 52, 84, 48, 56, 58, 48, 48, 58, 48, 48, 90, 2, 0, 0, 0, 8, 0, 0, 0, 48, 95, 55, 95,
        100, 97, 121, 115, 3, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 56, 95, 51, 48, 95, 100, 97, 121,
        115, 4, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 1, 20, 0, 0, 0, 50, 48, 50, 54, 45,
        48, 54, 45, 49, 51, 84, 48, 56, 58, 48, 48, 58, 48, 48, 90, 1, 20, 0, 0, 0, 50, 48, 50, 54,
        45, 48, 54, 45, 49, 52, 84, 48, 56, 58, 48, 48, 58, 48, 48, 90,
    ];
    const CANONICAL_TEAM_CAPTURED_BYTES: [u8; 71] = [
        4, 0, 0, 0, 97, 99, 109, 101, 8, 0, 0, 0, 112, 108, 97, 116, 102, 111, 114, 109, 2, 0, 0,
        0, 5, 0, 0, 0, 97, 108, 105, 99, 101, 0, 1, 1, 3, 0, 0, 0, 98, 111, 98, 1, 0, 0, 20, 0, 0,
        0, 50, 48, 50, 54, 45, 48, 55, 45, 49, 54, 84, 48, 48, 58, 48, 48, 58, 48, 48, 90, 0,
    ];
    const CANONICAL_TIMEOUT_OPENED_BYTES: [u8; 17] =
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 100];
    const CANONICAL_TIMEOUT_FIRED_BYTES: [u8; 45] = [
        1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 200, 5, 0, 0, 0, 114, 117, 110, 45, 49, 7,
        0, 0, 0, 116, 105, 109, 101, 111, 117, 116, 136, 19, 0, 0, 0, 0, 0, 0,
    ];

    fn assert_admitted_and_identity<T: PardosaSchema>(
        expected_truthful_hex: &str,
        expected_legacy_hex: &str,
    ) {
        let identity_hex = T::schema_identity().to_hex();
        assert_eq!(identity_hex, expected_truthful_hex);
        assert_ne!(identity_hex, expected_legacy_hex);

        let desc = SchemaDescriptor::new(T::schema_version(), T::schema_descriptor());
        desc.validate_structural_completeness()
            .expect("schema descriptor must be structurally complete");
        assert_eq!(desc.identity().to_hex(), expected_truthful_hex);
    }

    #[test]
    fn test_schema_legacy_baseline_and_wire_bytes() {
        let repo_captured = DomainEvent::RepositoryStateCaptured {
            domain_key: nes("id-repo-1"),
            repo_name: nes("repo-1"),
            timestamp: ts(40),
            evidence: Some(full_evidence()),
        };
        let repo_deleted = DomainEvent::RepositoryDeleted {
            domain_key: nes("id-repo-1"),
            repo_name: nes("repo-1"),
            detected_at: ts(50),
        };
        let org_captured = OrgStateCaptured::try_from(domain_org_snapshot()).expect("org snapshot");
        let team_captured = team_state_captured();
        let timeout_opened = SweepTimeoutEvent::TargetOpened {
            event_id: uuid::Uuid::from_u128(100),
        };
        let timeout_fired = SweepTimeoutEvent::TimeoutFired {
            event_id: uuid::Uuid::from_u128(200),
            run_id: es("run-1"),
            error: es("timeout"),
            elapsed_ms: 5000,
        };

        let repo_captured_bytes = to_vec(&repo_captured);
        let repo_deleted_bytes = to_vec(&repo_deleted);
        let org_captured_bytes = to_vec(&org_captured);
        let team_captured_bytes = to_vec(&team_captured);
        let timeout_opened_bytes = to_vec(&timeout_opened);
        let timeout_fired_bytes = to_vec(&timeout_fired);

        assert_eq!(repo_captured_bytes, CANONICAL_REPO_CAPTURED_BYTES);
        assert_eq!(repo_deleted_bytes, CANONICAL_REPO_DELETED_BYTES);
        assert_eq!(org_captured_bytes, CANONICAL_ORG_CAPTURED_BYTES);
        assert_eq!(team_captured_bytes, CANONICAL_TEAM_CAPTURED_BYTES);
        assert_eq!(timeout_opened_bytes, CANONICAL_TIMEOUT_OPENED_BYTES);
        assert_eq!(timeout_fired_bytes, CANONICAL_TIMEOUT_FIRED_BYTES);

        let decoded_repo: DomainEvent = from_bytes(&repo_captured_bytes).expect("decode repo");
        assert_eq!(decoded_repo, repo_captured);

        let decoded_deleted: DomainEvent = from_bytes(&repo_deleted_bytes).expect("decode deleted");
        assert_eq!(decoded_deleted, repo_deleted);

        let decoded_org: OrgStateCaptured = from_bytes(&org_captured_bytes).expect("decode org");
        assert_eq!(decoded_org, org_captured);

        let decoded_team: TeamStateCaptured =
            from_bytes(&team_captured_bytes).expect("decode team");
        assert_eq!(decoded_team, team_captured);

        let decoded_opened: SweepTimeoutEvent =
            from_bytes(&timeout_opened_bytes).expect("decode opened");
        assert_eq!(decoded_opened, timeout_opened);

        let decoded_fired: SweepTimeoutEvent =
            from_bytes(&timeout_fired_bytes).expect("decode fired");
        assert_eq!(decoded_fired, timeout_fired);

        assert_admitted_and_identity::<DomainEvent>(
            TRUTHFUL_DOMAIN_EVENT_SCHEMA_IDENTITY,
            LEGACY_DOMAIN_EVENT_SCHEMA_IDENTITY,
        );
        assert_admitted_and_identity::<OrgStateCaptured>(
            TRUTHFUL_ORG_STATE_SCHEMA_IDENTITY,
            LEGACY_ORG_STATE_SCHEMA_IDENTITY,
        );
        assert_admitted_and_identity::<TeamStateCaptured>(
            TRUTHFUL_TEAM_STATE_SCHEMA_IDENTITY,
            LEGACY_TEAM_STATE_SCHEMA_IDENTITY,
        );
        assert_admitted_and_identity::<SweepTimeoutEvent>(
            TRUTHFUL_SWEEP_TIMEOUT_SCHEMA_IDENTITY,
            LEGACY_SWEEP_TIMEOUT_SCHEMA_IDENTITY,
        );
    }
}
