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

/// Security policy evaluation outcome.
///
/// # Wire format
///
/// Fields encode in declaration order via `Encode::encode`: `status`,
/// `evidence`, `path`, `timestamp`. Field reorder is a wire-format break
/// (CHE-0022:R3 + PGN-0003 + PGN-0013:R8); new fields must append.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityPolicyResult {
    /// Whether a security policy was detected.
    pub status: SecurityPolicyStatus,
    /// How the security policy check was determined.
    pub evidence: SecurityPolicyEvidence,
    /// Path to the security policy file, if found.
    pub path: Option<String>,
    /// ISO 8601 timestamp of when the check was performed.
    pub timestamp: String,
}

/// Security policy status.
///
/// # Wire format
///
/// Variant discriminant is `u8` of declaration position (`Pass=0`, `Fail=1`,
/// `Unknown=2`, `NotApplicable=3`). Reordering or inserting a variant is a
/// wire-format break (CHE-0022:R3 + PGN-0003 + PGN-0013:R8); new variants must be
/// appended.
///
/// ```
/// use gh_report::domain::checks::SecurityPolicyStatus;
/// assert_eq!(SecurityPolicyStatus::Fail as u8, 1);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum SecurityPolicyStatus {
    Pass = 0,
    Fail = 1,
    Unknown = 2,
    NotApplicable = 3,
}

/// How the security policy check was determined.
///
/// # Wire format
///
/// Variant discriminant is `u8` of declaration position (`Setting=0`, `File=1`,
/// `Absent=2`, `PermissionDenied=3`, `TransientError=4`, `CollectionError=5`,
/// `NotApplicable=6`). Reorder or insert is a wire-format break (CHE-0022:R3 +
/// PGN-0003 + PGN-0013:R8); new variants must append.
///
/// ```
/// use gh_report::domain::checks::SecurityPolicyEvidence;
/// assert_eq!(SecurityPolicyEvidence::File as u8, 1);
/// ```
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

/// Secret scanning evaluation outcome.
///
/// # Wire format
///
/// Fields encode in declaration order via `Encode::encode`: `status`,
/// `has_open_alerts`, `alerts_observable`, `reason`, `timestamp`. Field
/// reorder is a wire-format break (CHE-0022:R3 + PGN-0003 + PGN-0013:R8); new fields
/// must append.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretScanningResult {
    /// Whether secret scanning is enabled on the repository.
    pub status: SecretScanningStatus,
    /// Whether the repository has open secret scanning alerts, if observable.
    pub has_open_alerts: Option<bool>,
    /// Whether alert data is observable for this repository.
    pub alerts_observable: bool,
    /// Human-readable reason for the current status.
    pub reason: Option<String>,
    /// ISO 8601 timestamp of when the check was performed.
    pub timestamp: String,
}

/// Secret scanning status.
///
/// # Wire format
///
/// Variant discriminant is `u8` of declaration position (`Enabled=0`,
/// `Disabled=1`, `PermissionDenied=2`, `Unknown=3`). Reorder or insert is a
/// wire-format break (CHE-0022:R3 + PGN-0003 + PGN-0013:R8); new variants must append.
///
/// ```
/// use gh_report::domain::checks::SecretScanningStatus;
/// assert_eq!(SecretScanningStatus::Disabled as u8, 1);
/// ```
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

/// Branch protection evaluation outcome.
///
/// # Wire format
///
/// Fields encode in declaration order via `Encode::encode`: `status`, `details`,
/// `timestamp`. Field reorder is a wire-format break (CHE-0022:R3 + PGN-0003 +
/// PGN-0013:R8); new fields must append.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchProtectionResult {
    /// Overall branch protection status.
    pub status: BranchProtectionStatus,
    /// Detailed state of individual branch protection controls.
    pub details: BranchProtectionDetails,
    /// ISO 8601 timestamp of when the check was performed.
    pub timestamp: String,
}

/// Branch protection status.
///
/// # Wire format
///
/// Variant discriminant is `u8` of declaration position (`Pass=0`, `Partial=1`,
/// `Fail=2`, `Unknown=3`). Reorder or insert is a wire-format break
/// (CHE-0022:R3 + PGN-0003 + PGN-0013:R8); new variants must append.
///
/// ```
/// use gh_report::domain::checks::BranchProtectionStatus;
/// assert_eq!(BranchProtectionStatus::Partial as u8, 1);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum BranchProtectionStatus {
    Pass = 0,
    Partial = 1,
    Fail = 2,
    Unknown = 3,
}

/// Report-side tier for branch-protection scoring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum BranchProtectionTier {
    /// Protection could not be read or scored.
    Excluded = 0,
    /// T0: no effective baseline protection detected.
    BelowBaseline = 1,
    /// T1: protected branch with baseline force-push/deletion integrity.
    Minimal = 2,
    /// T2: T1 plus pull request review enforcement.
    AcceptBar = 3,
    /// T3+: T2 plus additive hardening bonuses.
    Bonus = 4,
}

