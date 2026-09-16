//! Evidence artifact types produced by collection runs.

use serde::{Deserialize, Serialize};

use crate::projection::DeletedRepoRecord;

use super::auth::{AuthMode, Capability, TokenTier};
use super::checks::RepositoryChecks;
use super::metrics::{
    AggregatedMetrics, CollectionStatistics, OrgAlertSummary, SecretScanningObservability,
};
use super::repository::Repository;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepositoryReadState {
    Pending,
    Failed,
    Observed,
}

impl RepositoryReadState {
    pub(crate) const PENDING_REASON: &str = "pending";

    pub(crate) fn from_checks(checks: &RepositoryChecks) -> Self {
        let has_observation = Self::observed_check_timestamps(checks)
            .into_iter()
            .any(|at| at.is_some());
        let pending = [
            checks.secret_scanning.reason.as_deref(),
            checks.dependabot_security_updates.reason.as_deref(),
            checks.branch_protection.details.reason.as_deref(),
        ]
        .into_iter()
        .all(|reason| reason == Some(Self::PENDING_REASON));
        match (has_observation, pending) {
            (true, _) => Self::Observed,
            (false, true) => Self::Pending,
            (false, false) => Self::Failed,
        }
    }

    pub(crate) fn observed_check_timestamps(checks: &RepositoryChecks) -> [Option<&str>; 5] {
        use super::checks::{
            BranchProtectionStatus, CodeownersStatus, DependabotStatus, SecretScanningStatus,
            SecurityPolicyStatus,
        };
        [
            matches!(
                checks.security_policy.status,
                SecurityPolicyStatus::Pass | SecurityPolicyStatus::Fail
            )
            .then_some(checks.security_policy.timestamp.as_str()),
            matches!(
                checks.secret_scanning.status,
                SecretScanningStatus::Enabled | SecretScanningStatus::Disabled
            )
            .then_some(checks.secret_scanning.timestamp.as_str()),
            (checks.dependabot_security_updates.status != DependabotStatus::Unknown)
                .then_some(checks.dependabot_security_updates.timestamp.as_str()),
            (checks.branch_protection.status != BranchProtectionStatus::Unknown)
                .then_some(checks.branch_protection.timestamp.as_str()),
            (checks.codeowners.status != CodeownersStatus::Unknown)
                .then_some(checks.codeowners.timestamp.as_str()),
        ]
    }
}

/// Information about the most recent commit on a repository's default branch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LastCommitInfo {
    /// GitHub login of the committer (e.g., `"octocat"`), if available.
    pub committer_login: Option<String>,
    /// Display name of the committer from the git commit object.
    pub committer_name: Option<String>,
    /// ISO 8601 timestamp of the commit.
    pub commit_date: Option<String>,
}

/// A repository with its collected check results (evidence).
///
/// `repository` is owned: PGN-0013:R8 excludes shared-ownership wrappers from
/// event fields because `GenomeSafe` closes under bounded field types, not under
/// runtime sharing. Consumers that need to fan out the same
/// `Repository` across async tasks wrap with `Arc::new(evidence.repository)`
/// at the call site; cross-snapshot sharing semantics (CHE-0048) are
/// preserved by those runtime Arcs, not by the field type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepositoryEvidence {
    /// The repository this evidence pertains to.
    pub repository: Repository,
    /// Aggregated security check results for the repository.
    pub checks: RepositoryChecks,
    /// Information about the most recent commit on the default branch.
    /// `None` when the data could not be collected (API error, empty repo, etc.).
    pub last_commit: Option<LastCommitInfo>,
    /// Whether this evidence's `repo_details` call resolved via a 304
    /// not-modified `ETag` revalidation rather than a fresh, quota-consuming
    /// fetch (ghr-fe9bb970 / CHE-0055:R17). `#[serde(default)]` keeps
    /// deserialization of pre-existing persisted evidence additive-safe.
    #[serde(default)]
    pub repo_details_not_modified: bool,
}

