//! Aggregated metrics computed from per-repository check results.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::domain::checks::CollectionFailureReason;
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
    /// Repos at branch-protection T2 or higher.
    pub pass: u32,
    /// Repos at T1 minimal baseline.
    pub partial: u32,
    /// Repos at T0 below baseline.
    pub fail: u32,
    /// Repos excluded because branch protection could not be scored.
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
}

/// Owner type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerType {
    /// A team (e.g., `@org/team-name` — contains `/`).
    Team,
    /// An individual user (e.g., `@username` — no `/`).
    User,
}

impl std::fmt::Display for OwnerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Team => f.write_str("Team"),
            Self::User => f.write_str("User"),
        }
    }
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
}