/// Report-side branch-protection regime: a combination-based refinement of
/// [`BranchProtectionTier`] over the same seven persisted signals.
///
/// # Report-side only
///
/// NOT persisted on any evidence/wire/event type (CHE-0083:R7). Computed
/// on demand from [`BranchProtectionDetails`], mirroring the non-persisted
/// [`BranchProtectionTier`] model.
///
/// # Ordering
///
/// `BPR0` is the dedicated unmeasured band, off the strength axis. `BPR1`
/// through `BPR5` form the measured weakest-to-strongest ladder. `BPR3`
/// (`ReviewedWithBypass`) and `BPR4` (`ReviewedGated`) are the one place
/// this regime splits [`BranchProtectionTier::AcceptBar`], surfacing the
/// bypassable-vs-gated distinction the scalar tier collapses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum BranchProtectionRegime {
    /// BPR0: status unmeasured or excluded for collection-health reasons.
    Unmeasured = 0,
    /// BPR1: no effective baseline protection detected.
    Unprotected = 1,
    /// BPR2: integrity floor held (force-push and deletion both blocked),
    /// but no PR/review gate.
    IntegrityOnly = 2,
    /// BPR3: PR and review enforcement present, integrity floor held, but a
    /// broad bypass actor punches through.
    ReviewedWithBypass = 3,
    /// BPR4: PR and review enforcement present, integrity floor held, no
    /// broad bypass, but no required status checks.
    ReviewedGated = 4,
    /// BPR5: PR, review, integrity, and required status checks all held,
    /// with no broad bypass.
    Hardened = 5,
}

impl std::fmt::Display for BranchProtectionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pass => write!(f, "pass"),
            Self::Partial => write!(f, "partial"),
            Self::Fail => write!(f, "fail"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
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

/// Detailed branch protection controls state.
///
/// # Wire format
///
/// Fields encode in declaration order via `Encode::encode`: `default_branch`,
/// `has_pr`, `required_reviewers`, `has_status_checks`, `admin_equivalent`,
/// `has_broad_bypass`, `reason`, `reason_kind`, `http_status`,
/// `force_push_blocked`, `deletion_blocked`. Field reorder is a wire-format
/// break (CHE-0022:R3 + PGN-0003 + PGN-0013:R8); new fields must append.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchProtectionDetails {
    /// Name of the repository's default branch.
    pub default_branch: String,
    /// Whether pull requests are required before merging.
    pub has_pr: Option<bool>,
    /// Minimum number of required approving reviews, if configured.
    pub required_reviewers: Option<u32>,
    /// Whether required status checks are configured.
    pub has_status_checks: Option<bool>,
    /// Whether administrators are subject to the branch protection rules.
    pub admin_equivalent: Option<bool>,
    /// Whether a broad bypass actor weakens the protection guarantees.
    pub has_broad_bypass: Option<bool>,
    /// Human-readable reason for the current status.
    pub reason: Option<String>,
    /// Typed collection-health reason for aggregation and admin reporting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_kind: Option<CollectionFailureReason>,
    /// HTTP status code that produced the collection-health reason, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    /// Whether force pushes are blocked on the protected branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_push_blocked: Option<bool>,
    /// Whether branch deletion is blocked on the protected branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deletion_blocked: Option<bool>,
}

#[derive(Debug, Clone, Copy)]
struct BranchTierSignals {
    status: BranchProtectionStatus,
    reason_kind: Option<CollectionFailureReason>,
    has_pr: Option<bool>,
    required_reviewers: Option<u32>,
    has_status_checks: Option<bool>,
    admin_equivalent: Option<bool>,
    has_broad_bypass: Option<bool>,
    force_push_blocked: Option<bool>,
    deletion_blocked: Option<bool>,
}

fn classify_branch_tier(signals: BranchTierSignals) -> BranchProtectionTier {
    if signals.status == BranchProtectionStatus::Unknown
        || matches!(
            signals.reason_kind,
            Some(
                CollectionFailureReason::PermissionDenied
                    | CollectionFailureReason::PermissionSuspected
                    | CollectionFailureReason::Transient
                    | CollectionFailureReason::RateLimited
                    | CollectionFailureReason::Invalid
            )
        )
    {
        return BranchProtectionTier::Excluded;
    }

    let protected = signals.has_pr == Some(true)
        || signals.required_reviewers.is_some_and(|count| count > 0)
        || signals.has_status_checks == Some(true)
        || signals.admin_equivalent == Some(true)
        || signals.force_push_blocked == Some(true)
        || signals.deletion_blocked == Some(true);

    if !protected
        || signals.force_push_blocked == Some(false)
        || signals.deletion_blocked == Some(false)
    {
        return BranchProtectionTier::BelowBaseline;
    }

    let integrity_blocked =
        signals.force_push_blocked == Some(true) && signals.deletion_blocked == Some(true);

    if !integrity_blocked {
        return BranchProtectionTier::BelowBaseline;
    }

    if signals.has_pr != Some(true) || signals.required_reviewers.unwrap_or(0) == 0 {
        return BranchProtectionTier::Minimal;
    }

    if signals.has_broad_bypass == Some(true) {
        return BranchProtectionTier::AcceptBar;
    }

    if signals.has_status_checks == Some(true) {
        BranchProtectionTier::Bonus
    } else {
        BranchProtectionTier::AcceptBar
    }
}

fn classify_branch_protection_regime(signals: BranchTierSignals) -> BranchProtectionRegime {
    if signals.status == BranchProtectionStatus::Unknown
        || matches!(
            signals.reason_kind,
            Some(
                CollectionFailureReason::PermissionDenied
                    | CollectionFailureReason::PermissionSuspected
                    | CollectionFailureReason::Transient
                    | CollectionFailureReason::RateLimited
                    | CollectionFailureReason::Invalid
            )
        )
    {
        return BranchProtectionRegime::Unmeasured;
    }

    let protected = signals.has_pr == Some(true)
        || signals.required_reviewers.is_some_and(|count| count > 0)
        || signals.has_status_checks == Some(true)
        || signals.admin_equivalent == Some(true)
        || signals.force_push_blocked == Some(true)
        || signals.deletion_blocked == Some(true);

    if !protected {
        return BranchProtectionRegime::Unprotected;
    }

    let integrity_blocked =
        signals.force_push_blocked == Some(true) && signals.deletion_blocked == Some(true);

    if !integrity_blocked {
        return BranchProtectionRegime::Unprotected;
    }

    if signals.has_pr != Some(true) || signals.required_reviewers.unwrap_or(0) == 0 {
        return BranchProtectionRegime::IntegrityOnly;
    }

    if signals.has_broad_bypass == Some(true) {
        return BranchProtectionRegime::ReviewedWithBypass;
    }

    if signals.has_status_checks != Some(true) {
        return BranchProtectionRegime::ReviewedGated;
    }

    BranchProtectionRegime::Hardened
}