impl RepositoryEvidence {
    /// True when every check resolved to a definite status (own-scope
    /// completeness concept for the repository fold, CHE-0092:R1/R4 —
    /// a fresh `Unknown`-free observation is this fold's analogue of
    /// team's `TeamRosterStatus::Complete`).
    #[must_use]
    pub fn is_complete(&self) -> bool {
        use super::checks::{
            BranchProtectionStatus, CodeownersStatus, DependabotStatus, SecretScanningStatus,
            SecurityPolicyStatus,
        };
        self.checks.security_policy.status != SecurityPolicyStatus::Unknown
            && self.checks.secret_scanning.status != SecretScanningStatus::Unknown
            && self.checks.dependabot_security_updates.status != DependabotStatus::Unknown
            && self.checks.branch_protection.status != BranchProtectionStatus::Unknown
            && self.checks.codeowners.status != CodeownersStatus::Unknown
    }
}

/// Assessment metadata for a collection run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssessmentMetadata {
    /// Date of the collection run (YYYY-MM-DD).
    pub date: String,
    /// Target GitHub organization name.
    pub organization: String,
    /// Evidence schema version used for this run.
    pub schema_version: String,
    /// ISO 8601 timestamp of when the run started.
    pub run_timestamp: String,
    /// Unique identifier for this collection run.
    pub run_id: String,
    /// Token capability tier based on available OAuth scopes.
    pub token_tier: TokenTier,
    /// Comma-separated list of OAuth scopes, or `"not-available"`.
    pub token_scopes: String,
    /// Authentication mode used for API calls.
    pub auth_mode: AuthMode,
    /// Number of rate-limit warnings encountered during the run.
    pub rate_limit_warnings: u32,
    /// Capabilities that were unavailable during this run.
    pub unavailable_capabilities: Vec<Capability>,
    /// ISO 8601 timestamp of when `build_inventory_from_api()` completed.
    ///
    /// Provides observability into the baseline TOCTOU staleness window:
    /// `updated_at` is fetched at inventory time; a repo could change between
    /// inventory and evaluation. For large orgs this window can be minutes
    /// to hours.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inventory_fetched_at: Option<String>,
    /// Whether this evidence was rendered from a cached baseline (warm-start)
    /// rather than a fresh API collection.
    #[serde(default)]
    pub warm_start: bool,
    /// Coverage bounds and selection status for organization repository collection.
    #[serde(default)]
    pub coverage: CollectionCoverage,
}

/// Coverage bounds and selection status for organization repository collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CollectionCoverage {
    /// Coverage status is unknown.
    #[default]
    Unknown,
    /// Known coverage assessment with validated bounds.
    Known {
        /// Total unique repositories discovered in the organization.
        total: usize,
        /// Maximum repository admission cap applied.
        limit: std::num::NonZeroU64,
    },
}

impl CollectionCoverage {
    /// Construct a known coverage assessment.
    #[must_use]
    pub const fn known(total: usize, limit: std::num::NonZeroU64) -> Self {
        Self::Known { total, limit }
    }

    /// Whether this coverage is unknown.
    #[must_use]
    pub const fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }

    /// Whether this coverage is known.
    #[must_use]
    pub const fn is_known(&self) -> bool {
        matches!(self, Self::Known { .. })
    }

    /// Number of repositories selected/admitted for evaluation after applying cap.
    #[must_use]
    pub fn selected(&self) -> Option<usize> {
        match self {
            Self::Unknown => None,
            Self::Known { total, limit } => {
                let limit_val = usize::try_from(limit.get()).unwrap_or(usize::MAX);
                Some((*total).min(limit_val))
            }
        }
    }

    /// Whether repository collection was capped by `max_repos`.
    #[must_use]
    pub fn is_capped(&self) -> bool {
        match self {
            Self::Unknown => false,
            Self::Known { total, limit } => {
                let limit_val = usize::try_from(limit.get()).unwrap_or(usize::MAX);
                *total > limit_val
            }
        }
    }

    /// Total number of unique repositories discovered in the organization.
    #[must_use]
    pub const fn total(&self) -> Option<usize> {
        match self {
            Self::Unknown => None,
            Self::Known { total, .. } => Some(*total),
        }
    }

    /// Maximum repositories cap limit applied.
    #[must_use]
    pub const fn limit(&self) -> Option<std::num::NonZeroU64> {
        match self {
            Self::Unknown => None,
            Self::Known { limit, .. } => Some(*limit),
        }
    }
}

