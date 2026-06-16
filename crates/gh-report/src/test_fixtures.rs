//! Shared test fixtures for repository evidence, check results, and helper constructors.
//!
//! Consolidates duplicated test helpers from checkpoint.rs, collect.rs,
//! metrics.rs, repository.rs, and others into a single reusable module.

use crate::config;
use crate::domain::auth::{AuthMode, TokenTier};
use crate::domain::checks::{
    BranchProtectionDetails, BranchProtectionResult, BranchProtectionStatus, CodeownersResult,
    CodeownersStatus, DependabotResult, DependabotStatus, RepositoryChecks, SecretScanningResult,
    SecretScanningStatus, SecurityPolicyEvidence, SecurityPolicyResult, SecurityPolicyStatus,
};
use crate::domain::codeowners::{CodeownersEntry, ParsedCodeowners};
use crate::domain::evidence::{AssessmentMetadata, Evidence, RepositoryEvidence};
use crate::domain::metrics::{
    AggregatedMetrics, BranchProtectionCounts, CodeownersCounts, CollectionStatistics,
    DependabotCounts, PolicyCounts, RateMetric, SecretAlertCounts, SecretScanningCounts,
    SecretScanningObservability,
};
use crate::domain::repository::{Repository, Visibility};
use crate::domain::status::CollectionStatus;
use std::collections::HashMap;

/// Standard test timestamp.
#[must_use]
pub fn make_timestamp() -> String {
    "2026-04-09T12:00:00+00:00".to_string()
}

/// Create a test `Repository` domain object.
#[must_use]
pub fn make_repository(name: &str, archived: bool, visibility: Visibility) -> Repository {
    Repository {
        id: format!("id-{name}"),
        node_id: None,
        name: name.to_string(),
        visibility,
        language: None,
        default_branch: "main".to_string(),
        archived,
        has_issues: true,
        inventory_key: format!("id-{name}"),
        updated_at: None,
        pushed_at: None,
        created_at: None,
        description: None,
        fork: false,
        html_url: None,
        topics: vec![],
        license_spdx: None,
    }
}

/// Create a `RepositoryEvidence` with explicit checks.
#[must_use]
pub fn make_repository_evidence(
    name: &str,
    visibility: Visibility,
    archived: bool,
    checks: RepositoryChecks,
) -> RepositoryEvidence {
    RepositoryEvidence {
        repository: make_repository(name, archived, visibility),
        checks,
        last_commit: None,
    }
}

/// Create a `RepositoryEvidence` with all passing checks.
#[must_use]
pub fn all_passing_evidence(name: &str) -> RepositoryEvidence {
    make_repository_evidence(
        name,
        Visibility::Public,
        false,
        make_checks(
            policy_pass_setting(),
            secret_enabled_observable(false),
            dependabot_enabled(),
            branch_pass(),
            codeowners_conforming(),
        ),
    )
}

/// Create a `RepositoryEvidence` from a domain `Repository` with all passing checks.
#[must_use]
pub fn evidence_from_repository(repo: &Repository, timestamp: &str) -> RepositoryEvidence {
    RepositoryEvidence {
        repository: repo.clone(),
        checks: RepositoryChecks {
            security_policy: SecurityPolicyResult {
                status: SecurityPolicyStatus::Pass,
                evidence: SecurityPolicyEvidence::Setting,
                path: None,
                timestamp: timestamp.to_string(),
            },
            secret_scanning: SecretScanningResult {
                status: SecretScanningStatus::Enabled,
                has_open_alerts: Some(false),
                alerts_observable: true,
                reason: None,
                timestamp: timestamp.to_string(),
            },
            dependabot_security_updates: DependabotResult {
                status: DependabotStatus::Enabled,
                reason: None,
                timestamp: timestamp.to_string(),
            },
            branch_protection: BranchProtectionResult {
                status: BranchProtectionStatus::Pass,
                details: BranchProtectionDetails {
                    default_branch: repo.default_branch.clone(),
                    has_pr: Some(true),
                    required_reviewers: Some(1),
                    has_status_checks: Some(true),
                    admin_equivalent: Some(true),
                    has_broad_bypass: Some(false),
                    reason: None,
                },
                timestamp: timestamp.to_string(),
            },
            codeowners: CodeownersResult {
                status: CodeownersStatus::Conforming,
                path: Some(".github/CODEOWNERS".to_string()),
                timestamp: timestamp.to_string(),
                parsed: None,
                truncation: None,
            },
        },
        last_commit: None,
    }
}

