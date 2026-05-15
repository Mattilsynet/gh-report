//! Evidence artifact types produced by collection runs.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::auth::{AuthMode, Capability, TokenTier};
use super::checks::RepositoryChecks;
use super::metrics::{AggregatedMetrics, CollectionStatistics, SecretScanningObservability};
use super::repository::Repository;

/// Information about the most recent commit on a repository's default branch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LastCommitInfo {
    /// GitHub login of the committer (e.g., `"octocat"`), if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub committer_login: Option<String>,
    /// Display name of the committer from the git commit object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub committer_name: Option<String>,
    /// ISO 8601 timestamp of the commit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_date: Option<String>,
}

/// A repository with its collected check results (evidence).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepositoryEvidence {
    /// The repository this evidence pertains to.
    ///
    /// Wrapped in `Arc` to avoid deep-cloning the full `Repository` struct
    /// when evidence is shared across checkpoint snapshots, partial
    /// publisher, and baseline caches.  Transparent to serde thanks to
    /// `serde(features = ["rc"])`.
    pub repository: Arc<Repository>,
    /// Aggregated security check results for the repository.
    pub checks: RepositoryChecks,
    /// Information about the most recent commit on the default branch.
    /// `None` when the data could not be collected (API error, empty repo, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_commit: Option<LastCommitInfo>,
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

    // ── Baseline backward/forward compatibility tests ───────────

    /// Backward compat: a `MessagePack` payload containing `warm_start: true`
    /// deserializes correctly into the current `AssessmentMetadata` struct.
    /// This guards against regressions if the field's `#[serde(default)]`
    /// attribute is accidentally removed.
    #[test]
    fn msgpack_backward_compat_warm_start_present() {
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
        };

        // Serialize with named fields (same as baseline.msgpack).
        let encoded = rmp_serde::to_vec_named(&metadata).expect("serialize");

        // Deserialize back — warm_start should be preserved.
        let decoded: AssessmentMetadata = rmp_serde::from_slice(&encoded).expect("deserialize");

        assert!(decoded.warm_start);
        assert_eq!(decoded.organization, "TestOrg");
        assert_eq!(decoded.run_id, "compat-test");
    }

    /// Forward compat: a `MessagePack` payload *without* the `warm_start`
    /// field deserializes successfully. This simulates reading a baseline
    /// written by a future binary that has removed the field, or an old
    /// baseline from before the field was added. The `#[serde(default)]`
    /// attribute ensures `warm_start` defaults to `false`.
    #[test]
    fn msgpack_forward_compat_warm_start_absent() {
        // We can't easily build a partial msgpack map with rmp_serde's
        // typed API, so we use serde_json as an intermediate: serialize
        // the full struct, remove the field, then round-trip through
        // MessagePack.
        let metadata = test_fixtures::make_metadata();
        let mut json_val = serde_json::to_value(&metadata).expect("to json");
        let obj = json_val.as_object_mut().expect("object");
        obj.remove("warm_start");

        // Re-serialize to MessagePack named format via serde_json::Value.
        let msgpack = rmp_serde::to_vec_named(&json_val).expect("to msgpack");

        // Deserialize into AssessmentMetadata — warm_start should default to false.
        let decoded: super::AssessmentMetadata =
            rmp_serde::from_slice(&msgpack).expect("deserialize without warm_start");

        assert!(
            !decoded.warm_start,
            "warm_start should default to false when absent"
        );
        assert_eq!(decoded.organization, "TestOrg");
    }

    /// Extra-field compat: a `MessagePack` payload with an *unknown* extra
    /// field deserializes successfully. This guards the assumption that
    /// `AssessmentMetadata` does not use `#[serde(deny_unknown_fields)]`.
    #[test]
    fn msgpack_ignores_unknown_fields() {
        let metadata = test_fixtures::make_metadata();
        let mut json_val = serde_json::to_value(&metadata).expect("to json");
        let obj = json_val.as_object_mut().expect("object");
        obj.insert(
            "future_field".to_string(),
            serde_json::Value::String("hello".to_string()),
        );

        let msgpack = rmp_serde::to_vec_named(&json_val).expect("to msgpack");

        let decoded: super::AssessmentMetadata =
            rmp_serde::from_slice(&msgpack).expect("deserialize with unknown field");

        assert_eq!(decoded.organization, "TestOrg");
        assert_eq!(decoded.run_id, "test-run-id");
    }
}