impl Serialize for CollectionCoverage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        match self {
            Self::Unknown => {
                let mut s = serializer.serialize_struct("CollectionCoverage", 1)?;
                s.serialize_field("status", "unknown")?;
                s.end()
            }
            Self::Known { total, limit } => {
                let limit_val = usize::try_from(limit.get()).unwrap_or(usize::MAX);
                let mut s = serializer.serialize_struct("CollectionCoverage", 5)?;
                s.serialize_field("status", "known")?;
                s.serialize_field("total", total)?;
                s.serialize_field("limit", limit)?;
                s.serialize_field("selected", &(*total).min(limit_val))?;
                s.serialize_field("capped", &(*total > limit_val))?;
                s.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for CollectionCoverage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum KnownTag {
            Known,
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum UnknownTag {
            Unknown,
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawKnownCoverage {
            #[allow(dead_code, reason = "tag validation")]
            status: KnownTag,
            total: usize,
            limit: std::num::NonZeroU64,
            selected: Option<usize>,
            capped: Option<bool>,
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawUnknownCoverage {
            #[allow(dead_code, reason = "tag validation")]
            status: UnknownTag,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RawCoverageTagged {
            Known(RawKnownCoverage),
            Unknown(RawUnknownCoverage),
        }

        match RawCoverageTagged::deserialize(deserializer)? {
            RawCoverageTagged::Unknown(_) => Ok(Self::Unknown),
            RawCoverageTagged::Known(known) => {
                let total = known.total;
                let limit = known.limit;
                let limit_val = usize::try_from(limit.get()).unwrap_or(usize::MAX);
                let derived_selected = total.min(limit_val);
                let derived_capped = total > limit_val;
                if let Some(selected) = known.selected
                    && selected != derived_selected
                {
                    return Err(serde::de::Error::custom(format!(
                        "contradictory coverage selected: provided {selected}, expected min(total, limit) = {derived_selected}"
                    )));
                }
                if let Some(capped) = known.capped
                    && capped != derived_capped
                {
                    return Err(serde::de::Error::custom(format!(
                        "contradictory coverage capped: provided {capped}, expected (total > limit) = {derived_capped}"
                    )));
                }
                Ok(Self::Known { total, limit })
            }
        }
    }
}

/// Organization-scope durable snapshot payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgStateSnapshot {
    /// Number of archived repositories observed at org scope.
    pub archived_repos: u32,
    /// Metadata for the collection run that produced this snapshot.
    pub assessment_metadata: AssessmentMetadata,
    /// Organization-level secret-scanning alert summary.
    pub alert_summary: OrgAlertSummary,
}

/// Complete evidence artifact for a collection run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Evidence {
    /// Metadata about the collection run (date, auth, schema version).
    pub assessment_metadata: AssessmentMetadata,
    /// Repository count breakdown by visibility.
    pub collection_statistics: CollectionStatistics,
    /// Aggregated security metrics across all non-archived repositories.
    pub metrics: AggregatedMetrics,
    /// Organization-level secret scanning observability summary.
    pub secret_scanning_observability: SecretScanningObservability,
    /// Per-repository evidence with check results.
    pub repositories: Vec<RepositoryEvidence>,
    /// Pruned records for repositories detected as deleted.
    #[serde(default)]
    pub deleted: Vec<DeletedRepoRecord>,
}

#[cfg(test)]
mod tests {
    use crate::collector::inventory::InventoryPayload;
    use crate::config;
    use crate::domain::repository::Visibility;
    use crate::test_fixtures;
    use insta::assert_json_snapshot;

    #[test]
    fn snapshot_assessment_metadata() {
        let metadata = test_fixtures::make_metadata();
        assert_json_snapshot!(metadata);
    }

    #[test]
    fn snapshot_secret_scanning_observability() {
        let observability = test_fixtures::make_observability();
        assert_json_snapshot!(observability);
    }

    #[test]
    fn snapshot_repository_evidence() {
        let evidence = test_fixtures::all_passing_evidence("snapshot-repo");
        assert_json_snapshot!(evidence);
    }

    #[test]
    fn snapshot_inventory_payload() {
        let repo = test_fixtures::make_repository("snap-repo", false, Visibility::Private);
        let payload = InventoryPayload {
            schema_version: config::INVENTORY_SCHEMA_VERSION.to_string(),
            organization: "TestOrg".to_string(),
            generated_at: "2026-04-09T12:00:00+00:00".to_string(),
            repositories: vec![repo],
            complete: true,
            inventory_fetched_at: None,
        };
        assert_json_snapshot!(payload);
    }

    #[test]
    fn snapshot_full_evidence() {
        let evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            test_fixtures::make_collection_statistics(1, 1, 0, 0),
            test_fixtures::make_minimal_metrics(),
            test_fixtures::make_observability(),
            vec![test_fixtures::all_passing_evidence("snap-repo")],
        );
        assert_json_snapshot!(evidence);
    }

    /// Backward compat: a serialized payload containing `warm_start: true`
    /// deserializes correctly into the current `AssessmentMetadata` struct.
    /// This guards against regressions if the field's `#[serde(default)]`
    /// attribute is accidentally removed.
    #[test]
    fn backward_compat_warm_start_present() {
        use super::AssessmentMetadata;

        let metadata = AssessmentMetadata {
            date: "2026-04-15".to_string(),
            organization: "TestOrg".to_string(),
            schema_version: config::EVIDENCE_SCHEMA_VERSION.to_string(),
            run_timestamp: "2026-04-15T12:00:00+00:00".to_string(),
            run_id: "compat-test".to_string(),
            token_tier: crate::domain::auth::TokenTier::Full,
            token_scopes: "repo".to_string(),
            auth_mode: crate::domain::auth::AuthMode::Pat,
            rate_limit_warnings: 0,
            unavailable_capabilities: vec![],
            inventory_fetched_at: None,
            warm_start: true,
            coverage: super::CollectionCoverage::default(),
        };

        let encoded = serde_json::to_vec(&metadata).expect("serialize");

        let decoded: AssessmentMetadata = serde_json::from_slice(&encoded).expect("deserialize");

        assert!(decoded.warm_start);
        assert_eq!(decoded.organization, "TestOrg");
        assert_eq!(decoded.run_id, "compat-test");
    }

    /// Forward compat: a serialized payload *without* the `warm_start`
    /// field deserializes successfully. This simulates reading a baseline
    /// written by a future binary that has removed the field, or an old
    /// baseline from before the field was added. The `#[serde(default)]`
    /// attribute ensures `warm_start` defaults to `false`.
    #[test]
    fn forward_compat_warm_start_absent() {
        let metadata = test_fixtures::make_metadata();
        let mut json_val = serde_json::to_value(&metadata).expect("to json");
        let obj = json_val.as_object_mut().expect("object");
        obj.remove("warm_start");

        let encoded = serde_json::to_vec(&json_val).expect("to json bytes");

        let decoded: super::AssessmentMetadata =
            serde_json::from_slice(&encoded).expect("deserialize without warm_start");

        assert!(
            !decoded.warm_start,
            "warm_start should default to false when absent"
        );
        assert_eq!(decoded.organization, "TestOrg");
    }

    /// Extra-field compat: a serialized payload with an *unknown* extra
    /// field deserializes successfully. This guards the assumption that
    /// `AssessmentMetadata` does not use `#[serde(deny_unknown_fields)]`.
    #[test]
    fn ignores_unknown_fields() {
        let metadata = test_fixtures::make_metadata();
        let mut json_val = serde_json::to_value(&metadata).expect("to json");
        let obj = json_val.as_object_mut().expect("object");
        obj.insert(
            "future_field".to_string(),
            serde_json::Value::String("hello".to_string()),
        );

        let encoded = serde_json::to_vec(&json_val).expect("to json bytes");

        let decoded: super::AssessmentMetadata =
            serde_json::from_slice(&encoded).expect("deserialize with unknown field");

        assert_eq!(decoded.organization, "TestOrg");
        assert_eq!(decoded.run_id, "test-run-id");
    }

    #[test]
    fn collection_coverage_known_computes_selected_and_capped() {
        let cov = super::CollectionCoverage::known(776, std::num::NonZeroU64::new(10).unwrap());
        assert!(!cov.is_unknown());
        assert_eq!(cov.total(), Some(776));
        assert_eq!(cov.limit().map(std::num::NonZero::get), Some(10));
        assert_eq!(cov.selected(), Some(10));
        assert!(cov.is_capped());

        let uncapped = super::CollectionCoverage::known(5, std::num::NonZeroU64::new(10).unwrap());
        assert_eq!(uncapped.selected(), Some(5));
        assert!(!uncapped.is_capped());

        let exact = super::CollectionCoverage::known(10, std::num::NonZeroU64::new(10).unwrap());
        assert_eq!(exact.selected(), Some(10));
        assert!(!exact.is_capped());

        let unknown = super::CollectionCoverage::Unknown;
        assert!(unknown.is_unknown());
        assert_eq!(unknown.selected(), None);
        assert!(!unknown.is_capped());
        assert_eq!(unknown.total(), None);
        assert_eq!(unknown.limit(), None);
    }

    #[test]
    fn collection_coverage_serde_roundtrip() {
        let cov = super::CollectionCoverage::known(100, std::num::NonZeroU64::new(20).unwrap());
        let json = serde_json::to_string(&cov).expect("serialize");
        let decoded: super::CollectionCoverage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, cov);

        let unk = super::CollectionCoverage::Unknown;
        let json_unk = serde_json::to_string(&unk).expect("serialize unknown");
        let decoded_unk: super::CollectionCoverage =
            serde_json::from_str(&json_unk).expect("deserialize unknown");
        assert_eq!(decoded_unk, unk);
    }