/// Assemble a `RepositoryChecks` from individual check results.
#[must_use]
pub fn make_checks(
    policy: SecurityPolicyResult,
    secret: SecretScanningResult,
    dependabot: DependabotResult,
    branch: BranchProtectionResult,
    codeowners: CodeownersResult,
) -> RepositoryChecks {
    RepositoryChecks {
        security_policy: policy,
        secret_scanning: secret,
        dependabot_security_updates: dependabot,
        branch_protection: branch,
        codeowners,
    }
}

/// Security policy result: pass via GitHub API setting.
#[must_use]
pub fn policy_pass_setting() -> SecurityPolicyResult {
    SecurityPolicyResult {
        status: SecurityPolicyStatus::Pass,
        evidence: SecurityPolicyEvidence::Setting,
        path: None,
        timestamp: make_timestamp(),
    }
}

/// Security policy result: pass via file presence (`SECURITY.md`).
#[must_use]
pub fn policy_pass_file() -> SecurityPolicyResult {
    SecurityPolicyResult {
        status: SecurityPolicyStatus::Pass,
        evidence: SecurityPolicyEvidence::File,
        path: Some("SECURITY.md".to_string()),
        timestamp: make_timestamp(),
    }
}

/// Security policy result: fail (no policy detected).
#[must_use]
pub fn policy_fail() -> SecurityPolicyResult {
    SecurityPolicyResult {
        status: SecurityPolicyStatus::Fail,
        evidence: SecurityPolicyEvidence::Absent,
        path: None,
        timestamp: make_timestamp(),
    }
}

/// Security policy result: unknown (permission denied).
#[must_use]
pub fn policy_unknown() -> SecurityPolicyResult {
    SecurityPolicyResult {
        status: SecurityPolicyStatus::Unknown,
        evidence: SecurityPolicyEvidence::PermissionDenied,
        path: None,
        timestamp: make_timestamp(),
    }
}

/// Secret scanning result: enabled with observable alerts.
#[must_use]
pub fn secret_enabled_observable(has_open: bool) -> SecretScanningResult {
    SecretScanningResult {
        status: SecretScanningStatus::Enabled,
        has_open_alerts: Some(has_open),
        alerts_observable: true,
        reason: None,
        timestamp: make_timestamp(),
    }
}

/// Secret scanning result: disabled.
#[must_use]
pub fn secret_disabled() -> SecretScanningResult {
    SecretScanningResult {
        status: SecretScanningStatus::Disabled,
        has_open_alerts: None,
        alerts_observable: false,
        reason: None,
        timestamp: make_timestamp(),
    }
}

/// Secret scanning result: unknown (insufficient evidence).
#[must_use]
pub fn secret_unknown() -> SecretScanningResult {
    SecretScanningResult {
        status: SecretScanningStatus::Unknown,
        has_open_alerts: None,
        alerts_observable: false,
        reason: Some("insufficient_evidence".to_string()),
        timestamp: make_timestamp(),
    }
}

/// Secret scanning result: permission denied.
#[must_use]
pub fn secret_permission_denied() -> SecretScanningResult {
    SecretScanningResult {
        status: SecretScanningStatus::PermissionDenied,
        has_open_alerts: None,
        alerts_observable: false,
        reason: Some("permission_denied".to_string()),
        timestamp: make_timestamp(),
    }
}

/// Dependabot security updates result: enabled.
#[must_use]
pub fn dependabot_enabled() -> DependabotResult {
    DependabotResult {
        status: DependabotStatus::Enabled,
        reason: None,
        timestamp: make_timestamp(),
    }
}

/// Dependabot security updates result: disabled.
#[must_use]
pub fn dependabot_disabled() -> DependabotResult {
    DependabotResult {
        status: DependabotStatus::Disabled,
        reason: None,
        timestamp: make_timestamp(),
    }
}

/// Dependabot security updates result: unknown (insufficient evidence).
#[must_use]
pub fn dependabot_unknown() -> DependabotResult {
    DependabotResult {
        status: DependabotStatus::Unknown,
        reason: Some("insufficient_evidence".to_string()),
        timestamp: make_timestamp(),
    }
}

