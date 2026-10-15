//! Security check result types.
//!
//! These are the internal domain representations of per-repository check outcomes.
//! They use strongly typed enums rather than free-form strings.

use serde::{Deserialize, Serialize};

use crate::domain::codeowners::ParsedCodeowners;

/// Aggregated per-repository security check results.
///
/// # Wire format
///
/// Fields encode in declaration order via `Encode::encode`: `security_policy`,
/// `secret_scanning`, `dependabot_security_updates`, `branch_protection`,
/// `codeowners`. Field reorder is a wire-format break (CHE-0022:R3 +
/// PGN-0003 + PGN-0013:R8); new fields must append.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryChecks {
    /// Security policy check result.
    pub security_policy: SecurityPolicyResult,
    /// Secret scanning check result.
    pub secret_scanning: SecretScanningResult,
    /// Dependabot security updates check result.
    pub dependabot_security_updates: DependabotResult,
    /// Branch protection check result.
    pub branch_protection: BranchProtectionResult,
    /// CODEOWNERS check result.
    pub codeowners: CodeownersResult,
}

impl RepositoryChecks {
    /// The timestamp this evidence was actually collected — every check
    /// result within one collection pass is stamped from the same
    /// `run_timestamp` (see `collector::branch_protection`,
    /// `collector::security_policy`, `collector::dependabot`,
    /// `collector::codeowners`, and `app::collect::failure_evidence_with_reason`),
    /// so any one field is representative; `branch_protection` is used as
    /// the anchor via `BranchProtectionResult::timestamp`.
    ///
    /// This is the correct basis for cache-reuse age (adr-fmt-glprg):
    /// GitHub's `Repository.updated_at` records the last repository
    /// change, not when we observed it, and does not move when
    /// branch-protection rules change.
    #[must_use]
    pub fn observed_at(&self) -> &str {
        self.branch_protection.timestamp()
    }
}

/// Nonempty path observed at the security-policy boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SecurityPolicyPath(String);

impl SecurityPolicyPath {
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Option<Self> {
        let raw = raw.into();
        (!raw.is_empty()).then_some(Self(raw))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SecurityPolicyPath {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value).ok_or("security policy path must not be empty")
    }
}

impl From<SecurityPolicyPath> for String {
    fn from(value: SecurityPolicyPath) -> Self {
        value.0
    }
}

/// Collection failures that cannot establish absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndeterminateReason {
    PermissionDenied,
    PermissionSuspected,
    Transient,
    RateLimited,
    Invalid,
    Pending,
}

/// Security policy evaluation outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityPolicyResult {
    EnabledBySetting {
        timestamp: String,
    },
    EnabledByFile {
        path: SecurityPolicyPath,
        timestamp: String,
    },
    Absent {
        timestamp: String,
    },
    Unobservable {
        reason: IndeterminateReason,
        timestamp: String,
    },
    NotApplicable {
        timestamp: String,
    },
}

impl SecurityPolicyResult {
    #[must_use]
    pub fn status(&self) -> SecurityPolicyStatus {
        match self {
            Self::EnabledBySetting { .. } | Self::EnabledByFile { .. } => {
                SecurityPolicyStatus::Pass
            }
            Self::Absent { .. } => SecurityPolicyStatus::Fail,
            Self::Unobservable { .. } => SecurityPolicyStatus::Unknown,
            Self::NotApplicable { .. } => SecurityPolicyStatus::NotApplicable,
        }
    }

    #[must_use]
    pub fn evidence(&self) -> SecurityPolicyEvidence {
        match self {
            Self::EnabledBySetting { .. } => SecurityPolicyEvidence::Setting,
            Self::EnabledByFile { .. } => SecurityPolicyEvidence::File,
            Self::Absent { .. } => SecurityPolicyEvidence::Absent,
            Self::NotApplicable { .. } => SecurityPolicyEvidence::NotApplicable,
            Self::Unobservable { reason, .. } => match reason {
                IndeterminateReason::PermissionDenied
                | IndeterminateReason::PermissionSuspected => {
                    SecurityPolicyEvidence::PermissionDenied
                }
                IndeterminateReason::Transient => SecurityPolicyEvidence::TransientError,
                IndeterminateReason::RateLimited
                | IndeterminateReason::Invalid
                | IndeterminateReason::Pending => SecurityPolicyEvidence::CollectionError,
            },
        }
    }

    #[must_use]
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::EnabledByFile { path, .. } => Some(path.as_str()),
            Self::EnabledBySetting { .. }
            | Self::Absent { .. }
            | Self::Unobservable { .. }
            | Self::NotApplicable { .. } => None,
        }
    }

    #[must_use]
    pub fn timestamp(&self) -> &str {
        match self {
            Self::EnabledBySetting { timestamp }
            | Self::EnabledByFile { timestamp, .. }
            | Self::Absent { timestamp }
            | Self::Unobservable { timestamp, .. }
            | Self::NotApplicable { timestamp } => timestamp,
        }
    }
}

