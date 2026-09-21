//! Shared test fixtures for repository evidence, check results, and helper constructors.
//!
//! Consolidates duplicated test helpers from checkpoint.rs, collect.rs,
//! metrics.rs, repository.rs, and others into a single reusable module.

use crate::config;
use crate::domain::checks::{
    DisabledObservation, EnabledProvenance, ProbeSource, SecretScanningAlerts,
    SecretScanningFailureReason,
};

#[must_use]
pub fn secret_for_status(
    status: SecretScanningStatus,
    open: Option<bool>,
    reason: Option<&str>,
) -> SecretScanningResult {
    let supplied_reason = reason;
    let reason = match reason {
        Some("permission_denied" | "alerts_permission_denied") => {
            SecretScanningFailureReason::PermissionDenied
        }
        Some("transient_error" | "alerts_transient_error") => {
            SecretScanningFailureReason::Transient
        }
        Some("pending") => SecretScanningFailureReason::Pending,
        Some("collection_error") => SecretScanningFailureReason::Invalid,
        Some("alerts_unavailable") => SecretScanningFailureReason::Unavailable,
        Some("alert_coordinate_conflict") => SecretScanningFailureReason::Conflict,
        _ => SecretScanningFailureReason::InsufficientEvidence,
    };
    let source = ProbeSource::PerRepoEndpoint;
    let http_status = None;
    match status {
        SecretScanningStatus::Enabled => SecretScanningResult::Enabled {
            provenance: EnabledProvenance::Metadata {
                http_status,
                alerts: match open {
                    Some(has_open_alerts) => SecretScanningAlerts::Observable {
                        source,
                        has_open_alerts,
                        http_status,
                    },
                    None => SecretScanningAlerts::Unobservable {
                        source,
                        reason,
                        http_status,
                    },
                },
            },
            timestamp: make_timestamp(),
        },
        SecretScanningStatus::Disabled => SecretScanningResult::Disabled {
            metadata_http_status: None,
            observation: match (open, supplied_reason) {
                (Some(true), _) | (_, Some("status_mismatch")) => {
                    DisabledObservation::StatusMismatch {
                        source,
                        http_status,
                    }
                }
                (_, Some(_)) => DisabledObservation::ProbeFailed {
                    source,
                    reason,
                    http_status,
                },
                _ => DisabledObservation::NoMismatch {
                    source,
                    http_status,
                },
            },
            timestamp: make_timestamp(),
        },
        SecretScanningStatus::PermissionDenied => SecretScanningResult::unobservable(
            SecretScanningFailureReason::PermissionDenied,
            make_timestamp(),
        ),
        SecretScanningStatus::Unknown => {
            SecretScanningResult::unobservable(reason, make_timestamp())
        }
    }
}
use crate::domain::auth::{AuthMode, TokenTier};
use crate::domain::checks::{
    BranchProtectionDetails, BranchProtectionResult, BranchProtectionStatus, CodeownersContent,
    CodeownersNonConformingLocation, CodeownersResult, CodeownersStatus, DependabotResult,
    DependabotStatus, IndeterminateReason, RepositoryChecks, SecretScanningResult,
    SecretScanningStatus, SecurityPolicyPath, SecurityPolicyResult,
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
        is_empty: false,
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
        repo_details_not_modified: false,
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
            security_policy: SecurityPolicyResult::EnabledBySetting {
                timestamp: timestamp.to_string(),
            },
            secret_scanning: secret_enabled_observable(false).with_timestamp(timestamp),
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
                    reason_kind: None,
                    http_status: None,
                    force_push_blocked: Some(true),
                    deletion_blocked: Some(true),
                },
                timestamp: timestamp.to_string(),
            },
            codeowners: CodeownersResult::Conforming {
                content: CodeownersContent::Unparsed,
                timestamp: timestamp.to_string(),
            },
        },
        last_commit: None,
        repo_details_not_modified: false,
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
    SecurityPolicyResult::EnabledBySetting {
        timestamp: make_timestamp(),
    }
}

/// Security policy result: pass via file presence (`SECURITY.md`).
///
/// # Panics
/// Panics if the fixed nonempty fixture path is invalid.
#[must_use]
pub fn policy_pass_file() -> SecurityPolicyResult {
    SecurityPolicyResult::EnabledByFile {
        path: SecurityPolicyPath::new("SECURITY.md").unwrap(),
        timestamp: make_timestamp(),
    }
}

/// Security policy result: fail (no policy detected).
#[must_use]
pub fn policy_fail() -> SecurityPolicyResult {
    SecurityPolicyResult::Absent {
        timestamp: make_timestamp(),
    }
}