/// Branch protection result: all controls satisfied.
#[must_use]
pub fn branch_pass() -> BranchProtectionResult {
    BranchProtectionResult {
        status: BranchProtectionStatus::Pass,
        details: BranchProtectionDetails {
            default_branch: "main".to_string(),
            has_pr: Some(true),
            required_reviewers: Some(1),
            has_status_checks: Some(true),
            admin_equivalent: Some(true),
            has_broad_bypass: Some(false),
            reason: None,
        },
        timestamp: make_timestamp(),
    }
}

/// Branch protection result: some controls satisfied.
#[must_use]
pub fn branch_partial() -> BranchProtectionResult {
    BranchProtectionResult {
        status: BranchProtectionStatus::Partial,
        details: BranchProtectionDetails {
            default_branch: "main".to_string(),
            has_pr: Some(true),
            required_reviewers: Some(0),
            has_status_checks: Some(false),
            admin_equivalent: Some(false),
            has_broad_bypass: Some(false),
            reason: None,
        },
        timestamp: make_timestamp(),
    }
}

/// Branch protection result: no controls detected.
#[must_use]
pub fn branch_fail() -> BranchProtectionResult {
    BranchProtectionResult {
        status: BranchProtectionStatus::Fail,
        details: BranchProtectionDetails {
            default_branch: "main".to_string(),
            has_pr: None,
            required_reviewers: None,
            has_status_checks: None,
            admin_equivalent: None,
            has_broad_bypass: None,
            reason: None,
        },
        timestamp: make_timestamp(),
    }
}

/// Branch protection result: unknown (permission denied).
#[must_use]
pub fn branch_unknown() -> BranchProtectionResult {
    BranchProtectionResult {
        status: BranchProtectionStatus::Unknown,
        details: BranchProtectionDetails {
            default_branch: "main".to_string(),
            has_pr: None,
            required_reviewers: None,
            has_status_checks: None,
            admin_equivalent: None,
            has_broad_bypass: None,
            reason: Some("permission_denied".to_string()),
        },
        timestamp: make_timestamp(),
    }
}

/// CODEOWNERS result: file found in conforming location (`.github/CODEOWNERS`).
#[must_use]
pub fn codeowners_conforming() -> CodeownersResult {
    CodeownersResult {
        status: CodeownersStatus::Conforming,
        path: Some(".github/CODEOWNERS".to_string()),
        timestamp: make_timestamp(),
        parsed: None,
        truncation: None,
    }
}

/// CODEOWNERS result: file found in non-conforming location (repo root).
#[must_use]
pub fn codeowners_non_conforming() -> CodeownersResult {
    CodeownersResult {
        status: CodeownersStatus::NonConforming,
        path: Some("CODEOWNERS".to_string()),
        timestamp: make_timestamp(),
        parsed: None,
        truncation: None,
    }
}

/// CODEOWNERS result: no file detected.
#[must_use]
pub fn codeowners_absent() -> CodeownersResult {
    CodeownersResult {
        status: CodeownersStatus::Absent,
        path: None,
        timestamp: make_timestamp(),
        parsed: None,
        truncation: None,
    }
}

/// CODEOWNERS result: status could not be determined.
#[must_use]
pub fn codeowners_unknown() -> CodeownersResult {
    CodeownersResult {
        status: CodeownersStatus::Unknown,
        path: None,
        timestamp: make_timestamp(),
        parsed: None,
        truncation: None,
    }
}

/// CODEOWNERS result: conforming, with parsed data containing the given owners.
///
/// Builds a single entry with pattern `/src/` and the supplied `@`-prefixed owners.
#[must_use]
pub fn codeowners_with_owners(owners: &[&str]) -> CodeownersResult {
    CodeownersResult {
        status: CodeownersStatus::Conforming,
        path: Some(".github/CODEOWNERS".to_string()),
        timestamp: make_timestamp(),
        parsed: Some(ParsedCodeowners {
            entries: vec![CodeownersEntry {
                pattern: "/src/".to_string(),
                owners: owners.iter().map(ToString::to_string).collect(),
            }],
            unique_owners: owners.iter().map(ToString::to_string).collect(),
            skipped_lines: 0,
        }),
        truncation: None,
    }
}

