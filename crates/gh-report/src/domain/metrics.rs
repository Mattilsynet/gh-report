//! Aggregated metrics computed from per-repository check results.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::domain::checks::{CollectionFailureReason, ExclusionReason};
use crate::domain::status::CollectionStatus;

/// A rate metric with numerator, denominator, and optional rate percentage.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RateMetric {
    /// Number of repositories that satisfy the metric condition.
    pub numerator: u32,
    /// Total number of repositories in scope for the metric.
    pub denominator: u32,
    /// Percentage rate, or `None` if denominator is zero.
    pub rate: Option<f64>,
    /// Additional context fields.
    ///
    /// Uses `#[serde(flatten)]` which has O(n²) worst-case deserialization for
    /// wide JSON objects. This is acceptable because `RateMetric` is only ever
    /// deserialized from application-generated checkpoint/baseline files, never
    /// from untrusted external API input.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl RateMetric {
    /// Build a rate metric from numerator and denominator.
    ///
    /// # Examples
    ///
    /// ```
    /// use gh_report::domain::metrics::RateMetric;
    ///
    /// let metric = RateMetric::new(3, 10);
    /// assert_eq!(metric.rate, Some(30.0));
    /// ```
    #[must_use]
    pub fn new(numerator: u32, denominator: u32) -> Self {
        let rate = if denominator > 0 {
            Some((f64::from(numerator) / f64::from(denominator) * 1000.0).round() / 10.0)
        } else {
            None
        };
        Self {
            numerator,
            denominator,
            rate,
            extra: HashMap::new(),
        }
    }

    /// Build a rate metric with extra context fields.
    #[must_use]
    pub fn with_extra(mut self, key: &str, value: impl Into<serde_json::Value>) -> Self {
        self.extra.insert(key.to_string(), value.into());
        self
    }

    /// Render this rate at whole-percent precision, for table-cell display
    /// where [`std::fmt::Display`]'s 1-decimal precision is reserved for
    /// prose (e.g. `"80% (4/5)"` vs. `Display`'s `"80.0% (4/5)"`).
    ///
    /// # Examples
    ///
    /// ```
    /// use gh_report::domain::metrics::RateMetric;
    ///
    /// let metric = RateMetric::new(4, 5);
    /// assert_eq!(metric.to_table_string(), "80% (4/5)");
    /// ```
    #[must_use]
    pub fn to_table_string(&self) -> String {
        match self.rate {
            Some(rate) => format!("{rate:.0}% ({}/{})", self.numerator, self.denominator),
            None => format!("N/A ({}/{})", self.numerator, self.denominator),
        }
    }
}

impl std::fmt::Display for RateMetric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.rate {
            Some(rate) => write!(f, "{rate:.1}% ({}/{})", self.numerator, self.denominator),
            None => write!(f, "N/A ({}/{})", self.numerator, self.denominator),
        }
    }
}

/// Counts for security policy evaluation buckets.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyCounts {
    /// Repos with a security policy detected via the GitHub API setting.
    pub via_setting: u32,
    /// Repos with a security policy detected via file presence (e.g., `SECURITY.md`).
    pub via_file: u32,
    /// Repos where the security policy status could not be determined.
    pub unknown: u32,
    /// Repos with no security policy detected.
    pub missing: u32,
}

/// Counts for secret scanning status buckets.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretScanningCounts {
    /// Repos with secret scanning enabled.
    pub enabled: u32,
    /// Repos with secret scanning disabled.
    pub disabled: u32,
    /// Repos where secret scanning status could not be read due to insufficient permissions.
    pub permission_denied: u32,
    /// Repos where the secret scanning status could not be determined.
    pub unknown: u32,
}

/// Counts for Dependabot security updates status buckets.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DependabotCounts {
    /// Repos with Dependabot security updates enabled.
    pub enabled: u32,
    /// Repos with Dependabot security updates paused.
    #[serde(default)]
    pub paused: u32,
    /// Repos with Dependabot security updates disabled.
    pub disabled: u32,
    /// Repos where the Dependabot status could not be determined.
    pub unknown: u32,
}

