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
/// `has_broad_bypass`, `reason`, `reason_kind`, `http_status`. Field reorder
/// is a wire-format break (CHE-0022:R3 + PGN-0003 + PGN-0013:R8); new fields
/// must append.
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
}

impl BranchRequirements {
    /// Create the set of branch protection requirements.
    #[must_use]
    pub fn new(has_pr: bool, has_status_checks: bool, admin_equivalent: bool) -> Self {
        Self {
            has_pr,
            has_status_checks,
            admin_equivalent,
        }
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

    /// Derive the branch protection status from the current controls.
    #[must_use]
    pub fn status(&self) -> BranchProtectionStatus {
        let all_controls = self.has_pr()
            && self.reviewer_count >= 1
            && self.has_status_checks()
            && self.admin_equivalent()
            && !self.has_broad_bypass();

        if all_controls {
            return BranchProtectionStatus::Pass;
        }

        let any_controls = self.has_pr()
            || self.reviewer_count >= 1
            || self.has_status_checks()
            || self.admin_equivalent();

        if any_controls {
            BranchProtectionStatus::Partial
        } else {
            BranchProtectionStatus::Fail
        }
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

/// How a check status maps to score computation.
///
/// - `Pass` — control satisfied, counts as 1/1.
/// - `Fail` — control not satisfied, counts as 0/1.
/// - `Excluded` — status is indeterminate or not applicable, excluded from both
///   numerator and denominator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ScoreCategory {
    /// Control is satisfied.
    Pass = 0,
    /// Control is not satisfied.
    Fail = 1,
    /// Control is indeterminate or not applicable; excluded from scoring.
    Excluded = 2,
}

impl From<SecurityPolicyStatus> for ScoreCategory {
    fn from(s: SecurityPolicyStatus) -> Self {
        match s {
            SecurityPolicyStatus::Pass => Self::Pass,
            SecurityPolicyStatus::Fail => Self::Fail,
            SecurityPolicyStatus::Unknown | SecurityPolicyStatus::NotApplicable => Self::Excluded,
        }
    }
}

impl From<SecretScanningStatus> for ScoreCategory {
    fn from(s: SecretScanningStatus) -> Self {
        match s {
            SecretScanningStatus::Enabled => Self::Pass,
            SecretScanningStatus::Disabled => Self::Fail,
            SecretScanningStatus::PermissionDenied | SecretScanningStatus::Unknown => {
                Self::Excluded
            }
        }
    }
}

impl From<DependabotStatus> for ScoreCategory {
    fn from(s: DependabotStatus) -> Self {
        match s {
            DependabotStatus::Enabled => Self::Pass,
            DependabotStatus::Paused | DependabotStatus::Disabled => Self::Fail,
            DependabotStatus::Unknown => Self::Excluded,
        }
    }
}

impl From<BranchProtectionStatus> for ScoreCategory {
    fn from(s: BranchProtectionStatus) -> Self {
        match s {
            BranchProtectionStatus::Pass => Self::Pass,
            BranchProtectionStatus::Partial | BranchProtectionStatus::Fail => Self::Fail,
            BranchProtectionStatus::Unknown => Self::Excluded,
        }
    }
}

impl From<CodeownersStatus> for ScoreCategory {
    fn from(s: CodeownersStatus) -> Self {
        match s {
            CodeownersStatus::Conforming => Self::Pass,
            CodeownersStatus::NonConforming | CodeownersStatus::Absent => Self::Fail,
            CodeownersStatus::Unknown => Self::Excluded,
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
        let controls = BranchControls::new(BranchRequirements::new(true, true, true), 1, false);
        assert_eq!(controls.status(), BranchProtectionStatus::Pass);
    }

    #[test]
    fn branch_controls_partial_when_some_present() {
        let controls = BranchControls::new(BranchRequirements::new(true, false, false), 0, false);
        assert_eq!(controls.status(), BranchProtectionStatus::Partial);
    }

    #[test]
    fn branch_controls_fail_when_none_present() {
        let controls = BranchControls::default();
        assert_eq!(controls.status(), BranchProtectionStatus::Fail);
    }

    #[test]
    fn branch_controls_broad_bypass_prevents_pass() {
        let controls = BranchControls::new(BranchRequirements::new(true, true, true), 2, true);
        assert_eq!(controls.status(), BranchProtectionStatus::Partial);
    }

    #[test]
    fn merge_empty_returns_none() {
        assert_eq!(BranchControls::merge(&[]), None);
    }

    #[test]
    fn merge_takes_strongest_signals() {
        let a = BranchControls::new(BranchRequirements::new(true, false, true), 1, false);
        let b = BranchControls::new(BranchRequirements::new(false, true, false), 2, false);
        let merged = BranchControls::merge(&[a, b]).unwrap();
        assert!(merged.has_pr());
        assert_eq!(merged.reviewer_count, 2);
        assert!(merged.has_status_checks());
        assert!(merged.admin_equivalent());
        assert!(!merged.has_broad_bypass());
    }

    #[test]
    fn merge_broad_bypass_disables_admin_equivalent() {
        let a = BranchControls::new(BranchRequirements::new(true, true, true), 1, false);
        let b = BranchControls::new(BranchRequirements::new(false, false, false), 0, true);
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
    fn serde_round_trip_codeowners_status_non_conforming() {
        let status = CodeownersStatus::NonConforming;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"non_conforming\"");
        let deserialized: CodeownersStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, CodeownersStatus::NonConforming);
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
            ScoreCategory::Excluded
        );
        assert_eq!(
            ScoreCategory::from(SecurityPolicyStatus::NotApplicable),
            ScoreCategory::Excluded
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
            ScoreCategory::Excluded
        );
        assert_eq!(
            ScoreCategory::from(SecretScanningStatus::Unknown),
            ScoreCategory::Excluded
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
            ScoreCategory::Excluded
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
            ScoreCategory::Excluded
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
            ScoreCategory::Excluded
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
}