    #[test]
    fn collection_coverage_rejects_contradictory_selected_deserialize() {
        let contradictory_json = r#"{
            "status": "known",
            "total": 100,
            "limit": 20,
            "selected": 999,
            "capped": true
        }"#;
        let result: Result<super::CollectionCoverage, _> = serde_json::from_str(contradictory_json);
        assert!(result.is_err());
    }

    #[test]
    fn collection_coverage_rejects_contradictory_capped_deserialize() {
        let contradictory_json = r#"{
            "status": "known",
            "total": 100,
            "limit": 20,
            "selected": 20,
            "capped": false
        }"#;
        let result: Result<super::CollectionCoverage, _> = serde_json::from_str(contradictory_json);
        assert!(result.is_err());
    }

    #[test]
    fn collection_coverage_rejects_zero_limit_deserialize() {
        let zero_limit_json = r#"{
            "status": "known",
            "total": 100,
            "limit": 0
        }"#;
        let result: Result<super::CollectionCoverage, _> = serde_json::from_str(zero_limit_json);
        assert!(result.is_err());
    }

    #[test]
    fn collection_coverage_rejects_statusless_deserialize() {
        let json = r#"{"total": 100, "selected": 999}"#;
        assert!(serde_json::from_str::<super::CollectionCoverage>(json).is_err());
    }

    #[test]
    fn collection_coverage_rejects_unknown_status_with_bounds() {
        let json = r#"{"status":"unknown","total":100,"limit":20,"selected":999,"capped":false}"#;
        assert!(serde_json::from_str::<super::CollectionCoverage>(json).is_err());
    }

    #[test]
    fn collection_coverage_rejects_known_missing_total_or_limit() {
        let json = r#"{"status":"known","total":100}"#;
        assert!(serde_json::from_str::<super::CollectionCoverage>(json).is_err());
        let json2 = r#"{"status":"known","limit":20}"#;
        assert!(serde_json::from_str::<super::CollectionCoverage>(json2).is_err());
    }

    #[test]
    fn collection_coverage_rejects_extra_fields() {
        let json = r#"{"status":"known","total":100,"limit":20,"unexpected":"field"}"#;
        assert!(serde_json::from_str::<super::CollectionCoverage>(json).is_err());
        let json2 = r#"{"status":"unknown","unexpected":"field"}"#;
        assert!(serde_json::from_str::<super::CollectionCoverage>(json2).is_err());
    }
}