/// Counts for branch protection status buckets.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BranchProtectionCounts {
    /// Repos at branch-protection regime BPR2 (`IntegrityOnly`) or higher
    /// (CHE-0083 amended, CHE-0090).
    pub pass: u32,
    /// Unused under the BPR2+ pass bar — always zero. Retained so
    /// [`AggregatedMetrics::branch_protection_coverage`]'s
    /// `non_pass = partial + fail` arithmetic keeps a stable shape across
    /// pass-bar changes.
    pub partial: u32,
    /// Repos at branch-protection regime BPR1 (`Unprotected`) — below the
    /// pass bar.
    pub fail: u32,
    /// Repos where branch protection is `Excluded` from scoring — status
    /// indeterminate or not applicable (`BranchProtectionTier::Excluded`) —
    /// and so dropped from `branch_protection_coverage`'s denominator
    /// rather than counted as `fail`. Populated since item6-02; previously
    /// dead (folded into `fail`, never incremented — adr-fmt-pcoqb). See
    /// [`AggregatedMetrics::score_exclusion_counts`] for the breakdown by
    /// exclusion reason.
    pub unknown: u32,
}

/// Counts for CODEOWNERS status buckets.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodeownersCounts {
    /// Repos with a CODEOWNERS file in the conforming location (`.github/CODEOWNERS`).
    pub conforming: u32,
    /// Repos with a CODEOWNERS file in a non-conforming location (e.g., repo root).
    pub non_conforming: u32,
    /// Repos with no CODEOWNERS file detected.
    pub absent: u32,
    /// Repos where the CODEOWNERS status could not be determined.
    pub unknown: u32,
    /// Repos where the CODEOWNERS file was found but content parsing was
    /// skipped (encoding mismatch, oversized payload, decode failure, invalid
    /// UTF-8). Orthogonal to status: a repo may be `Conforming` *and*
    /// truncated (status comes from the path probe, parsing comes from the
    /// content fetch). Surfaced so operators can detect silent data loss
    /// without scanning per-repo evidence. Forward-compat: `#[serde(default)]`
    /// allows loading baselines that predate `EVIDENCE_SCHEMA_VERSION 15.0`.
    #[serde(default)]
    pub truncated: u32,
}

/// Counts for secret alert observability buckets.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretAlertCounts {
    /// Repos with at least one open secret scanning alert.
    pub repos_with_open_alerts: u32,
    /// Repos with no open secret scanning alerts.
    pub repos_without_open_alerts: u32,
    /// Repos where alert status could not be observed (disabled or permission denied).
    pub unobservable: u32,
}

/// Aggregated metrics across all non-archived repositories.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AggregatedMetrics {
    /// Security policy coverage rate across active repositories.
    pub security_policy_coverage: RateMetric,
    /// Breakdown of security policy evaluation outcomes.
    pub policy_counts: PolicyCounts,

    /// Secret scanning enablement rate across active repositories.
    pub secret_scanning_coverage: RateMetric,
    /// Breakdown of secret scanning status outcomes.
    pub secret_scanning_counts: SecretScanningCounts,

    /// Dependabot security updates enablement rate across active repositories.
    pub dependabot_security_updates_coverage: RateMetric,
    /// Breakdown of Dependabot security updates status outcomes.
    pub dependabot_security_updates_counts: DependabotCounts,

    /// Prevalence of open secret scanning alerts across observable repositories.
    pub open_secret_alert_prevalence: RateMetric,
    /// Breakdown of secret alert observability outcomes.
    pub secret_alert_counts: SecretAlertCounts,

    /// Branch protection coverage rate across active repositories.
    pub branch_protection_coverage: RateMetric,
    /// Breakdown of branch protection status outcomes.
    pub branch_protection_counts: BranchProtectionCounts,

    /// CODEOWNERS coverage rate across active repositories.
    pub codeowners_coverage: RateMetric,
    /// Breakdown of CODEOWNERS status outcomes.
    pub codeowners_counts: CodeownersCounts,

    /// Per-owner metrics computed from CODEOWNERS parsed data.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub owner_metrics: Vec<OwnerMetrics>,
    /// Report-side collection-health taxonomy keyed by check kind and reason.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collection_health_counts: Vec<CollectionHealthCount>,
    /// Report-side breakdown of the 5 shared controls' `Excluded`
    /// classifications, keyed by `(check_kind, reason)`. Each control's
    /// coverage denominator above already drops these repos; this field
    /// says *why* they were dropped. Derived from `ScoreCategory`
    /// classification — never persisted on `RepositoryEvidence`
    /// (CHE-0082:R6/CHE-0022:R6). Mirrors `collection_health_counts`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub score_exclusion_counts: Vec<ScoreExclusionCount>,
    /// Team rosters fetched fresh at render time (B1), one per team-type
    /// owner in `owner_metrics`. Assigned by the evidence builder after
    /// aggregation returns, because the roster fetch is async and this
    /// struct is otherwise built synchronously from already-collected
    /// repository evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub team_rosters: Vec<TeamRoster>,
}