/// Create a `RepositoryEvidence` with an explicit `updated_at` value.
///
/// Useful for lifecycle/staleness tests. Defaults to public, non-archived,
/// with passing checks and the given CODEOWNERS owners.
#[must_use]
pub fn make_repo_with_updated_at(
    name: &str,
    updated_at: Option<&str>,
    secret_scanning_enabled: bool,
    has_open_alerts: Option<bool>,
    alerts_observable: bool,
    owners: &[&str],
) -> RepositoryEvidence {
    let mut repo = make_repository_evidence(
        name,
        Visibility::Public,
        false,
        make_checks(
            policy_pass_setting(),
            if secret_scanning_enabled {
                SecretScanningResult {
                    status: SecretScanningStatus::Enabled,
                    has_open_alerts,
                    alerts_observable,
                    reason: None,
                    timestamp: make_timestamp(),
                }
            } else {
                secret_disabled()
            },
            dependabot_enabled(),
            branch_pass(),
            codeowners_with_owners(owners),
        ),
    );
    repo.repository.updated_at = updated_at.map(ToString::to_string);
    repo
}

/// Standard test metadata. Override individual fields after construction
/// when tests need specific values.
#[must_use]
pub fn make_metadata() -> AssessmentMetadata {
    AssessmentMetadata {
        date: "2026-04-09".to_string(),
        organization: "TestOrg".to_string(),
        schema_version: config::EVIDENCE_SCHEMA_VERSION.to_string(),
        run_timestamp: "2026-04-09T12:00:00+00:00".to_string(),
        run_id: "test-run-id".to_string(),
        token_tier: TokenTier::Full,
        token_scopes: "repo, read:org, security_events".to_string(),
        auth_mode: AuthMode::Pat,
        rate_limit_warnings: 0,
        unavailable_capabilities: vec![],
        inventory_fetched_at: None,
        warm_start: false,
    }
}

/// Collection statistics with explicit counts.
#[must_use]
pub fn make_collection_statistics(
    total: u32,
    public: u32,
    internal: u32,
    private: u32,
) -> CollectionStatistics {
    CollectionStatistics {
        total_repos: total,
        public_repos: public,
        internal_repos: internal,
        private_repos: private,
        archived_repos: 0,
    }
}

/// Minimal metrics where every repo passes every check (1 repo, all 1/0 counts).
/// Suitable for publish/serialization tests that don't assert on specific values.
#[must_use]
pub fn make_minimal_metrics() -> AggregatedMetrics {
    AggregatedMetrics {
        security_policy_coverage: RateMetric::new(1, 1),
        policy_counts: PolicyCounts {
            via_setting: 1,
            via_file: 0,
            unknown: 0,
            missing: 0,
        },
        secret_scanning_coverage: RateMetric::new(1, 1),
        secret_scanning_counts: SecretScanningCounts {
            enabled: 1,
            disabled: 0,
            permission_denied: 0,
            unknown: 0,
        },
        dependabot_security_updates_coverage: RateMetric::new(1, 1),
        dependabot_security_updates_counts: DependabotCounts {
            enabled: 1,
            paused: 0,
            disabled: 0,
            unknown: 0,
        },
        open_secret_alert_prevalence: RateMetric::new(0, 1),
        secret_alert_counts: SecretAlertCounts {
            repos_with_open_alerts: 0,
            repos_without_open_alerts: 1,
            unobservable: 0,
        },
        branch_protection_coverage: RateMetric::new(1, 1),
        branch_protection_counts: BranchProtectionCounts {
            pass: 1,
            partial: 0,
            fail: 0,
            unknown: 0,
        },
        codeowners_coverage: RateMetric::new(1, 1),
        codeowners_counts: CodeownersCounts {
            conforming: 1,
            non_conforming: 0,
            absent: 0,
            unknown: 0,
            truncated: 0,
        },
        owner_metrics: vec![],
    }
}

/// Default secret scanning observability.
#[must_use]
pub fn make_observability() -> SecretScanningObservability {
    SecretScanningObservability {
        collection_status: CollectionStatus::Success,
        collection_reason: None,
        total_open_secret_alerts: 0,
        open_secret_alert_age_buckets: HashMap::new(),
        oldest_open_secret_alert_created_at: None,
        newest_open_secret_alert_created_at: None,
        status_mismatch_count: 0,
        observable_enabled_repositories: 1,
        unobservable_repositories: 0,
    }
}

/// Assemble a complete `Evidence` from its components.
#[must_use]
pub fn make_full_evidence(
    metadata: AssessmentMetadata,
    stats: CollectionStatistics,
    metrics: AggregatedMetrics,
    observability: SecretScanningObservability,
    repos: Vec<RepositoryEvidence>,
) -> Evidence {
    Evidence {
        assessment_metadata: metadata,
        collection_statistics: stats,
        metrics,
        secret_scanning_observability: observability,
        repositories: repos,
    }
}