/// Security policy result: unknown (permission denied).
#[must_use]
pub fn policy_unknown() -> SecurityPolicyResult {
    SecurityPolicyResult::Unobservable {
        reason: IndeterminateReason::PermissionDenied,
        timestamp: make_timestamp(),
    }
}

/// Secret scanning result: enabled with observable alerts.
#[must_use]
pub fn secret_enabled_observable(has_open: bool) -> SecretScanningResult {
    secret_for_status(SecretScanningStatus::Enabled, Some(has_open), None)
}

/// Secret scanning result: disabled.
#[must_use]
pub fn secret_disabled() -> SecretScanningResult {
    secret_for_status(SecretScanningStatus::Disabled, None, None)
}

/// Secret scanning result: unknown (insufficient evidence).
#[must_use]
pub fn secret_unknown() -> SecretScanningResult {
    secret_for_status(SecretScanningStatus::Unknown, None, None)
}

/// Secret scanning result: permission denied.
#[must_use]
pub fn secret_permission_denied() -> SecretScanningResult {
    secret_for_status(
        SecretScanningStatus::PermissionDenied,
        None,
        Some("permission_denied"),
    )
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
            reason_kind: None,
            http_status: None,
            force_push_blocked: Some(true),
            deletion_blocked: Some(true),
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
            reason_kind: None,
            http_status: None,
            force_push_blocked: Some(true),
            deletion_blocked: Some(true),
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
            reason_kind: None,
            http_status: None,
            force_push_blocked: None,
            deletion_blocked: None,
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
            reason_kind: Some(crate::domain::checks::CollectionFailureReason::PermissionDenied),
            http_status: Some(403),
            force_push_blocked: None,
            deletion_blocked: None,
        },
        timestamp: make_timestamp(),
    }
}

/// CODEOWNERS result: file found in conforming location (`.github/CODEOWNERS`).
#[must_use]
pub fn codeowners_conforming() -> CodeownersResult {
    CodeownersResult::Conforming {
        content: CodeownersContent::Unparsed,
        timestamp: make_timestamp(),
    }
}

/// CODEOWNERS result: file found in non-conforming location (repo root).
#[must_use]
pub fn codeowners_non_conforming() -> CodeownersResult {
    CodeownersResult::NonConforming {
        location: CodeownersNonConformingLocation::Root,
        content: CodeownersContent::Unparsed,
        timestamp: make_timestamp(),
    }
}

/// CODEOWNERS result: no file detected.
#[must_use]
pub fn codeowners_absent() -> CodeownersResult {
    CodeownersResult::Absent {
        timestamp: make_timestamp(),
    }
}

/// CODEOWNERS result: status could not be determined.
#[must_use]
pub fn codeowners_unknown() -> CodeownersResult {
    CodeownersResult::Unobservable {
        reason: IndeterminateReason::Pending,
        timestamp: make_timestamp(),
    }
}

/// CODEOWNERS result: conforming, with parsed data containing the given owners.
///
/// Builds a single entry with pattern `/src/` and the supplied `@`-prefixed owners.
#[must_use]
pub fn codeowners_with_owners(owners: &[&str]) -> CodeownersResult {
    CodeownersResult::Conforming {
        timestamp: make_timestamp(),
        content: CodeownersContent::Parsed(ParsedCodeowners {
            entries: vec![CodeownersEntry {
                pattern: "/src/".to_string(),
                owners: owners.iter().map(ToString::to_string).collect(),
            }],
            unique_owners: owners.iter().map(ToString::to_string).collect(),
            skipped_lines: 0,
        }),
    }
}

#[must_use]
pub fn codeowners_for_status(status: CodeownersStatus) -> CodeownersResult {
    match status {
        CodeownersStatus::Conforming => codeowners_conforming(),
        CodeownersStatus::NonConforming => codeowners_non_conforming(),
        CodeownersStatus::Absent => codeowners_absent(),
        CodeownersStatus::Unknown => codeowners_unknown(),
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
                secret_for_status(
                    SecretScanningStatus::Enabled,
                    has_open_alerts.filter(|_| alerts_observable),
                    None,
                )
            } else {
                secret_disabled()
            },
            dependabot_enabled(),
            branch_pass(),
            codeowners_with_owners(owners),
        ),
    );
    repo.repository.updated_at = updated_at.and_then(crate::domain::repository::UpdatedAt::new);
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
        coverage: crate::domain::evidence::CollectionCoverage::default(),
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
        collection_health_counts: vec![],
        score_exclusion_counts: vec![],
        team_rosters: vec![],
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
        deleted: vec![],
    }
}