/// Collection-health check-kind axis for report-side aggregation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum CollectionHealthCheckKind {
    BranchProtection = 0,
    SecretScanning = 1,
    Dependabot = 2,
    Codeowners = 3,
    SecurityPolicy = 4,
    Inventory = 5,
    Rulesets = 6,
}

/// Report-side collection-health count for a `(check_kind, reason)` pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionHealthCount {
    pub check_kind: CollectionHealthCheckKind,
    pub reason: CollectionFailureReason,
    pub count: u32,
}

/// Report-side count of `Excluded` classifications for one shared control,
/// keyed by `(check_kind, reason)`. Mirrors [`CollectionHealthCount`], but
/// counts a different axis: how many repos this control's org-wide rate
/// dropped from its denominator, and why (per [`ExclusionReason`]) — not
/// raw collection-health signal prevalence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoreExclusionCount {
    pub check_kind: CollectionHealthCheckKind,
    pub reason: ExclusionReason,
    pub count: u32,
}

/// Per-owner metrics computed from CODEOWNERS parsed data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OwnerMetrics {
    /// Canonical owner name (e.g., `@org/team` or `@user`).
    pub owner: String,
    /// Display name (first-seen casing).
    pub display_name: String,
    /// Whether this owner is a team or individual user.
    pub owner_type: OwnerType,
    /// Number of repos this owner is responsible for.
    pub total_repos: u32,
    /// Per-control pass rate metrics.
    pub per_control_coverage: HashMap<String, RateMetric>,
    /// Report-side breakdown of this owner's shared controls' `Excluded`
    /// classifications, keyed by `(check_kind, reason)`. Each control's
    /// coverage denominator in `per_control_coverage` already drops these
    /// repos; this field says *why*. Mirrors
    /// `AggregatedMetrics::score_exclusion_counts`, scoped to this owner's
    /// repos. Derived from `ScoreCategory` classification — never persisted
    /// on `RepositoryEvidence` (CHE-0082:R6/CHE-0022:R6).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub score_exclusion_counts: Vec<ScoreExclusionCount>,
    /// Current org-membership state for `OwnerType::User` owners, fetched
    /// fresh at render time (item9 Part B). `None` for `OwnerType::Team`
    /// owners (team membership isn't org membership) and whenever the
    /// org-members fetch was unfetched/degraded (never flag on missing
    /// data); `Some(false)`/`Some(true)` when the org-members set was
    /// fetched and this owner's login was checked against it. Render-time
    /// only, mirroring [`TeamMember::in_org`] — never persisted to the
    /// native per-repo event payload (oracle adr-fmt-kqavx CLASS B verdict,
    /// re-confirmed adr-fmt-v6hgj).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_org: Option<bool>,
}

/// Owner type classification.
///
/// Three-valued (CHE-0082:R5 + CHE-0089:R4): a canonical CODEOWNERS owner string is
/// classified `Team` only when a team slug can actually be extracted
/// from it (`@org/team-slug`); `AmbiguousTeamShaped` when the string
/// contains `/` (team-shaped) but no usable slug could be extracted
/// (e.g. a trailing-slash or empty-segment malformed reference); `User`
/// only for a genuinely slash-less owner. A team-shaped owner never
/// silently collapses to `User` (CHE-0082:R5 + CHE-0089:R4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerType {
    /// A team with an extractable slug (e.g., `@org/team-name`).
    Team,
    /// Team-shaped (contains `/`) but no usable team slug could be
    /// extracted — classification is unresolved, never silently `User`.
    AmbiguousTeamShaped,
    /// An individual user (e.g., `@username` — no `/`).
    User,
}

impl std::fmt::Display for OwnerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Team => f.write_str("Team"),
            Self::AmbiguousTeamShaped => f.write_str("Ambiguous"),
            Self::User => f.write_str("User"),
        }
    }
}