impl From<&SecurityPolicyResult> for ScoreCategory {
    fn from(result: &SecurityPolicyResult) -> Self {
        match result {
            SecurityPolicyResult::Unobservable { reason, .. } => Self::Excluded(match reason {
                IndeterminateReason::PermissionDenied
                | IndeterminateReason::PermissionSuspected => ExclusionReason::PermissionDenied,
                IndeterminateReason::Transient
                | IndeterminateReason::RateLimited
                | IndeterminateReason::Invalid
                | IndeterminateReason::Pending => ExclusionReason::Other,
            }),
            SecurityPolicyResult::EnabledBySetting { .. }
            | SecurityPolicyResult::EnabledByFile { .. }
            | SecurityPolicyResult::Absent { .. }
            | SecurityPolicyResult::NotApplicable { .. } => Self::from(result.status()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum SecurityPolicyStatus {
    Pass = 0,
    Fail = 1,
    Unknown = 2,
    NotApplicable = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum SecurityPolicyEvidence {
    /// Detected via the GitHub API `is_security_policy_enabled` setting.
    Setting = 0,
    /// Detected via file presence (e.g., `SECURITY.md`).
    File = 1,
    /// No evidence of a security policy.
    #[serde(alias = "none")]
    Absent = 2,
    /// API returned a permission error; status could not be determined.
    PermissionDenied = 3,
    /// API returned a transient error; status may succeed on retry.
    TransientError = 4,
    /// An unexpected error occurred during collection.
    CollectionError = 5,
    /// Security policy evaluation is not applicable (non-public repository).
    NotApplicable = 6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretScanningFailureReason {
    PermissionDenied,
    PermissionSuspected,
    RateLimited,
    Transient,
    Unavailable,
    InsufficientEvidence,
    Conflict,
    Invalid,
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeSource {
    OrgSummary,
    PerRepoEndpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataUnavailable {
    Missing {
        http_status: Option<u16>,
    },
    Malformed {
        http_status: Option<u16>,
    },
    Failure {
        reason: SecretScanningFailureReason,
        http_status: Option<u16>,
    },
}

impl MetadataUnavailable {
    #[must_use]
    pub fn failure_reason(self) -> SecretScanningFailureReason {
        match self {
            Self::Missing { .. } => SecretScanningFailureReason::InsufficientEvidence,
            Self::Malformed { .. } => SecretScanningFailureReason::Invalid,
            Self::Failure { reason, .. } => reason,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretScanningAlerts {
    Observable {
        source: ProbeSource,
        has_open_alerts: bool,
        http_status: Option<u16>,
    },
    Unobservable {
        source: ProbeSource,
        reason: SecretScanningFailureReason,
        http_status: Option<u16>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnabledProvenance {
    Metadata {
        http_status: Option<u16>,
        alerts: SecretScanningAlerts,
    },
    Fallback {
        metadata: MetadataUnavailable,
        has_open_alerts: bool,
        http_status: Option<u16>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisabledObservation {
    NoMismatch {
        source: ProbeSource,
        http_status: Option<u16>,
    },
    StatusMismatch {
        source: ProbeSource,
        http_status: Option<u16>,
    },
    ProbeFailed {
        source: ProbeSource,
        reason: SecretScanningFailureReason,
        http_status: Option<u16>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnobservableProbe {
    Failed {
        source: ProbeSource,
        reason: SecretScanningFailureReason,
        http_status: Option<u16>,
    },
    OrgSummaryObserved {
        has_open_alerts: bool,
        http_status: Option<u16>,
    },
}

/// Secret scanning evaluation with contemporaneous metadata and probe facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretScanningResult {
    Enabled {
        provenance: EnabledProvenance,
        timestamp: String,
    },
    Disabled {
        metadata_http_status: Option<u16>,
        observation: DisabledObservation,
        timestamp: String,
    },
    Unobservable {
        metadata: MetadataUnavailable,
        probe: UnobservableProbe,
        timestamp: String,
    },
}

impl SecretScanningResult {
    #[must_use]
    pub fn status(&self) -> SecretScanningStatus {
        use SecretScanningFailureReason::{PermissionDenied, PermissionSuspected};
        match self {
            Self::Enabled { .. } => SecretScanningStatus::Enabled,
            Self::Disabled { .. } => SecretScanningStatus::Disabled,
            Self::Unobservable {
                metadata:
                    MetadataUnavailable::Failure {
                        reason: PermissionDenied | PermissionSuspected,
                        ..
                    },
                ..
            }
            | Self::Unobservable {
                probe:
                    UnobservableProbe::Failed {
                        reason: PermissionDenied | PermissionSuspected,
                        ..
                    },
                ..
            } => SecretScanningStatus::PermissionDenied,
            Self::Unobservable { .. } => SecretScanningStatus::Unknown,
        }
    }

    #[must_use]
    pub fn timestamp(&self) -> &str {
        match self {
            Self::Enabled { timestamp, .. }
            | Self::Disabled { timestamp, .. }
            | Self::Unobservable { timestamp, .. } => timestamp,
        }
    }

    #[must_use]
    pub fn with_timestamp(mut self, value: impl Into<String>) -> Self {
        let timestamp = match &mut self {
            Self::Enabled { timestamp, .. }
            | Self::Disabled { timestamp, .. }
            | Self::Unobservable { timestamp, .. } => timestamp,
        };
        *timestamp = value.into();
        self
    }

    #[must_use]
    pub fn has_open_alerts(&self) -> Option<bool> {
        match self {
            Self::Enabled {
                provenance:
                    EnabledProvenance::Metadata {
                        alerts:
                            SecretScanningAlerts::Observable {
                                has_open_alerts, ..
                            },
                        ..
                    },
                ..
            }
            | Self::Enabled {
                provenance:
                    EnabledProvenance::Fallback {
                        has_open_alerts, ..
                    },
                ..
            } => Some(*has_open_alerts),
            Self::Enabled { .. } | Self::Disabled { .. } | Self::Unobservable { .. } => None,
        }
    }

    #[must_use]
    pub fn alerts_observable(&self) -> bool {
        self.has_open_alerts().is_some()
    }

    #[must_use]
    pub fn status_mismatch(&self) -> bool {
        matches!(
            self,
            Self::Disabled {
                observation: DisabledObservation::StatusMismatch { .. },
                ..
            }
        )
    }

    #[must_use]
    pub fn failure_reason(&self) -> Option<SecretScanningFailureReason> {
        match self {
            Self::Enabled {
                provenance:
                    EnabledProvenance::Metadata {
                        alerts: SecretScanningAlerts::Unobservable { reason, .. },
                        ..
                    },
                ..
            }
            | Self::Disabled {
                observation: DisabledObservation::ProbeFailed { reason, .. },
                ..
            }
            | Self::Unobservable {
                probe: UnobservableProbe::Failed { reason, .. },
                ..
            } => Some(*reason),
            Self::Unobservable { metadata, .. } => Some(metadata.failure_reason()),
            Self::Enabled { .. } | Self::Disabled { .. } => None,
        }
    }

    #[must_use]
    pub fn reason(&self) -> Option<&'static str> {
        match (self.status_mismatch(), self.failure_reason()) {
            (true, _) => Some("status_mismatch"),
            (
                _,
                Some(
                    SecretScanningFailureReason::PermissionDenied
                    | SecretScanningFailureReason::PermissionSuspected,
                ),
            ) => Some("permission_denied"),
            (_, Some(SecretScanningFailureReason::RateLimited)) => Some("rate_limited"),
            (_, Some(SecretScanningFailureReason::Transient)) => Some("transient_error"),
            (_, Some(SecretScanningFailureReason::Unavailable)) => Some("alerts_unavailable"),
            (_, Some(SecretScanningFailureReason::InsufficientEvidence)) => {
                Some("insufficient_evidence")
            }
            (_, Some(SecretScanningFailureReason::Conflict)) => Some("conflict"),
            (_, Some(SecretScanningFailureReason::Invalid)) => Some("collection_error"),
            (_, Some(SecretScanningFailureReason::Pending)) => Some("pending"),
            (_, None) => None,
        }
    }

    #[must_use]
    pub fn unobservable(reason: SecretScanningFailureReason, timestamp: impl Into<String>) -> Self {
        Self::Unobservable {
            metadata: MetadataUnavailable::Failure {
                reason,
                http_status: None,
            },
            probe: UnobservableProbe::Failed {
                source: ProbeSource::PerRepoEndpoint,
                reason,
                http_status: None,
            },
            timestamp: timestamp.into(),
        }
    }
}

impl From<&SecretScanningResult> for ScoreCategory {
    fn from(value: &SecretScanningResult) -> Self {
        match value.status() {
            SecretScanningStatus::Enabled => Self::Pass,
            SecretScanningStatus::Disabled => Self::Fail,
            SecretScanningStatus::PermissionDenied => {
                Self::Excluded(ExclusionReason::PermissionDenied)
            }
            SecretScanningStatus::Unknown => Self::Excluded(match value.failure_reason() {
                Some(
                    SecretScanningFailureReason::RateLimited
                    | SecretScanningFailureReason::Transient
                    | SecretScanningFailureReason::Invalid
                    | SecretScanningFailureReason::Pending,
                ) => ExclusionReason::Other,
                _ => ExclusionReason::Unknown,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum SecretScanningStatus {
    Enabled = 0,
    Disabled = 1,
    PermissionDenied = 2,
    Unknown = 3,
}

impl std::fmt::Display for SecretScanningStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Enabled => write!(f, "enabled"),
            Self::Disabled => write!(f, "disabled"),
            Self::PermissionDenied => write!(f, "permission_denied"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Dependabot security updates evaluation outcome.
///
/// # Wire format
///
/// Fields encode in declaration order via `Encode::encode`: `status`, `reason`,
/// `timestamp`. Field reorder is a wire-format break (CHE-0022:R3 + PGN-0003 +
/// PGN-0013:R8); new fields must append.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependabotResult {
    /// Whether Dependabot security updates are enabled on the repository.
    pub status: DependabotStatus,
    /// Human-readable reason for the current status.
    pub reason: Option<String>,
    /// ISO 8601 timestamp of when the check was performed.
    pub timestamp: String,
}

/// Dependabot security updates status.
///
/// # Wire format
///
/// Variant discriminant is `u8` of declaration position (`Enabled=0`,
/// `Paused=1`, `Disabled=2`, `Unknown=3`). Reorder or insert is a wire-format
/// break (CHE-0022:R3 + PGN-0003 + PGN-0013:R8); new variants must append.
///
/// ```
/// use gh_report::domain::checks::DependabotStatus;
/// assert_eq!(DependabotStatus::Paused as u8, 1);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum DependabotStatus {
    Enabled = 0,
    Paused = 1,
    Disabled = 2,
    Unknown = 3,
}

impl std::fmt::Display for DependabotStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Enabled => write!(f, "enabled"),
            Self::Paused => write!(f, "paused"),
            Self::Disabled => write!(f, "disabled"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// History-protection gap observed on a readable default branch.
///
/// `(force_push_blocked = true, deletion_blocked = true)` is
/// unrepresentable here — that combination is
/// [`BranchProtectionResult::Protected`] (CHE-0002); these variants only
/// admit partial or absent integrity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryProtectionGap {
    /// No branch-protection controls were observed at all.
    Absent {
        /// Whether absence was confirmed by an authority signal
        /// (admin-scoped 404) rather than merely not found.
        confirmed_not_found: bool,
        /// HTTP status that produced the absence evidence, if any.
        http_status: Option<u16>,
    },
    /// Both force pushes and branch deletion are not blocked.
    NeitherBlocked,
    /// Force pushes are not blocked while branch deletion is.
    ForcePushAllowed,
    /// Branch deletion is not blocked while force pushes are.
    DeletionAllowed,
}

impl HistoryProtectionGap {
    /// Whether the gap is a confirmed absence of branch protection.
    #[must_use]
    pub const fn is_absent(&self) -> bool {
        matches!(self, Self::Absent { .. })
    }

    /// HTTP status that produced the absence evidence, if any.
    #[must_use]
    pub const fn http_status(&self) -> Option<u16> {
        match self {
            Self::Absent { http_status, .. } => *http_status,
            Self::NeitherBlocked | Self::ForcePushAllowed | Self::DeletionAllowed => None,
        }
    }
}

/// Branch protection evaluation outcome on the default branch.
///
/// A closed algebraic sum (CHE-0002): every outcome is exactly one of a
/// protected branch, an unprotected branch carrying a
/// [`HistoryProtectionGap`], or an unobservable failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchProtectionResult {
    /// Force pushes and branch deletion are both blocked.
    Protected {
        /// Name of the repository's default branch.
        default_branch: String,
        /// ISO 8601 timestamp of when the check was performed.
        timestamp: String,
    },
    /// The default branch is readable but history protection is not held.
    Unprotected {
        /// Name of the repository's default branch.
        default_branch: String,
        /// The concrete history-protection gap observed.
        gap: HistoryProtectionGap,
        /// ISO 8601 timestamp of when the check was performed.
        timestamp: String,
    },
    /// The default branch could not be read for a collection-health reason.
    Unobservable {
        /// Name of the repository's default branch.
        default_branch: String,
        /// Typed collection-health reason; never a compliance verdict.
        reason: IndeterminateReason,
        /// HTTP status that produced the failure, if any.
        http_status: Option<u16>,
        /// ISO 8601 timestamp of when the check was performed.
        timestamp: String,
    },
}

/// Typed collection-health reason for per-check observability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum CollectionFailureReason {
    /// API denied access with an explicit permission response.
    PermissionDenied = 0,
    /// GitHub returned a not-found response that is ambiguous for non-public repositories.
    PermissionSuspected = 1,
    /// The requested resource is absent independently of credentials.
    NotFoundAbsent = 2,
    /// The API response was retryable and may succeed later.
    Transient = 3,
    /// The API response was rate-limited.
    RateLimited = 4,
    /// Input or response shape was invalid for this check.
    Invalid = 5,
}

impl std::fmt::Display for CollectionFailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionDenied => write!(f, "permission_denied"),
            Self::PermissionSuspected => write!(f, "permission_suspected"),
            Self::NotFoundAbsent => write!(f, "not_found_absent"),
            Self::Transient => write!(f, "transient"),
            Self::RateLimited => write!(f, "rate_limited"),
            Self::Invalid => write!(f, "invalid"),
        }
    }
}

/// History-protection signal pair merged across branch-protection sources.
///
/// A private intermediate of the collector eval path; force-push and deletion
/// blocking are the only signals governance evaluates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistorySignals {
    /// Whether force pushes are blocked on the default branch, when observed.
    pub force_push_blocked: Option<bool>,
    /// Whether branch deletion is blocked on the default branch, when observed.
    pub deletion_blocked: Option<bool>,
}

impl HistorySignals {
    /// Merge multiple source signals, taking any block as definitive and any
    /// observed unblock before an unobserved signal.
    #[must_use]
    pub fn merge(signals: &[HistorySignals]) -> Option<HistorySignals> {
        if signals.is_empty() {
            return None;
        }
        Some(HistorySignals {
            force_push_blocked: merge_optional_blocking_signal(signals, |s| s.force_push_blocked),
            deletion_blocked: merge_optional_blocking_signal(signals, |s| s.deletion_blocked),
        })
    }
}

fn merge_optional_blocking_signal(
    signals: &[HistorySignals],
    select: fn(&HistorySignals) -> Option<bool>,
) -> Option<bool> {
    let mut saw_unblocked = false;
    for signal in signals {
        match select(signal) {
            Some(true) => return Some(true),
            Some(false) => saw_unblocked = true,
            None => {}
        }
    }
    if saw_unblocked { Some(false) } else { None }
}

impl BranchProtectionResult {
    /// ISO 8601 timestamp of when the check was performed.
    #[must_use]
    pub fn timestamp(&self) -> &str {
        match self {
            Self::Protected { timestamp, .. }
            | Self::Unprotected { timestamp, .. }
            | Self::Unobservable { timestamp, .. } => timestamp,
        }
    }

    /// Name of the repository's default branch.
    #[must_use]
    pub fn default_branch(&self) -> &str {
        match self {
            Self::Protected { default_branch, .. }
            | Self::Unprotected { default_branch, .. }
            | Self::Unobservable { default_branch, .. } => default_branch,
        }
    }

    /// Whether history protection is fully held (force-push and deletion both
    /// blocked).
    #[must_use]
    pub fn is_protected(&self) -> bool {
        matches!(self, Self::Protected { .. })
    }

    /// Whether this is a definite observation rather than an
    /// [`IndeterminateReason`]-carrying unmeasured outcome.
    #[must_use]
    pub fn is_observed(&self) -> bool {
        !matches!(self, Self::Unobservable { .. })
    }

    /// Whether collection is pending (never observed).
    #[must_use]
    pub fn is_pending(&self) -> bool {
        matches!(
            self,
            Self::Unobservable {
                reason: IndeterminateReason::Pending,
                ..
            }
        )
    }

    /// The history-protection gap when the result is [`Self::Unprotected`].
    #[must_use]
    pub fn gap(&self) -> Option<HistoryProtectionGap> {
        match self {
            Self::Unprotected { gap, .. } => Some(*gap),
            Self::Protected { .. } | Self::Unobservable { .. } => None,
        }
    }

    /// HTTP status that produced the outcome, when the evidence carried one.
    #[must_use]
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::Unobservable { http_status, .. } => *http_status,
            Self::Unprotected { gap, .. } => gap.http_status(),
            Self::Protected { .. } => None,
        }
    }

    /// Map the outcome to score inclusion semantics.
    #[must_use]
    pub fn score_category(&self) -> ScoreCategory {
        match self {
            Self::Protected { .. } => ScoreCategory::Pass,
            Self::Unprotected { .. } => ScoreCategory::Fail,
            Self::Unobservable { reason, .. } => ScoreCategory::Excluded(match reason {
                IndeterminateReason::PermissionDenied
                | IndeterminateReason::PermissionSuspected => ExclusionReason::PermissionDenied,
                IndeterminateReason::Transient
                | IndeterminateReason::RateLimited
                | IndeterminateReason::Invalid
                | IndeterminateReason::Pending => ExclusionReason::Other,
            }),
        }
    }

    /// Classify observed history signals into an outcome.
    ///
    /// Both signals must be observed `true` for [`Self::Protected`]; `None`
    /// (unreadable) never counts as blocked. Absent sources must be supplied
    /// by the caller as an `Unprotected { gap: Absent }` variant directly.
    #[must_use]
    pub fn from_history_signals(
        default_branch: &str,
        force_push_blocked: Option<bool>,
        deletion_blocked: Option<bool>,
        timestamp: &str,
    ) -> Self {
        match (
            force_push_blocked.unwrap_or(false),
            deletion_blocked.unwrap_or(false),
        ) {
            (true, true) => Self::Protected {
                default_branch: default_branch.to_string(),
                timestamp: timestamp.to_string(),
            },
            (false, true) => Self::Unprotected {
                default_branch: default_branch.to_string(),
                gap: HistoryProtectionGap::ForcePushAllowed,
                timestamp: timestamp.to_string(),
            },
            (true, false) => Self::Unprotected {
                default_branch: default_branch.to_string(),
                gap: HistoryProtectionGap::DeletionAllowed,
                timestamp: timestamp.to_string(),
            },
            (false, false) => Self::Unprotected {
                default_branch: default_branch.to_string(),
                gap: HistoryProtectionGap::NeitherBlocked,
                timestamp: timestamp.to_string(),
            },
        }
    }
}

/// Content observed at a CODEOWNERS location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeownersContent {
    Parsed(ParsedCodeowners),
    Truncated(crate::domain::codeowners::CodeownersTruncationReason),
    Unparsed,
}

/// Recognized nonconforming CODEOWNERS locations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeownersNonConformingLocation {
    Root,
    Docs,
}

/// CODEOWNERS evaluation outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeownersResult {
    Conforming {
        content: CodeownersContent,
        timestamp: String,
    },
    NonConforming {
        location: CodeownersNonConformingLocation,
        content: CodeownersContent,
        timestamp: String,
    },
    Absent {
        timestamp: String,
    },
    Unobservable {
        reason: IndeterminateReason,
        timestamp: String,
    },
}

impl CodeownersResult {
    #[must_use]
    pub fn status(&self) -> CodeownersStatus {
        match self {
            Self::Conforming { .. } => CodeownersStatus::Conforming,
            Self::NonConforming { .. } => CodeownersStatus::NonConforming,
            Self::Absent { .. } => CodeownersStatus::Absent,
            Self::Unobservable { .. } => CodeownersStatus::Unknown,
        }
    }

    #[must_use]
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Conforming { .. } => Some(".github/CODEOWNERS"),
            Self::NonConforming {
                location: CodeownersNonConformingLocation::Root,
                ..
            } => Some("CODEOWNERS"),
            Self::NonConforming {
                location: CodeownersNonConformingLocation::Docs,
                ..
            } => Some("docs/CODEOWNERS"),
            Self::Absent { .. } | Self::Unobservable { .. } => None,
        }
    }

    #[must_use]
    pub fn timestamp(&self) -> &str {
        match self {
            Self::Conforming { timestamp, .. }
            | Self::NonConforming { timestamp, .. }
            | Self::Absent { timestamp }
            | Self::Unobservable { timestamp, .. } => timestamp,
        }
    }

    #[must_use]
    pub fn with_timestamp(mut self, value: impl Into<String>) -> Self {
        let timestamp = match &mut self {
            Self::Conforming { timestamp, .. }
            | Self::NonConforming { timestamp, .. }
            | Self::Absent { timestamp }
            | Self::Unobservable { timestamp, .. } => timestamp,
        };
        *timestamp = value.into();
        self
    }

    #[must_use]
    pub fn parsed(&self) -> Option<&ParsedCodeowners> {
        match self {
            Self::Conforming {
                content: CodeownersContent::Parsed(parsed),
                ..
            }
            | Self::NonConforming {
                content: CodeownersContent::Parsed(parsed),
                ..
            } => Some(parsed),
            Self::Conforming { .. }
            | Self::NonConforming { .. }
            | Self::Absent { .. }
            | Self::Unobservable { .. } => None,
        }
    }

    #[must_use]
    pub fn truncation(&self) -> Option<crate::domain::codeowners::CodeownersTruncationReason> {
        match self {
            Self::Conforming {
                content: CodeownersContent::Truncated(reason),
                ..
            }
            | Self::NonConforming {
                content: CodeownersContent::Truncated(reason),
                ..
            } => Some(*reason),
            Self::Conforming { .. }
            | Self::NonConforming { .. }
            | Self::Absent { .. }
            | Self::Unobservable { .. } => None,
        }
    }
}

impl From<&CodeownersResult> for ScoreCategory {
    fn from(result: &CodeownersResult) -> Self {
        match result {
            CodeownersResult::Unobservable { reason, .. } => Self::Excluded(match reason {
                IndeterminateReason::PermissionDenied
                | IndeterminateReason::PermissionSuspected => ExclusionReason::PermissionDenied,
                IndeterminateReason::Transient
                | IndeterminateReason::RateLimited
                | IndeterminateReason::Invalid
                | IndeterminateReason::Pending => ExclusionReason::Other,
            }),
            CodeownersResult::Conforming { .. }
            | CodeownersResult::NonConforming { .. }
            | CodeownersResult::Absent { .. } => Self::from(result.status()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum CodeownersStatus {
    /// CODEOWNERS file found in the conforming location (`.github/CODEOWNERS`).
    Conforming = 0,
    /// CODEOWNERS file found in a non-conforming location (e.g., repo root).
    NonConforming = 1,
    /// No CODEOWNERS file detected.
    Absent = 2,
    /// CODEOWNERS status could not be determined.
    Unknown = 3,
}

impl std::fmt::Display for CodeownersStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conforming => write!(f, "conforming"),
            Self::NonConforming => write!(f, "non_conforming"),
            Self::Absent => write!(f, "absent"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Reason a control was excluded from scoring.
///
/// Exhaustive over the exclusion space (COM-0028): every `*Status` source that
/// maps to [`ScoreCategory::Excluded`] selects one of these variants, with
/// `Other` as the explicit catch-all for causes the score breakdown does not
/// otherwise distinguish.
///
/// # Wire format
///
/// Serializable so it can ride in report-side aggregates (e.g. a
/// `(check_kind, reason) -> count` breakdown); never persisted on
/// `RepositoryEvidence` itself (CHE-0082:R6/CHE-0022:R6) — the reason
/// already lives in the per-check `*Status` enums via the
/// `From<...> for ScoreCategory` funnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    /// The check returned an explicit or suspected permission-denied response.
    PermissionDenied = 0,
    /// The check's status could not be determined.
    Unknown = 1,
    /// The check does not apply to this repository.
    NotApplicable = 2,
    /// Any other exclusion cause not otherwise distinguished (e.g. transient,
    /// rate-limited, or invalid collection responses).
    Other = 3,
}

impl std::fmt::Display for ExclusionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionDenied => write!(f, "permission_denied"),
            Self::Unknown => write!(f, "unknown"),
            Self::NotApplicable => write!(f, "not_applicable"),
            Self::Other => write!(f, "other"),
        }
    }
}

/// How a check status maps to score computation.
///
/// - `Pass` — control satisfied, counts as 1/1.
/// - `Fail` — control not satisfied, counts as 0/1.
/// - `Excluded` — status is indeterminate or not applicable, excluded from both
///   numerator and denominator. Carries [`ExclusionReason`] so an unmeasured
///   control's cause cannot be dropped (COM-0020 MISU).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoreCategory {
    /// Control is satisfied.
    Pass,
    /// Control is not satisfied.
    Fail,
    /// Control is indeterminate or not applicable; excluded from scoring.
    Excluded(ExclusionReason),
}

impl From<SecurityPolicyStatus> for ScoreCategory {
    fn from(s: SecurityPolicyStatus) -> Self {
        match s {
            SecurityPolicyStatus::Pass => Self::Pass,
            SecurityPolicyStatus::Fail => Self::Fail,
            SecurityPolicyStatus::Unknown => Self::Excluded(ExclusionReason::Unknown),
            SecurityPolicyStatus::NotApplicable => Self::Excluded(ExclusionReason::NotApplicable),
        }
    }
}

impl From<SecretScanningStatus> for ScoreCategory {
    fn from(s: SecretScanningStatus) -> Self {
        match s {
            SecretScanningStatus::Enabled => Self::Pass,
            SecretScanningStatus::Disabled => Self::Fail,
            SecretScanningStatus::PermissionDenied => {
                Self::Excluded(ExclusionReason::PermissionDenied)
            }
            SecretScanningStatus::Unknown => Self::Excluded(ExclusionReason::Unknown),
        }
    }
}

impl From<DependabotStatus> for ScoreCategory {
    fn from(s: DependabotStatus) -> Self {
        match s {
            DependabotStatus::Enabled => Self::Pass,
            DependabotStatus::Paused | DependabotStatus::Disabled => Self::Fail,
            DependabotStatus::Unknown => Self::Excluded(ExclusionReason::Unknown),
        }
    }
}

impl From<CodeownersStatus> for ScoreCategory {
    fn from(s: CodeownersStatus) -> Self {
        match s {
            CodeownersStatus::Conforming => Self::Pass,
            CodeownersStatus::NonConforming | CodeownersStatus::Absent => Self::Fail,
            CodeownersStatus::Unknown => Self::Excluded(ExclusionReason::Unknown),
        }
    }
}

/// Identifies a security check type.
///
/// Used to selectively trigger evaluation of individual checks
/// (e.g., in response to a webhook event affecting a specific control).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum CheckType {
    /// Maps to [`RepositoryChecks::security_policy`].
    SecurityPolicy = 0,
    /// Maps to [`RepositoryChecks::secret_scanning`].
    SecretScanning = 1,
    /// Maps to [`RepositoryChecks::dependabot_security_updates`].
    Dependabot = 2,
    /// Maps to [`RepositoryChecks::branch_protection`].
    BranchProtection = 3,
    /// Maps to [`RepositoryChecks::codeowners`].
    Codeowners = 4,
}

impl CheckType {
    /// All check types in declaration order. Useful for "evaluate everything"
    /// semantics and exhaustive iteration.
    ///
    /// **Ordering contract:** New variants must be appended at the end of the
    /// enum to preserve `Ord` stability (derived from declaration order).
    pub const ALL: &[CheckType] = &[
        CheckType::SecurityPolicy,
        CheckType::SecretScanning,
        CheckType::Dependabot,
        CheckType::BranchProtection,
        CheckType::Codeowners,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_disabled_preserves_positive_probe_without_becoming_enabled() {
        let result = SecretScanningResult::Disabled {
            metadata_http_status: Some(200),
            observation: DisabledObservation::StatusMismatch {
                source: ProbeSource::PerRepoEndpoint,
                http_status: Some(200),
            },
            timestamp: "2026-09-21T00:00:00Z".into(),
        };
        assert_eq!(result.status(), SecretScanningStatus::Disabled);
        assert!(!result.alerts_observable());
        assert!(result.status_mismatch());
    }

    #[test]
    fn policy_path_rejects_empty_and_unobservable_preserves_reason() {
        assert!(SecurityPolicyPath::new("").is_none());
        assert!(serde_json::from_str::<SecurityPolicyPath>("\"\"").is_err());
        let result = SecurityPolicyResult::Unobservable {
            reason: IndeterminateReason::PermissionDenied,
            timestamp: "2026-09-21T00:00:00Z".to_string(),
        };
        assert_eq!(result.status(), SecurityPolicyStatus::Unknown);
        assert_eq!(
            ScoreCategory::from(&result),
            ScoreCategory::Excluded(ExclusionReason::PermissionDenied)
        );
    }

    #[test]
    fn codeowners_location_and_content_are_associated() {
        let result = CodeownersResult::NonConforming {
            location: CodeownersNonConformingLocation::Docs,
            content: CodeownersContent::Unparsed,
            timestamp: "2026-09-21T00:00:00Z".into(),
        };
        assert_eq!(result.path(), Some("docs/CODEOWNERS"));
        assert!(result.parsed().is_none());
        assert!(result.truncation().is_none());
        let denied = CodeownersResult::Unobservable {
            reason: IndeterminateReason::PermissionDenied,
            timestamp: result.timestamp().to_string(),
        };
        assert_eq!(
            ScoreCategory::from(&denied),
            ScoreCategory::Excluded(ExclusionReason::PermissionDenied)
        );
    }

    #[test]
    fn history_signals_classify_protected_only_when_both_observed_blocked() {
        let protected = BranchProtectionResult::from_history_signals(
            "main",
            Some(true),
            Some(true),
            "2026-06-17T11:31:04Z",
        );
        assert!(protected.is_protected());
        assert_eq!(protected.gap(), None);

        let push_allowed = BranchProtectionResult::from_history_signals(
            "main",
            Some(false),
            Some(true),
            "2026-06-17T11:31:04Z",
        );
        assert!(!push_allowed.is_protected());
        assert_eq!(
            push_allowed.gap(),
            Some(HistoryProtectionGap::ForcePushAllowed)
        );

        let deletion_allowed = BranchProtectionResult::from_history_signals(
            "main",
            Some(true),
            Some(false),
            "2026-06-17T11:31:04Z",
        );
        assert!(!deletion_allowed.is_protected());
        assert_eq!(
            deletion_allowed.gap(),
            Some(HistoryProtectionGap::DeletionAllowed)
        );

        let neither = BranchProtectionResult::from_history_signals(
            "main",
            Some(false),
            Some(false),
            "2026-06-17T11:31:04Z",
        );
        assert!(!neither.is_protected());
        assert_eq!(neither.gap(), Some(HistoryProtectionGap::NeitherBlocked));
    }

    #[test]
    fn unreadable_signal_never_counts_as_blocked() {
        let unreadable_push = BranchProtectionResult::from_history_signals(
            "main",
            None,
            Some(true),
            "2026-06-17T11:31:04Z",
        );
        assert!(!unreadable_push.is_protected(), "None must not block");
        assert_eq!(
            unreadable_push.gap(),
            Some(HistoryProtectionGap::ForcePushAllowed),
            "an unreadable force-push signal plus an observed blocked deletion \
             is ForcePushAllowed, not NeitherBlocked"
        );

        let unreadable_deletion = BranchProtectionResult::from_history_signals(
            "main",
            Some(true),
            None,
            "2026-06-17T11:31:04Z",
        );
        assert!(!unreadable_deletion.is_protected());
        assert_eq!(
            unreadable_deletion.gap(),
            Some(HistoryProtectionGap::DeletionAllowed),
            "an observed blocked force-push plus an unreadable deletion signal \
             is DeletionAllowed, not NeitherBlocked"
        );

        let unobserved = BranchProtectionResult::from_history_signals(
            "main",
            None,
            None,
            "2026-06-17T11:31:04Z",
        );
        assert!(!unobserved.is_protected());
        assert_eq!(unobserved.gap(), Some(HistoryProtectionGap::NeitherBlocked));
    }

    #[test]
    fn branch_result_accessors_cover_all_variants() {
        let protected = BranchProtectionResult::Protected {
            default_branch: "main".to_string(),
            timestamp: "2026-06-17T11:31:04Z".to_string(),
        };
        assert!(protected.is_observed());
        assert!(!protected.is_pending());
        assert_eq!(protected.default_branch(), "main");
        assert_eq!(protected.timestamp(), "2026-06-17T11:31:04Z");
        assert_eq!(protected.http_status(), None);

        let absent = BranchProtectionResult::Unprotected {
            default_branch: "main".to_string(),
            gap: HistoryProtectionGap::Absent {
                confirmed_not_found: true,
                http_status: Some(404),
            },
            timestamp: "2026-06-17T11:31:04Z".to_string(),
        };
        assert!(absent.is_observed());
        assert_eq!(absent.http_status(), Some(404));
        assert!(absent.gap().is_some_and(|gap| gap.is_absent()));

        let pending = BranchProtectionResult::Unobservable {
            default_branch: "main".to_string(),
            reason: IndeterminateReason::Pending,
            http_status: None,
            timestamp: "2026-06-17T11:31:04Z".to_string(),
        };
        assert!(!pending.is_observed());
        assert!(pending.is_pending());
    }

    #[test]
    fn history_gap_http_status_only_on_absent() {
        assert_eq!(
            HistoryProtectionGap::Absent {
                confirmed_not_found: false,
                http_status: Some(403),
            }
            .http_status(),
            Some(403)
        );
        assert_eq!(HistoryProtectionGap::NeitherBlocked.http_status(), None);
        assert_eq!(HistoryProtectionGap::ForcePushAllowed.http_status(), None);
        assert_eq!(HistoryProtectionGap::DeletionAllowed.http_status(), None);
        assert!(
            HistoryProtectionGap::Absent {
                confirmed_not_found: false,
                http_status: None,
            }
            .is_absent()
        );
    }

    #[test]
    fn merge_empty_returns_none() {
        assert_eq!(HistorySignals::merge(&[]), None);
    }

    #[test]
    fn merge_takes_any_block_as_definitive() {
        let a = HistorySignals {
            force_push_blocked: Some(true),
            deletion_blocked: Some(false),
        };
        let b = HistorySignals {
            force_push_blocked: Some(false),
            deletion_blocked: Some(true),
        };
        let merged = HistorySignals::merge(&[a, b]).unwrap();
        assert_eq!(merged.force_push_blocked, Some(true));
        assert_eq!(merged.deletion_blocked, Some(true));
    }

    #[test]
    fn merge_observed_unblock_beats_unobserved() {
        let unobserved = HistorySignals {
            force_push_blocked: None,
            deletion_blocked: None,
        };
        let unblocked = HistorySignals {
            force_push_blocked: Some(false),
            deletion_blocked: Some(false),
        };
        let merged = HistorySignals::merge(&[unobserved, unblocked]).unwrap();
        assert_eq!(merged.force_push_blocked, Some(false));
        assert_eq!(merged.deletion_blocked, Some(false));
    }

    #[test]
    fn merge_absent_signals_are_preserved() {
        let merged = HistorySignals::merge(&[
            HistorySignals {
                force_push_blocked: None,
                deletion_blocked: None,
            },
            HistorySignals {
                force_push_blocked: None,
                deletion_blocked: Some(false),
            },
        ])
        .unwrap();
        assert_eq!(merged.force_push_blocked, None);
        assert_eq!(merged.deletion_blocked, Some(false));
    }

    #[test]
    fn serde_round_trip_security_policy_status_not_applicable() {
        let status = SecurityPolicyStatus::NotApplicable;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"not_applicable\"");
        let deserialized: SecurityPolicyStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, SecurityPolicyStatus::NotApplicable);
    }

    #[test]
    fn serde_round_trip_security_policy_evidence_not_applicable() {
        let evidence = SecurityPolicyEvidence::NotApplicable;
        let json = serde_json::to_string(&evidence).unwrap();
        assert_eq!(json, "\"not_applicable\"");
        let deserialized: SecurityPolicyEvidence = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, SecurityPolicyEvidence::NotApplicable);
    }

    #[test]
    fn serde_round_trip_exclusion_reason_all_variants() {
        for (reason, wire) in [
            (ExclusionReason::PermissionDenied, "\"permission_denied\""),
            (ExclusionReason::Unknown, "\"unknown\""),
            (ExclusionReason::NotApplicable, "\"not_applicable\""),
            (ExclusionReason::Other, "\"other\""),
        ] {
            let json = serde_json::to_string(&reason).unwrap();
            assert_eq!(json, wire);
            let deserialized: ExclusionReason = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, reason);
            assert_eq!(reason.to_string(), wire.trim_matches('"'));
        }
    }

    #[test]
    fn serde_round_trip_codeowners_status_non_conforming() {
        let status = CodeownersStatus::NonConforming;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"non_conforming\"");
        let deserialized: CodeownersStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, CodeownersStatus::NonConforming);
    }