impl BranchProtectionResult {
    /// Compute the report-side branch-protection tier from raw details.
    #[must_use]
    pub fn tier(&self) -> BranchProtectionTier {
        classify_branch_tier(BranchTierSignals {
            status: self.status,
            reason_kind: self.details.reason_kind,
            has_pr: self.details.has_pr,
            required_reviewers: self.details.required_reviewers,
            has_status_checks: self.details.has_status_checks,
            admin_equivalent: self.details.admin_equivalent,
            has_broad_bypass: self.details.has_broad_bypass,
            force_push_blocked: self.details.force_push_blocked,
            deletion_blocked: self.details.deletion_blocked,
        })
    }

    /// Compute the report-side branch-protection regime (BPR0..BPR5) from
    /// raw details. Report-side only; not persisted (CHE-0083:R7).
    #[must_use]
    pub fn regime(&self) -> BranchProtectionRegime {
        classify_branch_protection_regime(BranchTierSignals {
            status: self.status,
            reason_kind: self.details.reason_kind,
            has_pr: self.details.has_pr,
            required_reviewers: self.details.required_reviewers,
            has_status_checks: self.details.has_status_checks,
            admin_equivalent: self.details.admin_equivalent,
            has_broad_bypass: self.details.has_broad_bypass,
            force_push_blocked: self.details.force_push_blocked,
            deletion_blocked: self.details.deletion_blocked,
        })
    }

    /// Map the report-side tier to score inclusion semantics.
    #[must_use]
    pub fn score_category(&self) -> ScoreCategory {
        match self.tier() {
            BranchProtectionTier::AcceptBar | BranchProtectionTier::Bonus => ScoreCategory::Pass,
            BranchProtectionTier::Minimal | BranchProtectionTier::BelowBaseline => {
                ScoreCategory::Fail
            }
            BranchProtectionTier::Excluded => ScoreCategory::Excluded(
                branch_protection_exclusion_reason(self.details.reason_kind),
            ),
        }
    }
}

fn branch_protection_exclusion_reason(
    reason_kind: Option<CollectionFailureReason>,
) -> ExclusionReason {
    match reason_kind {
        Some(
            CollectionFailureReason::PermissionDenied
            | CollectionFailureReason::PermissionSuspected,
        ) => ExclusionReason::PermissionDenied,
        Some(
            CollectionFailureReason::Transient
            | CollectionFailureReason::RateLimited
            | CollectionFailureReason::Invalid,
        ) => ExclusionReason::Other,
        Some(CollectionFailureReason::NotFoundAbsent) | None => ExclusionReason::Unknown,
    }
}

/// Intermediate representation of merged branch protection controls.
///
/// Used during evaluation before mapping to a final status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchRequirements {
    /// Whether pull requests are required before merging.
    pub has_pr: bool,
    /// Whether required status checks are configured.
    pub has_status_checks: bool,
    /// Whether administrators are subject to enforcement.
    pub admin_equivalent: bool,
    /// Whether force pushes are blocked by branch protection controls.
    pub force_push_blocked: Option<bool>,
    /// Whether branch deletion is blocked by branch protection controls.
    pub deletion_blocked: Option<bool>,
}

impl BranchRequirements {
    /// Create the set of branch protection requirements.
    #[must_use]
    pub fn new(has_pr: bool, has_status_checks: bool, admin_equivalent: bool) -> Self {
        Self {
            has_pr,
            has_status_checks,
            admin_equivalent,
            force_push_blocked: None,
            deletion_blocked: None,
        }
    }

    /// Attach force-push and deletion blocking signals.
    #[must_use]
    pub fn with_integrity_controls(
        mut self,
        force_push_blocked: Option<bool>,
        deletion_blocked: Option<bool>,
    ) -> Self {
        self.force_push_blocked = force_push_blocked;
        self.deletion_blocked = deletion_blocked;
        self
    }
}

/// Branch-control exceptions that weaken enforcement guarantees.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchExceptions {
    /// Whether a broad bypass actor (e.g., `OrganizationAdmin`) is configured.
    pub has_broad_bypass: bool,
}

/// Intermediate representation of merged branch protection controls.
///
/// Used during evaluation before mapping to a final status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchControls {
    /// Minimum number of required approving reviews across all sources.
    pub reviewer_count: u32,
    /// Merged branch protection requirements.
    pub requirements: BranchRequirements,
    /// Branch-control exceptions that weaken enforcement.
    pub exceptions: BranchExceptions,
}

impl BranchControls {
    /// Create a branch control set from the merged requirement signals.
    #[must_use]
    pub fn new(
        requirements: BranchRequirements,
        reviewer_count: u32,
        has_broad_bypass: bool,
    ) -> Self {
        Self {
            reviewer_count,
            requirements,
            exceptions: BranchExceptions { has_broad_bypass },
        }
    }

    /// Whether pull requests are required on the protected branch.
    #[must_use]
    pub fn has_pr(&self) -> bool {
        self.requirements.has_pr
    }

    /// Whether required status checks are configured.
    #[must_use]
    pub fn has_status_checks(&self) -> bool {
        self.requirements.has_status_checks
    }

    /// Whether admin-equivalent enforcement is in effect.
    #[must_use]
    pub fn admin_equivalent(&self) -> bool {
        self.requirements.admin_equivalent
    }