/// A GitHub team member's role within the team.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamMemberRole {
    /// Team maintainer (elevated team-management permissions).
    Maintainer,
    /// Ordinary team member.
    Member,
}

impl std::fmt::Display for TeamMemberRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Maintainer => f.write_str("Maintainer"),
            Self::Member => f.write_str("Member"),
        }
    }
}

/// A single GitHub team member, fetched fresh at render time (B1).
///
/// Render-time-only: never persisted to the native per-repo event payload
/// (oracle adr-fmt-kqavx CLASS B verdict).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMember {
    /// GitHub login.
    pub login: String,
    /// The member's role on this team.
    pub role: TeamMemberRole,
    /// Current org-membership state, cross-checked at render time against
    /// a freshly-fetched org-members set (item9 Part B). `None` when the
    /// org-members fetch was unfetched/degraded — never flag on missing
    /// data; `Some(false)` when this login is confirmed absent from the
    /// org (departed); `Some(true)` when confirmed present. Render-time
    /// only, same CLASS B verdict as the rest of this type (oracle
    /// adr-fmt-kqavx, re-confirmed adr-fmt-v6hgj).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_org: Option<bool>,
}

/// Outcome of fetching a single team's roster.
///
/// Deliberately separate from [`super::auth::Capability`]: `Capability`
/// derives `GenomeSafe` and rides in the persisted `AssessmentMetadata`, so
/// adding a variant there would bump `SCHEMA_HASH`. Membership-read
/// degradation is a render-time-only concern instead (oracle adr-fmt-kqavx).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamRosterStatus {
    /// Both member and maintainer pages were fetched successfully.
    Complete,
    /// The team no longer exists on GitHub (404) — a CODEOWNERS reference
    /// to a team GitHub has deleted.
    Deleted,
    /// The GitHub API denied access to this team's membership.
    PermissionDenied,
    /// The fetch failed for a retryable/transient reason.
    TransientError,
}

/// A GitHub team's member roster, fetched fresh each collection tick (B1).
///
/// Render-time-only, mirroring [`OwnerMetrics`]: rebuilt on every render,
/// never folded into the persisted native event tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TeamRoster {
    /// Canonical owner name matching [`OwnerMetrics::owner`] (e.g. `@org/team-slug`).
    pub canonical_owner: String,
    /// GitHub team slug used in API paths (e.g. `team-slug`).
    pub team_slug: String,
    /// Whether the roster fetch completed or degraded.
    pub status: TeamRosterStatus,
    /// Team members, complete and role-tagged when `status == Complete`.
    pub members: Vec<TeamMember>,
}

/// Extract the GitHub team slug from a canonical CODEOWNERS owner string.
///
/// Returns `None` for user-type owners (no `/`), malformed input, or a
/// slug containing glob metacharacters (`* ? [ ] !`). A CODEOWNERS entry
/// like `* @org/*` is a wildcard-shaped owner, not a real GitHub team —
/// GitHub team slugs cannot contain glob metacharacters, so such a slug
/// is invalid by construction and must never reach a team-roster fetch.
/// The canonical form is `@org/team-slug` (already lowercased by
/// [`build_owner_repo_map`]'s dedup key); the API path needs only the
/// `team-slug` segment, since the org is supplied separately.
///
/// ```
/// use gh_report::domain::metrics::team_slug_from_canonical_owner;
///
/// assert_eq!(
///     team_slug_from_canonical_owner("@mattilsynet/team-foo"),
///     Some("team-foo")
/// );
/// assert_eq!(team_slug_from_canonical_owner("@individual-user"), None);
/// assert_eq!(team_slug_from_canonical_owner("@mattilsynet/*"), None);
/// ```
#[must_use]
pub fn team_slug_from_canonical_owner(canonical_owner: &str) -> Option<&str> {
    let stripped = canonical_owner.strip_prefix('@')?;
    let (_, slug) = stripped.split_once('/')?;
    (!slug.is_empty() && !slug.contains(['*', '?', '[', ']', '!'])).then_some(slug)
}

/// True when a canonical CODEOWNERS owner is a `WildcardOwner` acknowledged
/// owner anomaly (CHE-0093:R1/R4): team-shaped (contains `/`, already
/// `OwnerType::AmbiguousTeamShaped`) but its slug is not a valid GitHub team
/// slug — a glob-shaped catch-all construct such as `@org/*`, not a
/// resolvable team. Report-side derivation only; does not change
/// `OwnerType` classification (CHE-0093:R2).
#[must_use]
pub fn is_wildcard_owner(canonical_owner: &str) -> bool {
    canonical_owner.starts_with('@')
        && canonical_owner.contains('/')
        && team_slug_from_canonical_owner(canonical_owner).is_none()
}