    #[test]
    fn branch_result_serde_round_trip_all_variants() {
        let protected = BranchProtectionResult::Protected {
            default_branch: "main".to_string(),
            timestamp: "2026-06-17T11:31:04Z".to_string(),
        };
        let unprotected = BranchProtectionResult::Unprotected {
            default_branch: "main".to_string(),
            gap: HistoryProtectionGap::Absent {
                confirmed_not_found: true,
                http_status: Some(404),
            },
            timestamp: "2026-06-17T11:31:04Z".to_string(),
        };
        let unobservable = BranchProtectionResult::Unobservable {
            default_branch: "main".to_string(),
            reason: IndeterminateReason::PermissionDenied,
            http_status: Some(403),
            timestamp: "2026-06-17T11:31:04Z".to_string(),
        };

        for result in [protected, unprotected, unobservable] {
            let json = serde_json::to_string(&result).unwrap();
            let decoded: BranchProtectionResult = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, result);
        }
    }

    #[test]
    fn history_protection_gap_serde_round_trip() {
        for gap in [
            HistoryProtectionGap::Absent {
                confirmed_not_found: true,
                http_status: Some(404),
            },
            HistoryProtectionGap::NeitherBlocked,
            HistoryProtectionGap::ForcePushAllowed,
            HistoryProtectionGap::DeletionAllowed,
        ] {
            let json = serde_json::to_string(&gap).unwrap();
            let decoded: HistoryProtectionGap = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, gap);
        }
    }

    #[test]
    fn score_category_from_security_policy() {
        assert_eq!(
            ScoreCategory::from(SecurityPolicyStatus::Pass),
            ScoreCategory::Pass
        );
        assert_eq!(
            ScoreCategory::from(SecurityPolicyStatus::Fail),
            ScoreCategory::Fail
        );
        assert_eq!(
            ScoreCategory::from(SecurityPolicyStatus::Unknown),
            ScoreCategory::Excluded(ExclusionReason::Unknown)
        );
        assert_eq!(
            ScoreCategory::from(SecurityPolicyStatus::NotApplicable),
            ScoreCategory::Excluded(ExclusionReason::NotApplicable)
        );
    }

    #[test]
    fn score_category_from_secret_scanning() {
        assert_eq!(
            ScoreCategory::from(SecretScanningStatus::Enabled),
            ScoreCategory::Pass
        );
        assert_eq!(
            ScoreCategory::from(SecretScanningStatus::Disabled),
            ScoreCategory::Fail
        );
        assert_eq!(
            ScoreCategory::from(SecretScanningStatus::PermissionDenied),
            ScoreCategory::Excluded(ExclusionReason::PermissionDenied)
        );
        assert_eq!(
            ScoreCategory::from(SecretScanningStatus::Unknown),
            ScoreCategory::Excluded(ExclusionReason::Unknown)
        );
    }

    #[test]
    fn score_category_from_dependabot() {
        assert_eq!(
            ScoreCategory::from(DependabotStatus::Enabled),
            ScoreCategory::Pass
        );
        assert_eq!(
            ScoreCategory::from(DependabotStatus::Paused),
            ScoreCategory::Fail
        );
        assert_eq!(
            ScoreCategory::from(DependabotStatus::Disabled),
            ScoreCategory::Fail
        );
        assert_eq!(
            ScoreCategory::from(DependabotStatus::Unknown),
            ScoreCategory::Excluded(ExclusionReason::Unknown)
        );
    }

    #[test]
    fn score_category_from_branch_protection() {
        assert_eq!(
            BranchProtectionResult::Protected {
                default_branch: "main".to_string(),
                timestamp: "2026-06-17T11:31:04Z".to_string(),
            }
            .score_category(),
            ScoreCategory::Pass
        );
        assert_eq!(
            BranchProtectionResult::Unprotected {
                default_branch: "main".to_string(),
                gap: HistoryProtectionGap::NeitherBlocked,
                timestamp: "2026-06-17T11:31:04Z".to_string(),
            }
            .score_category(),
            ScoreCategory::Fail
        );
        assert_eq!(
            BranchProtectionResult::Unobservable {
                default_branch: "main".to_string(),
                reason: IndeterminateReason::Invalid,
                http_status: None,
                timestamp: "2026-06-17T11:31:04Z".to_string(),
            }
            .score_category(),
            ScoreCategory::Excluded(ExclusionReason::Other)
        );
    }

    #[test]
    fn branch_protection_score_category_carries_specific_reason() {
        fn excluded_with(reason: IndeterminateReason) -> ScoreCategory {
            BranchProtectionResult::Unobservable {
                default_branch: "main".to_string(),
                reason,
                http_status: Some(403),
                timestamp: "2026-06-17T11:31:04Z".to_string(),
            }
            .score_category()
        }

        assert_eq!(
            excluded_with(IndeterminateReason::PermissionDenied),
            ScoreCategory::Excluded(ExclusionReason::PermissionDenied)
        );
        assert_eq!(
            excluded_with(IndeterminateReason::PermissionSuspected),
            ScoreCategory::Excluded(ExclusionReason::PermissionDenied)
        );
        assert_eq!(
            excluded_with(IndeterminateReason::Transient),
            ScoreCategory::Excluded(ExclusionReason::Other)
        );
        assert_eq!(
            excluded_with(IndeterminateReason::RateLimited),
            ScoreCategory::Excluded(ExclusionReason::Other)
        );
        assert_eq!(
            excluded_with(IndeterminateReason::Invalid),
            ScoreCategory::Excluded(ExclusionReason::Other)
        );
        assert_eq!(
            excluded_with(IndeterminateReason::Pending),
            ScoreCategory::Excluded(ExclusionReason::Other)
        );
    }

    #[test]
    fn score_category_from_codeowners() {
        assert_eq!(
            ScoreCategory::from(CodeownersStatus::Conforming),
            ScoreCategory::Pass
        );
        assert_eq!(
            ScoreCategory::from(CodeownersStatus::NonConforming),
            ScoreCategory::Fail
        );
        assert_eq!(
            ScoreCategory::from(CodeownersStatus::Absent),
            ScoreCategory::Fail
        );
        assert_eq!(
            ScoreCategory::from(CodeownersStatus::Unknown),
            ScoreCategory::Excluded(ExclusionReason::Unknown)
        );
    }

    #[test]
    fn check_type_serde_wire_format() {
        assert_eq!(
            serde_json::to_string(&CheckType::SecurityPolicy).unwrap(),
            "\"security_policy\""
        );
        assert_eq!(
            serde_json::to_string(&CheckType::SecretScanning).unwrap(),
            "\"secret_scanning\""
        );
        assert_eq!(
            serde_json::to_string(&CheckType::Dependabot).unwrap(),
            "\"dependabot\""
        );
        assert_eq!(
            serde_json::to_string(&CheckType::BranchProtection).unwrap(),
            "\"branch_protection\""
        );
        assert_eq!(
            serde_json::to_string(&CheckType::Codeowners).unwrap(),
            "\"codeowners\""
        );
    }

    #[test]
    fn check_type_all_exhaustive() {
        use std::collections::BTreeSet;
        let serialized: BTreeSet<String> = CheckType::ALL
            .iter()
            .map(|c| {
                serde_json::to_string(c)
                    .unwrap()
                    .trim_matches('"')
                    .to_string()
            })
            .collect();
        let expected: BTreeSet<String> = [
            "security_policy",
            "secret_scanning",
            "dependabot",
            "branch_protection",
            "codeowners",
        ]
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
        assert_eq!(serialized, expected);
        assert_eq!(CheckType::ALL.len(), 5);
    }

    #[test]
    fn check_type_ord_is_stable() {
        let mut types = vec![
            CheckType::Codeowners,
            CheckType::SecurityPolicy,
            CheckType::Dependabot,
            CheckType::BranchProtection,
            CheckType::SecretScanning,
        ];
        types.sort();
        assert_eq!(
            types,
            vec![
                CheckType::SecurityPolicy,
                CheckType::SecretScanning,
                CheckType::Dependabot,
                CheckType::BranchProtection,
                CheckType::Codeowners,
            ]
        );
    }

    #[test]
    fn check_type_rejects_unknown_variant() {
        let result = serde_json::from_str::<CheckType>("\"unknown_check\"");
        assert!(result.is_err());
    }

    #[test]
    fn check_type_serde_round_trip() {
        for ct in CheckType::ALL {
            let json = serde_json::to_string(ct).unwrap();
            let deserialized: CheckType = serde_json::from_str(&json).unwrap();
            assert_eq!(*ct, deserialized);
        }
    }

    #[test]
    fn from_history_signals_exhaustive_signal_grid() {
        for fp in [None, Some(false), Some(true)] {
            for del in [None, Some(false), Some(true)] {
                let result = BranchProtectionResult::from_history_signals(
                    "main",
                    fp,
                    del,
                    "2026-06-17T11:31:04Z",
                );
                let expected_gap = match (fp.unwrap_or(false), del.unwrap_or(false)) {
                    (true, true) => None,
                    (false, true) => Some(HistoryProtectionGap::ForcePushAllowed),
                    (true, false) => Some(HistoryProtectionGap::DeletionAllowed),
                    (false, false) => Some(HistoryProtectionGap::NeitherBlocked),
                };
                assert_eq!(
                    result.is_protected(),
                    fp == Some(true) && del == Some(true),
                    "signals ({fp:?}, {del:?})"
                );
                assert_eq!(result.gap(), expected_gap, "signals ({fp:?}, {del:?})");
                if fp != Some(true) || del != Some(true) {
                    assert_eq!(
                        result.score_category(),
                        ScoreCategory::Fail,
                        "signals ({fp:?}, {del:?})"
                    );
                }
            }
        }
    }
}