    /// Whether a broad bypass actor weakens the protection guarantees.
    #[must_use]
    pub fn has_broad_bypass(&self) -> bool {
        self.exceptions.has_broad_bypass
    }

    /// Whether force pushes are blocked, or `None` when unreadable.
    #[must_use]
    pub fn force_push_blocked(&self) -> Option<bool> {
        self.requirements.force_push_blocked
    }

    /// Whether branch deletion is blocked, or `None` when unreadable.
    #[must_use]
    pub fn deletion_blocked(&self) -> Option<bool> {
        self.requirements.deletion_blocked
    }

    /// Derive the branch protection status from the current controls.
    #[must_use]
    pub fn status(&self) -> BranchProtectionStatus {
        match self.tier() {
            BranchProtectionTier::Excluded => BranchProtectionStatus::Unknown,
            BranchProtectionTier::BelowBaseline => BranchProtectionStatus::Fail,
            BranchProtectionTier::Minimal => BranchProtectionStatus::Partial,
            BranchProtectionTier::AcceptBar | BranchProtectionTier::Bonus => {
                BranchProtectionStatus::Pass
            }
        }
    }

    /// Compute the report-side branch-protection tier from merged controls.
    #[must_use]
    pub fn tier(&self) -> BranchProtectionTier {
        classify_branch_tier(BranchTierSignals {
            status: BranchProtectionStatus::Partial,
            reason_kind: None,
            has_pr: Some(self.has_pr()),
            required_reviewers: Some(self.reviewer_count),
            has_status_checks: Some(self.has_status_checks()),
            admin_equivalent: Some(self.admin_equivalent()),
            has_broad_bypass: Some(self.has_broad_bypass()),
            force_push_blocked: self.force_push_blocked(),
            deletion_blocked: self.deletion_blocked(),
        })
    }

    /// Merge multiple control sets, taking the strongest signal for each field.
    ///
    /// The `admin_equivalent` field is set to `true` only if:
    /// 1. At least one control set has `admin_equivalent == true`, AND
    /// 2. NO control set has `has_broad_bypass == true`.
    ///
    /// This means a broad bypass from ANY source (e.g., a single ruleset with
    /// an org admin bypass) globally poisons the `admin_equivalent` signal across
    /// all merged sources. This ensures that
    /// broad bypass actors undermine administrative enforcement guarantees.
    ///
    /// # Examples
    ///
    /// ```
    /// use gh_report::domain::checks::{BranchControls, BranchRequirements};
    ///
    /// let a = BranchControls::new(BranchRequirements::new(true, false, true), 1, false);
    /// let b = BranchControls::new(BranchRequirements::new(false, true, false), 2, false);
    /// let merged = BranchControls::merge(&[a, b]).unwrap();
    /// assert!(merged.has_pr());
    /// assert!(merged.has_status_checks());
    /// ```
    #[must_use]
    pub fn merge(controls_list: &[BranchControls]) -> Option<BranchControls> {
        if controls_list.is_empty() {
            return None;
        }

        Some(BranchControls::new(
            BranchRequirements::new(
                controls_list.iter().any(BranchControls::has_pr),
                controls_list.iter().any(BranchControls::has_status_checks),
                !controls_list.iter().any(BranchControls::has_broad_bypass)
                    && controls_list.iter().any(BranchControls::admin_equivalent),
            )
            .with_integrity_controls(
                merge_optional_blocking_signal(controls_list, BranchControls::force_push_blocked),
                merge_optional_blocking_signal(controls_list, BranchControls::deletion_blocked),
            ),
            controls_list
                .iter()
                .map(|c| c.reviewer_count)
                .max()
                .unwrap_or(0),
            controls_list.iter().any(BranchControls::has_broad_bypass),
        ))
    }
}

fn merge_optional_blocking_signal(
    controls_list: &[BranchControls],
    select: fn(&BranchControls) -> Option<bool>,
) -> Option<bool> {
    let mut saw_unblocked = false;
    for controls in controls_list {
        match select(controls) {
            Some(true) => return Some(true),
            Some(false) => saw_unblocked = true,
            None => {}
        }
    }
    if saw_unblocked { Some(false) } else { None }
}

/// CODEOWNERS evaluation outcome.
///
/// # Wire format
///
/// Fields encode in declaration order via `Encode::encode`: `status`, `path`,
/// `timestamp`, `parsed`, `truncation`. Field reorder is a wire-format break
/// (CHE-0022:R3 + PGN-0003 + PGN-0013:R8); new fields must append.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeownersResult {
    /// Whether a CODEOWNERS file was found and in a conforming location.
    pub status: CodeownersStatus,
    /// Path to the CODEOWNERS file, if found.
    pub path: Option<String>,
    /// ISO 8601 timestamp of when the check was performed.
    pub timestamp: String,
    /// Parsed CODEOWNERS content (owners, patterns).
    /// Only populated when content was successfully downloaded and parsed.
    pub parsed: Option<ParsedCodeowners>,
    /// Reason the CODEOWNERS file was found but not parsed.
    /// `Some(_)` ⟺ status is `Conforming` or `NonConforming` AND `parsed` is `None`.
    /// Always `None` for `Absent` / `Unknown` (no file to parse) or when parse succeeded.
    pub truncation: Option<crate::domain::codeowners::CodeownersTruncationReason>,
}

/// CODEOWNERS status.
///
/// # Wire format
///
/// Variant discriminant is `u8` of declaration position (`Conforming=0`,
/// `NonConforming=1`, `Absent=2`, `Unknown=3`). Reorder or insert is a
/// wire-format break (CHE-0022:R3 + PGN-0003 + PGN-0013:R8); new variants must append.
///
/// ```
/// use gh_report::domain::checks::CodeownersStatus;
/// assert_eq!(CodeownersStatus::NonConforming as u8, 1);
/// ```
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