/// Collection statistics for the run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollectionStatistics {
    /// Total number of non-archived repositories.
    pub total_repos: u32,
    /// Number of public repositories.
    pub public_repos: u32,
    /// Number of internal repositories.
    pub internal_repos: u32,
    /// Number of private repositories.
    pub private_repos: u32,
    /// Number of archived repositories (excluded from `total_repos`).
    ///
    /// Defaults to `0` for backward compatibility with evidence files
    /// produced before this field was introduced.
    #[serde(default)]
    pub archived_repos: u32,
}

/// Secret scanning observability summary across the organization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretScanningObservability {
    /// Whether org-level secret scanning alert data was successfully collected.
    pub collection_status: CollectionStatus,
    /// Human-readable reason if collection was not successful.
    pub collection_reason: Option<String>,
    /// Total number of open secret scanning alerts across the organization.
    pub total_open_secret_alerts: u32,
    /// Distribution of open alerts by age bucket (e.g., `"0-30 days"` → count).
    pub open_secret_alert_age_buckets: HashMap<String, u32>,
    /// ISO 8601 timestamp of the oldest open secret scanning alert.
    pub oldest_open_secret_alert_created_at: Option<String>,
    /// ISO 8601 timestamp of the newest open secret scanning alert.
    pub newest_open_secret_alert_created_at: Option<String>,
    /// Number of repos where per-repo alert status disagrees with org-level data.
    pub status_mismatch_count: u32,
    /// Number of repos with secret scanning enabled and alert data observable.
    pub observable_enabled_repositories: u32,
    /// Number of repos where alert observability is not possible.
    pub unobservable_repositories: u32,
}

/// Per-repository open-alert summary collected from the org-level endpoint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepoAlertSummary {
    /// Number of open secret scanning alerts for this repository.
    pub open_alert_count: u64,
    /// ISO 8601 timestamp of the oldest open alert, if any.
    pub oldest_open_alert_created_at: Option<String>,
    /// ISO 8601 timestamp of the newest open alert, if any.
    pub newest_open_alert_created_at: Option<String>,
}

/// Org-level secret-scanning alert summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgAlertSummary {
    /// Whether org-level alert collection succeeded.
    pub collection_status: CollectionStatus,
    /// Human-readable reason if collection was not successful.
    pub collection_reason: Option<String>,
    /// Per-repository alert summaries keyed by repository ID.
    pub per_repo: HashMap<String, RepoAlertSummary>,
    /// Distribution of open alerts by age bucket (e.g., `"0_7_days"` → count).
    pub open_secret_alert_age_buckets: HashMap<String, u64>,
    /// Total number of open secret scanning alerts across the organization.
    pub total_open_secret_alerts: u64,
    /// ISO 8601 timestamp of the oldest open alert across the organization.
    pub oldest_open_secret_alert_created_at: Option<String>,
    /// ISO 8601 timestamp of the newest open alert across the organization.
    pub newest_open_secret_alert_created_at: Option<String>,
}

use crate::domain::evidence::RepositoryEvidence;