impl From<BranchProtectionStatus> for ScoreCategory {
    fn from(s: BranchProtectionStatus) -> Self {
        match s {
            BranchProtectionStatus::Pass => Self::Pass,
            BranchProtectionStatus::Partial | BranchProtectionStatus::Fail => Self::Fail,
            BranchProtectionStatus::Unknown => Self::Excluded(ExclusionReason::Unknown),
        }
    }
}

impl From<BranchProtectionTier> for ScoreCategory {
    fn from(tier: BranchProtectionTier) -> Self {
        match tier {
            BranchProtectionTier::AcceptBar | BranchProtectionTier::Bonus => Self::Pass,
            BranchProtectionTier::Minimal | BranchProtectionTier::BelowBaseline => Self::Fail,
            BranchProtectionTier::Excluded => Self::Excluded(ExclusionReason::Unknown),
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
    fn branch_controls_pass_when_all_present() {
        let controls = BranchControls::new(
            BranchRequirements::new(true, true, true)
                .with_integrity_controls(Some(true), Some(true)),
            1,
            false,
        );
        assert_eq!(controls.status(), BranchProtectionStatus::Pass);
    }

    #[test]
    fn branch_controls_partial_when_some_present() {
        let controls = BranchControls::new(
            BranchRequirements::new(false, false, true)
                .with_integrity_controls(Some(true), Some(true)),
            0,
            false,
        );
        assert_eq!(controls.status(), BranchProtectionStatus::Partial);
    }

    #[test]
    fn branch_controls_fail_when_none_present() {
        let controls = BranchControls::default();
        assert_eq!(controls.status(), BranchProtectionStatus::Fail);
    }

    #[test]
    fn branch_controls_broad_bypass_prevents_pass() {
        let controls = BranchControls::new(
            BranchRequirements::new(true, true, true)
                .with_integrity_controls(Some(true), Some(true)),
            2,
            true,
        );
        assert_eq!(controls.tier(), BranchProtectionTier::AcceptBar);
        assert_eq!(controls.status(), BranchProtectionStatus::Pass);
    }

    fn branch_details(
        has_pr: Option<bool>,
        required_reviewers: Option<u32>,
        has_status_checks: Option<bool>,
        admin_equivalent: Option<bool>,
        has_broad_bypass: Option<bool>,
        force_push_blocked: Option<bool>,
        deletion_blocked: Option<bool>,
    ) -> BranchProtectionDetails {
        BranchProtectionDetails {
            default_branch: "main".to_string(),
            has_pr,
            required_reviewers,
            has_status_checks,
            admin_equivalent,
            has_broad_bypass,
            reason: None,
            reason_kind: None,
            http_status: None,
            force_push_blocked,
            deletion_blocked,
        }
    }

    #[test]
    fn branch_tier_synthetic_force_push_deletion_boundaries() {
        let minimal = BranchProtectionResult {
            status: BranchProtectionStatus::Partial,
            details: branch_details(
                Some(false),
                Some(0),
                Some(false),
                Some(false),
                Some(false),
                Some(true),
                Some(true),
            ),
            timestamp: "2026-06-17T11:31:04Z".to_string(),
        };
        let accept = BranchProtectionResult {
            details: branch_details(
                Some(true),
                Some(1),
                Some(false),
                Some(false),
                Some(false),
                Some(true),
                Some(true),
            ),
            ..minimal.clone()
        };
        let bonus = BranchProtectionResult {
            details: branch_details(
                Some(true),
                Some(1),
                Some(true),
                Some(true),
                Some(false),
                Some(true),
                Some(true),
            ),
            ..minimal.clone()
        };
        let bypass_downgrade = BranchProtectionResult {
            details: branch_details(
                Some(true),
                Some(1),
                Some(true),
                Some(true),
                Some(true),
                Some(true),
                Some(true),
            ),
            ..minimal.clone()
        };
        let status_checks_without_pr = BranchProtectionResult {
            details: branch_details(
                Some(false),
                Some(0),
                Some(true),
                Some(false),
                Some(false),
                Some(true),
                Some(true),
            ),
            ..minimal.clone()
        };

        assert_eq!(minimal.tier(), BranchProtectionTier::Minimal);
        assert_eq!(accept.tier(), BranchProtectionTier::AcceptBar);
        assert_eq!(bonus.tier(), BranchProtectionTier::Bonus);
        assert_eq!(bypass_downgrade.tier(), BranchProtectionTier::AcceptBar);
        assert_eq!(
            status_checks_without_pr.tier(),
            BranchProtectionTier::Minimal
        );
    }

    #[test]
    fn branch_tier_permission_suspected_is_excluded() {
        let mut details = branch_details(None, None, None, None, None, None, None);
        details.reason_kind = Some(CollectionFailureReason::PermissionSuspected);
        details.http_status = Some(404);

        let result = BranchProtectionResult {
            status: BranchProtectionStatus::Unknown,
            details,
            timestamp: "2026-06-17T11:31:04Z".to_string(),
        };

        assert_eq!(result.tier(), BranchProtectionTier::Excluded);
    }

    #[test]
    fn branch_tier_missing_h5_inputs_are_always_below_baseline() {
        let legacy_pr = BranchProtectionResult {
            status: BranchProtectionStatus::Partial,
            details: branch_details(
                Some(true),
                Some(1),
                Some(false),
                Some(false),
                Some(false),
                None,
                None,
            ),
            timestamp: "2026-06-17T11:31:04Z".to_string(),
        };
        let legacy_weak_ruleset = BranchProtectionResult {
            status: BranchProtectionStatus::Partial,
            details: branch_details(
                Some(false),
                Some(0),
                Some(false),
                Some(true),
                Some(false),
                None,
                None,
            ),
            timestamp: "2026-06-17T11:31:04Z".to_string(),
        };

        assert_eq!(legacy_pr.tier(), BranchProtectionTier::BelowBaseline);
        assert_eq!(
            legacy_weak_ruleset.tier(),
            BranchProtectionTier::BelowBaseline
        );
    }

    #[test]
    fn branch_tier_soak_public_fingerprints_collapse_without_h5_and_synthetic_h5_orders() {
        let absent = BranchProtectionResult {
            status: BranchProtectionStatus::Fail,
            details: branch_details(None, None, None, None, None, None, None),
            timestamp: "2026-06-17T11:31:04Z".to_string(),
        };
        let weak_ruleset_without_h5 = BranchProtectionResult {
            status: BranchProtectionStatus::Partial,
            details: branch_details(
                Some(false),
                Some(0),
                Some(false),
                Some(true),
                Some(false),
                None,
                None,
            ),
            timestamp: "2026-06-17T11:31:04Z".to_string(),
        };
        let weak_ruleset_with_h5 = BranchProtectionResult {
            details: branch_details(
                Some(false),
                Some(0),
                Some(false),
                Some(true),
                Some(false),
                Some(true),
                Some(true),
            ),
            ..weak_ruleset_without_h5.clone()
        };

        let absent_count = (0..28)
            .filter(|_| absent.tier() == BranchProtectionTier::BelowBaseline)
            .count();
        let weak_without_h5_count = (0..14)
            .filter(|_| weak_ruleset_without_h5.tier() == BranchProtectionTier::BelowBaseline)
            .count();
        let weak_with_h5_count = (0..14)
            .filter(|_| weak_ruleset_with_h5.tier() == BranchProtectionTier::Minimal)
            .count();

        assert_eq!(absent_count, 28);
        assert_eq!(weak_without_h5_count, 14);
        assert_eq!(weak_with_h5_count, 14);
        assert_eq!(absent.tier(), weak_ruleset_without_h5.tier());
        assert!(absent.tier() < weak_ruleset_with_h5.tier());
        assert!(weak_ruleset_with_h5.tier() < BranchProtectionTier::AcceptBar);
    }

    #[test]
    fn merge_empty_returns_none() {
        assert_eq!(BranchControls::merge(&[]), None);
    }

    #[test]
    fn merge_takes_strongest_signals() {
        let a = BranchControls::new(
            BranchRequirements::new(true, false, true)
                .with_integrity_controls(Some(true), Some(false)),
            1,
            false,
        );
        let b = BranchControls::new(
            BranchRequirements::new(false, true, false)
                .with_integrity_controls(Some(false), Some(true)),
            2,
            false,
        );
        let merged = BranchControls::merge(&[a, b]).unwrap();
        assert!(merged.has_pr());
        assert_eq!(merged.reviewer_count, 2);
        assert!(merged.has_status_checks());
        assert!(merged.admin_equivalent());
        assert!(!merged.has_broad_bypass());
        assert_eq!(merged.force_push_blocked(), Some(true));
        assert_eq!(merged.deletion_blocked(), Some(true));
    }

    #[test]
    fn merge_broad_bypass_disables_admin_equivalent() {
        let a = BranchControls::new(
            BranchRequirements::new(true, true, true)
                .with_integrity_controls(Some(true), Some(true)),
            1,
            false,
        );
        let b = BranchControls::new(
            BranchRequirements::new(false, false, false)
                .with_integrity_controls(Some(false), Some(false)),
            0,
            true,
        );
        let merged = BranchControls::merge(&[a, b]).unwrap();
        assert!(!merged.admin_equivalent());
        assert!(merged.has_broad_bypass());
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
    fn branch_protection_details_round_trip_force_push_and_deletion_signals() {
        let details = BranchProtectionDetails {
            default_branch: "main".to_string(),
            has_pr: Some(true),
            required_reviewers: Some(1),
            has_status_checks: Some(false),
            admin_equivalent: Some(false),
            has_broad_bypass: Some(false),
            reason: None,
            reason_kind: None,
            http_status: None,
            force_push_blocked: Some(true),
            deletion_blocked: Some(false),
        };

        let json = serde_json::to_string(&details).unwrap();
        let decoded: BranchProtectionDetails = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.force_push_blocked, Some(true));
        assert_eq!(decoded.deletion_blocked, Some(false));
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
            ScoreCategory::from(BranchProtectionStatus::Pass),
            ScoreCategory::Pass
        );
        assert_eq!(
            ScoreCategory::from(BranchProtectionStatus::Partial),
            ScoreCategory::Fail
        );
        assert_eq!(
            ScoreCategory::from(BranchProtectionStatus::Fail),
            ScoreCategory::Fail
        );
        assert_eq!(
            ScoreCategory::from(BranchProtectionStatus::Unknown),
            ScoreCategory::Excluded(ExclusionReason::Unknown)
        );
    }

    #[test]
    fn score_category_from_branch_protection_tier() {
        assert_eq!(
            ScoreCategory::from(BranchProtectionTier::AcceptBar),
            ScoreCategory::Pass
        );
        assert_eq!(
            ScoreCategory::from(BranchProtectionTier::Bonus),
            ScoreCategory::Pass
        );
        assert_eq!(
            ScoreCategory::from(BranchProtectionTier::Minimal),
            ScoreCategory::Fail
        );
        assert_eq!(
            ScoreCategory::from(BranchProtectionTier::BelowBaseline),
            ScoreCategory::Fail
        );
        assert_eq!(
            ScoreCategory::from(BranchProtectionTier::Excluded),
            ScoreCategory::Excluded(ExclusionReason::Unknown)
        );
    }

    #[test]
    fn branch_protection_score_category_carries_specific_reason() {
        fn excluded_with(reason_kind: Option<CollectionFailureReason>) -> ScoreCategory {
            let mut details = branch_details(None, None, None, None, None, None, None);
            details.reason_kind = reason_kind;
            BranchProtectionResult {
                status: BranchProtectionStatus::Unknown,
                details,
                timestamp: "2026-06-17T11:31:04Z".to_string(),
            }
            .score_category()
        }

        assert_eq!(
            excluded_with(Some(CollectionFailureReason::PermissionDenied)),
            ScoreCategory::Excluded(ExclusionReason::PermissionDenied)
        );
        assert_eq!(
            excluded_with(Some(CollectionFailureReason::PermissionSuspected)),
            ScoreCategory::Excluded(ExclusionReason::PermissionDenied)
        );
        assert_eq!(
            excluded_with(Some(CollectionFailureReason::Transient)),
            ScoreCategory::Excluded(ExclusionReason::Other)
        );
        assert_eq!(
            excluded_with(Some(CollectionFailureReason::RateLimited)),
            ScoreCategory::Excluded(ExclusionReason::Other)
        );
        assert_eq!(
            excluded_with(Some(CollectionFailureReason::Invalid)),
            ScoreCategory::Excluded(ExclusionReason::Other)
        );
        assert_eq!(
            excluded_with(None),
            ScoreCategory::Excluded(ExclusionReason::Unknown)
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

    #[allow(
        clippy::too_many_arguments,
        reason = "test-only signal constructor mirroring BranchTierSignals's field count; fires only under --all-targets clippy, not the default lib build"
    )]
    fn bpr_signals(
        status: BranchProtectionStatus,
        reason_kind: Option<CollectionFailureReason>,
        has_pr: Option<bool>,
        required_reviewers: Option<u32>,
        has_status_checks: Option<bool>,
        admin_equivalent: Option<bool>,
        has_broad_bypass: Option<bool>,
        force_push_blocked: Option<bool>,
        deletion_blocked: Option<bool>,
    ) -> BranchTierSignals {
        BranchTierSignals {
            status,
            reason_kind,
            has_pr,
            required_reviewers,
            has_status_checks,
            admin_equivalent,
            has_broad_bypass,
            force_push_blocked,
            deletion_blocked,
        }
    }

    #[test]
    fn bpr_unmeasured_when_status_unknown() {
        let signals = bpr_signals(
            BranchProtectionStatus::Unknown,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(
            classify_branch_protection_regime(signals),
            BranchProtectionRegime::Unmeasured
        );
    }

    #[test]
    fn bpr_unmeasured_when_reason_kind_excluded() {
        let signals = bpr_signals(
            BranchProtectionStatus::Partial,
            Some(CollectionFailureReason::PermissionDenied),
            Some(true),
            Some(2),
            Some(true),
            Some(true),
            Some(false),
            Some(true),
            Some(true),
        );
        assert_eq!(
            classify_branch_protection_regime(signals),
            BranchProtectionRegime::Unmeasured
        );
    }

    #[test]
    fn bpr_not_excluded_when_reason_kind_not_found_absent() {
        let signals = bpr_signals(
            BranchProtectionStatus::Fail,
            Some(CollectionFailureReason::NotFoundAbsent),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(
            classify_branch_protection_regime(signals),
            BranchProtectionRegime::Unprotected
        );
    }

    #[test]
    fn bpr_unprotected_when_nothing_configured() {
        let signals = bpr_signals(
            BranchProtectionStatus::Fail,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(
            classify_branch_protection_regime(signals),
            BranchProtectionRegime::Unprotected
        );
    }

    #[test]
    fn bpr_unprotected_when_protected_but_integrity_not_held() {
        let signals = bpr_signals(
            BranchProtectionStatus::Partial,
            None,
            Some(true),
            Some(1),
            None,
            None,
            None,
            Some(true),
            Some(false),
        );
        assert_eq!(
            classify_branch_protection_regime(signals),
            BranchProtectionRegime::Unprotected
        );
    }

    #[test]
    fn bpr_integrity_only_when_no_review_gate() {
        let signals = bpr_signals(
            BranchProtectionStatus::Partial,
            None,
            Some(false),
            Some(0),
            None,
            None,
            None,
            Some(true),
            Some(true),
        );
        assert_eq!(
            classify_branch_protection_regime(signals),
            BranchProtectionRegime::IntegrityOnly
        );
    }

    #[test]
    fn bpr_reviewed_with_bypass_when_bypass_present() {
        let signals = bpr_signals(
            BranchProtectionStatus::Pass,
            None,
            Some(true),
            Some(2),
            Some(true),
            Some(false),
            Some(true),
            Some(true),
            Some(true),
        );
        assert_eq!(
            classify_branch_protection_regime(signals),
            BranchProtectionRegime::ReviewedWithBypass
        );
    }

    #[test]
    fn bpr_reviewed_gated_when_no_status_checks() {
        let signals = bpr_signals(
            BranchProtectionStatus::Pass,
            None,
            Some(true),
            Some(2),
            Some(false),
            Some(false),
            Some(false),
            Some(true),
            Some(true),
        );
        assert_eq!(
            classify_branch_protection_regime(signals),
            BranchProtectionRegime::ReviewedGated
        );
    }

    #[test]
    fn bpr_hardened_catch_all() {
        let signals = bpr_signals(
            BranchProtectionStatus::Pass,
            None,
            Some(true),
            Some(2),
            Some(true),
            Some(true),
            Some(false),
            Some(true),
            Some(true),
        );
        assert_eq!(
            classify_branch_protection_regime(signals),
            BranchProtectionRegime::Hardened
        );
    }

    #[test]
    fn bpr_consistency_map_matches_tier_for_non_split_bands() {
        type BprConsistencyCase = (
            Option<bool>,
            Option<u32>,
            Option<bool>,
            Option<bool>,
            Option<bool>,
            Option<bool>,
            Option<bool>,
        );
        let cases: &[BprConsistencyCase] = &[
            (None, None, None, None, None, None, None),
            (
                Some(false),
                Some(0),
                Some(false),
                Some(false),
                Some(false),
                Some(true),
                Some(true),
            ),
            (
                Some(true),
                Some(0),
                Some(false),
                Some(false),
                Some(false),
                Some(true),
                Some(true),
            ),
            (
                Some(true),
                Some(2),
                Some(true),
                Some(true),
                Some(false),
                Some(true),
                Some(true),
            ),
        ];
        for &(
            has_pr,
            required_reviewers,
            has_status_checks,
            admin_equivalent,
            has_broad_bypass,
            force_push_blocked,
            deletion_blocked,
        ) in cases
        {
            let signals = bpr_signals(
                BranchProtectionStatus::Partial,
                None,
                has_pr,
                required_reviewers,
                has_status_checks,
                admin_equivalent,
                has_broad_bypass,
                force_push_blocked,
                deletion_blocked,
            );
            let tier = classify_branch_tier(signals);
            let regime = classify_branch_protection_regime(signals);
            let expected = match tier {
                BranchProtectionTier::Excluded => BranchProtectionRegime::Unmeasured,
                BranchProtectionTier::BelowBaseline => BranchProtectionRegime::Unprotected,
                BranchProtectionTier::Minimal => BranchProtectionRegime::IntegrityOnly,
                BranchProtectionTier::AcceptBar => {
                    if has_broad_bypass == Some(true) {
                        BranchProtectionRegime::ReviewedWithBypass
                    } else {
                        BranchProtectionRegime::ReviewedGated
                    }
                }
                BranchProtectionTier::Bonus => BranchProtectionRegime::Hardened,
            };
            assert_eq!(regime, expected);
        }
    }

    proptest::proptest! {
        /// COM-0024:R2 — the BPR cascade is total (every input reaches
        /// exactly one band) and mutually exclusive (no input reaches two
        /// bands) over the full signal domain. Since the cascade always
        /// returns from its first matching step, totality/exclusivity
        /// reduce to: `classify_branch_protection_regime` never panics
        /// (totality) and always returns a single `BranchProtectionRegime`
        /// value per input (exclusivity is structural — a function returns
        /// one value). This test enumerates the option/bool domain named in
        /// the mission contract to demonstrate coverage over every reachable
        /// combination, cross-checking against the `excluded`/`expected`
        /// bindings computed inline below from the same signal domain.
        #[test]
        fn bpr_cascade_is_total_and_mutually_exclusive(
            status_idx in 0u8..4,
            reason_kind_idx in 0u8..7,
            has_pr in proptest::option::of(proptest::bool::ANY),
            required_reviewers in proptest::option::of(0u32..3),
            has_status_checks in proptest::option::of(proptest::bool::ANY),
            admin_equivalent in proptest::option::of(proptest::bool::ANY),
            has_broad_bypass in proptest::option::of(proptest::bool::ANY),
            force_push_blocked in proptest::option::of(proptest::bool::ANY),
            deletion_blocked in proptest::option::of(proptest::bool::ANY),
        ) {
            let status = match status_idx {
                0 => BranchProtectionStatus::Pass,
                1 => BranchProtectionStatus::Partial,
                2 => BranchProtectionStatus::Fail,
                _ => BranchProtectionStatus::Unknown,
            };
            let reason_kind = match reason_kind_idx {
                0 => None,
                1 => Some(CollectionFailureReason::PermissionDenied),
                2 => Some(CollectionFailureReason::PermissionSuspected),
                3 => Some(CollectionFailureReason::NotFoundAbsent),
                4 => Some(CollectionFailureReason::Transient),
                5 => Some(CollectionFailureReason::RateLimited),
                _ => Some(CollectionFailureReason::Invalid),
            };
            let signals = bpr_signals(
                status,
                reason_kind,
                has_pr,
                required_reviewers,
                has_status_checks,
                admin_equivalent,
                has_broad_bypass,
                force_push_blocked,
                deletion_blocked,
            );

            let regime = classify_branch_protection_regime(signals);

            let excluded = status == BranchProtectionStatus::Unknown
                || matches!(
                    reason_kind,
                    Some(
                        CollectionFailureReason::PermissionDenied
                            | CollectionFailureReason::PermissionSuspected
                            | CollectionFailureReason::Transient
                            | CollectionFailureReason::RateLimited
                            | CollectionFailureReason::Invalid
                    )
                );
            let protected = has_pr == Some(true)
                || required_reviewers.is_some_and(|count| count > 0)
                || has_status_checks == Some(true)
                || admin_equivalent == Some(true)
                || force_push_blocked == Some(true)
                || deletion_blocked == Some(true);
            let integrity_blocked =
                force_push_blocked == Some(true) && deletion_blocked == Some(true);

            let expected = if excluded {
                BranchProtectionRegime::Unmeasured
            } else if !protected || !integrity_blocked {
                BranchProtectionRegime::Unprotected
            } else if has_pr != Some(true) || required_reviewers.unwrap_or(0) == 0 {
                BranchProtectionRegime::IntegrityOnly
            } else if has_broad_bypass == Some(true) {
                BranchProtectionRegime::ReviewedWithBypass
            } else if has_status_checks != Some(true) {
                BranchProtectionRegime::ReviewedGated
            } else {
                BranchProtectionRegime::Hardened
            };

            proptest::prop_assert_eq!(regime, expected);
        }
    }
}