/// Build a mapping from canonical (lowercase) owner name to the repositories
/// that reference that owner in their CODEOWNERS file.
///
/// Only non-archived repositories with parsed CODEOWNERS data are included.
/// The returned map keys are lowercase owner names; values are tuples of
/// `(display_name, Vec<&RepositoryEvidence>)` where `display_name` preserves
/// the first-seen casing.
///
/// This is the canonical owner→repo mapping implementation, consumed by
/// [`crate::aggregate::metrics::build_owner_metrics`] (aggregation) and
/// the owner-detail view model builder (per-repo rendering).
#[must_use]
pub fn build_owner_repo_map<'a>(
    repositories: &'a [RepositoryEvidence],
) -> HashMap<String, (String, Vec<&'a RepositoryEvidence>)> {
    let active: Vec<_> = repositories
        .iter()
        .filter(|r| !r.repository.archived)
        .collect();

    let mut owner_repos: HashMap<String, (String, Vec<&'a RepositoryEvidence>)> = HashMap::new();

    for repo in &active {
        let Some(parsed) = &repo.checks.codeowners.parsed else {
            continue;
        };

        for owner in &parsed.unique_owners {
            let key = owner.to_lowercase();
            let entry = owner_repos
                .entry(key)
                .or_insert_with(|| (owner.clone(), Vec::new()));
            entry.1.push(repo);
        }
    }

    owner_repos
}

/// List `(canonical_owner, team_slug)` pairs for every team-type owner
/// referenced by CODEOWNERS across `repositories`.
///
/// Pure and cheap (delegates to [`build_owner_repo_map`]); called once per
/// collection tick, before the team-roster fetch, to determine which teams
/// to fetch. Excludes user-type owners (no `/`) and any malformed canonical
/// string that [`team_slug_from_canonical_owner`] cannot parse.
#[must_use]
pub fn team_owner_slugs(repositories: &[RepositoryEvidence]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = build_owner_repo_map(repositories)
        .into_keys()
        .filter_map(|canonical| {
            team_slug_from_canonical_owner(&canonical)
                .map(|slug| (canonical.clone(), slug.to_string()))
        })
        .collect();
    pairs.sort();
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_metric_percentage_calculation() {
        let metric = RateMetric::new(3, 10);
        assert_eq!(metric.rate, Some(30.0));
        assert_eq!(metric.to_string(), "30.0% (3/10)");
    }

    #[test]
    fn rate_metric_zero_denominator() {
        let metric = RateMetric::new(0, 0);
        assert_eq!(metric.rate, None);
        assert_eq!(metric.to_string(), "N/A (0/0)");
    }

    #[test]
    fn rate_metric_with_extra_fields() {
        let metric = RateMetric::new(5, 20)
            .with_extra("unknown", 3)
            .with_extra("observable_repositories", 17);
        assert_eq!(metric.extra.get("unknown"), Some(&serde_json::json!(3)));
        assert_eq!(
            metric.extra.get("observable_repositories"),
            Some(&serde_json::json!(17))
        );
    }

    #[test]
    fn rate_metric_rounds_to_one_decimal() {
        let metric = RateMetric::new(1, 3);
        assert_eq!(metric.rate, Some(33.3));
    }

    #[test]
    fn team_slug_from_canonical_owner_rejects_wildcard_shaped_owner() {
        assert_eq!(team_slug_from_canonical_owner("@mattilsynet/*"), None);
    }

    #[test]
    fn team_slug_from_canonical_owner_rejects_glob_metachars_in_slug() {
        assert_eq!(team_slug_from_canonical_owner("@org/team-?"), None);
        assert_eq!(team_slug_from_canonical_owner("@org/team-[a]"), None);
        assert_eq!(team_slug_from_canonical_owner("@org/team!"), None);
    }

    #[test]
    fn team_slug_from_canonical_owner_keeps_valid_team_slug() {
        assert_eq!(
            team_slug_from_canonical_owner("@mattilsynet/real-team"),
            Some("real-team")
        );
    }

    #[test]
    fn is_wildcard_owner_true_for_glob_shaped_owner() {
        assert!(is_wildcard_owner("@org/*"));
    }

    #[test]
    fn is_wildcard_owner_false_for_real_team_owner() {
        assert!(!is_wildcard_owner("@org/real-team"));
    }

    #[test]
    fn is_wildcard_owner_false_for_user_owner() {
        assert!(!is_wildcard_owner("@individual-user"));
    }

    #[test]
    fn team_owner_slugs_drops_wildcard_shaped_owner_but_keeps_real_team() {
        use crate::domain::repository::Visibility;
        use crate::test_fixtures::{
            branch_pass, codeowners_with_owners, dependabot_enabled, make_checks,
            make_repository_evidence, policy_pass_setting, secret_enabled_observable,
        };

        let repos = vec![make_repository_evidence(
            "repo-a",
            Visibility::Public,
            false,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_with_owners(&["@mattilsynet/*", "@mattilsynet/real-team"]),
            ),
        )];

        let pairs = team_owner_slugs(&repos);

        assert_eq!(
            pairs,
            vec![(
                "@mattilsynet/real-team".to_string(),
                "real-team".to_string()
            )],
            "wildcard-shaped owner @mattilsynet/* must be dropped; real-team must be kept"
        );
    }
}
