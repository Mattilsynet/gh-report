//! Metrics aggregation and collection statistics.
//!
//! Computes aggregated security metrics across repository evidence.
//!
//! All `u32` counts use saturating arithmetic to avoid panics or silent
//! wrapping when counts are near `u32::MAX` (e.g., from defensive
//! `u32::try_from(...).unwrap_or(u32::MAX)` fallbacks).

use std::collections::{HashMap, HashSet};

use tracing::warn;

use crate::config;
use crate::domain::checks::{
    BranchProtectionTier, CodeownersStatus, CollectionFailureReason, DependabotStatus,
    ExclusionReason, ScoreCategory, SecretScanningStatus, SecurityPolicyEvidence,
    SecurityPolicyStatus,
};
use crate::domain::evidence::RepositoryEvidence;
use crate::domain::metrics::{
    AggregatedMetrics, BranchProtectionCounts, CodeownersCounts, CollectionHealthCheckKind,
    CollectionHealthCount, CollectionStatistics, DependabotCounts, OrgAlertSummary, OwnerMetrics,
    OwnerType, PolicyCounts, RateMetric, RepoAlertSummary, ScoreExclusionCount, SecretAlertCounts,
    SecretScanningCounts, SecretScanningObservability,
};
use crate::domain::repository::Visibility;
use crate::domain::status::CollectionStatus;

/// Safely convert a `usize` count to `u32`, saturating at `u32::MAX`.
fn count_as_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Compute collection statistics from repository evidence.
///
/// `total_repos` and the per-visibility breakdowns count only non-archived
/// repositories. `archived_repos` is derived from the input slice — any
/// archived [`RepositoryEvidence`] entries surfaced by the projection (or
/// by replay during warm-start) are reflected directly. Live collections
/// use the same event-derived repository slice, so the rendered count is
/// always a projection fold rather than an inventory-side aggregate.
#[must_use]
pub fn build_collection_statistics(repositories: &[RepositoryEvidence]) -> CollectionStatistics {
    let active: Vec<_> = repositories
        .iter()
        .filter(|r| !r.repository.archived)
        .collect();

    let archived_count = count_as_u32(
        repositories
            .iter()
            .filter(|r| r.repository.archived)
            .count(),
    );

    CollectionStatistics {
        total_repos: count_as_u32(active.len()),
        public_repos: count_by_visibility(&active, Visibility::Public),
        internal_repos: count_by_visibility(&active, Visibility::Internal),
        private_repos: count_by_visibility(&active, Visibility::Private),
        archived_repos: archived_count,
    }
}

/// Count active repositories with a specific visibility.
///
/// Uses the `Visibility` enum for compile-time–safe matching (no string
/// typos can silently produce zero counts).
fn count_by_visibility(active: &[&RepositoryEvidence], visibility: Visibility) -> u32 {
    count_as_u32(
        active
            .iter()
            .filter(|r| r.repository.visibility == visibility)
            .count(),
    )
}

/// Aggregate security metrics across repository evidence.
///
/// Aggregation semantics (this is the org-page half of the per-metric
/// population matrix; the owner-page half lives on
/// `build_per_control_coverage`'s doc comment — single source for both:
/// bd bead `adr-fmt-5dfp2`):
///
/// - **Security policy**: counted over **all public** repositories,
///   including archived ones. Denominator is total public repos
///   (archived + active). Archived public repos with a security policy
///   on file are intentionally included so the count reflects the
///   organisation's published policy surface, not just its active one.
/// - **Secret scanning**: counted over **all public** repositories, including
///   archived ones, mirroring the security-policy denominator.
/// - **Dependabot, branch protection, CODEOWNERS**: counted over **all**
///   non-archived repos. Denominator is total active repos.
/// - **Open secret alert prevalence**: denominator is the number of repos
///   where secret scanning is enabled AND alerts are observable.
#[must_use]
pub fn aggregate_metrics(repositories: &[RepositoryEvidence]) -> AggregatedMetrics {
    let active: Vec<_> = repositories
        .iter()
        .filter(|r| !r.repository.archived)
        .collect();
    let public_policy: Vec<_> = repositories
        .iter()
        .filter(|r| r.repository.visibility == Visibility::Public)
        .collect();

    let policy_counts = count_policy_statuses(&public_policy);
    let secret_counts = count_secret_scanning_statuses(&public_policy);
    let dependabot_counts = count_dependabot_statuses(&active);
    let branch_counts = count_branch_protection_statuses(&active);
    let codeowners_counts = count_codeowners_statuses(&active);
    let secret_alert_counts = count_secret_alert_observability(&active);

    let taxonomy = count_collection_health_reasons(&active);
    let alert_observable_enabled = count_alert_observable_enabled(&active);
    let policy_tally = exclusion_tally_from_policy(&policy_counts);
    let secret_tally = exclusion_tally_from_secret_scanning(&secret_counts);
    let dependabot_tally = exclusion_tally_from_dependabot(&dependabot_counts);
    let branch_protection_exclusions = count_branch_protection_exclusion_reasons(&active);
    let codeowners_tally = exclusion_tally_from_codeowners(&codeowners_counts);
    let coverage = build_coverage_metrics(CoverageInputs {
        policy_counts: &policy_counts,
        secret_counts: &secret_counts,
        dependabot_counts: &dependabot_counts,
        branch_counts: &branch_counts,
        codeowners_counts: &codeowners_counts,
        secret_alert_counts: &secret_alert_counts,
        taxonomy: &taxonomy,
        alert_observable_enabled,
        policy_tally: &policy_tally,
        secret_tally: &secret_tally,
        dependabot_tally: &dependabot_tally,
        branch_tally: &branch_protection_exclusions,
        codeowners_tally: &codeowners_tally,
    });

    let mut score_exclusion_counts = Vec::new();
    policy_tally.push_counts(
        CollectionHealthCheckKind::SecurityPolicy,
        &mut score_exclusion_counts,
    );
    secret_tally.push_counts(
        CollectionHealthCheckKind::SecretScanning,
        &mut score_exclusion_counts,
    );
    dependabot_tally.push_counts(
        CollectionHealthCheckKind::Dependabot,
        &mut score_exclusion_counts,
    );
    branch_protection_exclusions.push_counts(
        CollectionHealthCheckKind::BranchProtection,
        &mut score_exclusion_counts,
    );
    codeowners_tally.push_counts(
        CollectionHealthCheckKind::Codeowners,
        &mut score_exclusion_counts,
    );

    AggregatedMetrics {
        security_policy_coverage: coverage.security_policy,
        policy_counts,
        secret_scanning_coverage: coverage.secret_scanning,
        secret_scanning_counts: secret_counts,
        dependabot_security_updates_coverage: coverage.dependabot,
        dependabot_security_updates_counts: dependabot_counts,
        open_secret_alert_prevalence: coverage.open_secret_alerts,
        secret_alert_counts,
        branch_protection_coverage: coverage.branch_protection,
        branch_protection_counts: branch_counts,
        codeowners_coverage: coverage.codeowners,
        codeowners_counts,
        owner_metrics: build_owner_metrics(repositories),
        collection_health_counts: taxonomy.into_counts(),
        score_exclusion_counts,
        team_rosters: Vec::new(),
    }
}

#[derive(Clone, Copy)]
struct CoverageInputs<'a> {
    policy_counts: &'a PolicyCounts,
    secret_counts: &'a SecretScanningCounts,
    dependabot_counts: &'a DependabotCounts,
    branch_counts: &'a BranchProtectionCounts,
    codeowners_counts: &'a CodeownersCounts,
    secret_alert_counts: &'a SecretAlertCounts,
    taxonomy: &'a CollectionHealthTaxonomyCounts,
    alert_observable_enabled: u32,
    policy_tally: &'a ExclusionTally,
    secret_tally: &'a ExclusionTally,
    dependabot_tally: &'a ExclusionTally,
    branch_tally: &'a ExclusionTally,
    codeowners_tally: &'a ExclusionTally,
}

#[derive(Debug, Default, Clone, Copy)]
struct ExclusionTally {
    permission_denied: u32,
    unknown: u32,
    not_applicable: u32,
    other: u32,
}

impl ExclusionTally {
    /// Whether the tally contains at least one repo excluded for a
    /// measurement-failure reason (`PermissionDenied`/`Unknown`/`Other`), as
    /// opposed to `NotApplicable` or an empty tally. An applicable-but-
    /// unmeasured control must be scored as a genuine failure rather than
    /// vanish as "no observable population" (UF2-5 clause 6, bd
    /// `adr-fmt-m1s6p`).
    fn has_measurement_failure(&self) -> bool {
        self.permission_denied > 0 || self.unknown > 0 || self.other > 0
    }

    fn record(&mut self, reason: ExclusionReason) {
        match reason {
            ExclusionReason::PermissionDenied => {
                self.permission_denied = self.permission_denied.saturating_add(1);
            }
            ExclusionReason::Unknown => self.unknown = self.unknown.saturating_add(1),
            ExclusionReason::NotApplicable => {
                self.not_applicable = self.not_applicable.saturating_add(1);
            }
            ExclusionReason::Other => self.other = self.other.saturating_add(1),
        }
    }

    fn push_counts(
        self,
        check_kind: CollectionHealthCheckKind,
        out: &mut Vec<ScoreExclusionCount>,
    ) {
        for (reason, count) in [
            (ExclusionReason::PermissionDenied, self.permission_denied),
            (ExclusionReason::Unknown, self.unknown),
            (ExclusionReason::NotApplicable, self.not_applicable),
            (ExclusionReason::Other, self.other),
        ] {
            if count > 0 {
                out.push(ScoreExclusionCount {
                    check_kind,
                    reason,
                    count,
                });
            }
        }
    }
}

/// Build a coverage `RateMetric` with a reason-aware floor for the
/// all-excluded case (UF2-5 clause 6, bd `adr-fmt-m1s6p`).
///
/// When `pass + fail == 0` (nothing observable) AND `tally` records a
/// measurement failure, the control is applicable but could not be
/// measured — it must enter the health-score product as a genuine `0.0`
/// (`RateMetric::new(0, 1)`), not vanish as `None`. When `pass + fail == 0`
/// and the tally is empty or all-`NotApplicable`, there is no observable
/// population and `None` is the correct, legitimate result. When
/// `pass + fail > 0`, behaviour is unchanged.
fn coverage_metric(pass: u32, fail: u32, tally: &ExclusionTally) -> RateMetric {
    let denominator = pass.saturating_add(fail);
    if denominator == 0 && tally.has_measurement_failure() {
        RateMetric::new(0, 1)
    } else {
        RateMetric::new(pass, denominator)
    }
}

fn exclusion_tally_from_policy(counts: &PolicyCounts) -> ExclusionTally {
    ExclusionTally {
        unknown: counts.unknown,
        ..ExclusionTally::default()
    }
}

fn exclusion_tally_from_secret_scanning(counts: &SecretScanningCounts) -> ExclusionTally {
    ExclusionTally {
        permission_denied: counts.permission_denied,
        unknown: counts.unknown,
        ..ExclusionTally::default()
    }
}

fn exclusion_tally_from_dependabot(counts: &DependabotCounts) -> ExclusionTally {
    ExclusionTally {
        unknown: counts.unknown,
        ..ExclusionTally::default()
    }
}

fn exclusion_tally_from_codeowners(counts: &CodeownersCounts) -> ExclusionTally {
    ExclusionTally {
        unknown: counts.unknown,
        ..ExclusionTally::default()
    }
}

fn count_branch_protection_exclusion_reasons(active: &[&RepositoryEvidence]) -> ExclusionTally {
    let mut tally = ExclusionTally::default();
    for repo in active {
        if let ScoreCategory::Excluded(reason) = repo.checks.branch_protection.score_category() {
            tally.record(reason);
        }
    }
    tally
}

struct CoverageMetrics {
    security_policy: RateMetric,
    secret_scanning: RateMetric,
    dependabot: RateMetric,
    open_secret_alerts: RateMetric,
    branch_protection: RateMetric,
    codeowners: RateMetric,
}

fn build_coverage_metrics(input: CoverageInputs<'_>) -> CoverageMetrics {
    let policy_pass = input
        .policy_counts
        .via_setting
        .saturating_add(input.policy_counts.via_file);
    let policy_observable = policy_pass.saturating_add(input.policy_counts.missing);
    CoverageMetrics {
        security_policy: coverage_metric(
            policy_pass,
            input.policy_counts.missing,
            input.policy_tally,
        )
        .with_extra("observable_repositories", policy_observable)
        .with_extra("unknown", input.policy_counts.unknown),
        secret_scanning: secret_scanning_coverage(input),
        dependabot: dependabot_coverage(input),
        open_secret_alerts: open_secret_alert_prevalence(input),
        branch_protection: branch_protection_coverage(input),
        codeowners: codeowners_coverage(input),
    }
}

fn secret_scanning_coverage(input: CoverageInputs<'_>) -> RateMetric {
    let observable = input
        .secret_counts
        .enabled
        .saturating_add(input.secret_counts.disabled);
    coverage_metric(
        input.secret_counts.enabled,
        input.secret_counts.disabled,
        input.secret_tally,
    )
    .with_extra("disabled", input.secret_counts.disabled)
    .with_extra("permission_denied", input.secret_counts.permission_denied)
    .with_extra("unknown", input.secret_counts.unknown)
    .with_extra("observable_repositories", observable)
    .with_extra(
        "collection_health_secret_scanning_permission_denied",
        input.taxonomy.secret_scanning_permission_denied,
    )
}

fn dependabot_coverage(input: CoverageInputs<'_>) -> RateMetric {
    let non_enabled = input
        .dependabot_counts
        .paused
        .saturating_add(input.dependabot_counts.disabled);
    let observable = input.dependabot_counts.enabled.saturating_add(non_enabled);
    coverage_metric(
        input.dependabot_counts.enabled,
        non_enabled,
        input.dependabot_tally,
    )
    .with_extra("paused", input.dependabot_counts.paused)
    .with_extra("disabled", input.dependabot_counts.disabled)
    .with_extra("unknown", input.dependabot_counts.unknown)
    .with_extra("observable_repositories", observable)
}

fn open_secret_alert_prevalence(input: CoverageInputs<'_>) -> RateMetric {
    RateMetric::new(
        input.secret_alert_counts.repos_with_open_alerts,
        input.alert_observable_enabled,
    )
    .with_extra(
        "repos_without_open_alerts",
        input.secret_alert_counts.repos_without_open_alerts,
    )
    .with_extra("unobservable", input.secret_alert_counts.unobservable)
}

fn branch_protection_coverage(input: CoverageInputs<'_>) -> RateMetric {
    let non_pass = input
        .branch_counts
        .partial
        .saturating_add(input.branch_counts.fail);
    let observable = input.branch_counts.pass.saturating_add(non_pass);
    coverage_metric(input.branch_counts.pass, non_pass, input.branch_tally)
        .with_extra("insufficient", non_pass)
        .with_extra("unknown", input.branch_counts.unknown)
        .with_extra("observable_repositories", observable)
        .with_extra(
            "collection_health_branch_protection_permission_suspected",
            input.taxonomy.branch_protection_permission_suspected,
        )
        .with_extra(
            "collection_health_branch_protection_not_found_absent",
            input.taxonomy.branch_protection_not_found_absent,
        )
}

fn codeowners_coverage(input: CoverageInputs<'_>) -> RateMetric {
    let codeowners_present = input
        .codeowners_counts
        .conforming
        .saturating_add(input.codeowners_counts.non_conforming);
    let observable = codeowners_present.saturating_add(input.codeowners_counts.absent);
    coverage_metric(
        codeowners_present,
        input.codeowners_counts.absent,
        input.codeowners_tally,
    )
    .with_extra("non_conforming", input.codeowners_counts.non_conforming)
    .with_extra("absent", input.codeowners_counts.absent)
    .with_extra("unknown", input.codeowners_counts.unknown)
    .with_extra("truncated", input.codeowners_counts.truncated)
    .with_extra("observable_repositories", observable)
}

/// Count security policy statuses across public repos.
///
/// Bucket assignment logic:
/// - pass + evidence=setting → `via_setting`
/// - pass + evidence=file → `via_file`
/// - fail → `missing`
/// - unknown → `unknown`
fn count_policy_statuses(public_repos: &[&RepositoryEvidence]) -> PolicyCounts {
    let mut counts = PolicyCounts::default();
    for repo in public_repos {
        let policy = &repo.checks.security_policy;
        match policy.status {
            SecurityPolicyStatus::Pass => {
                if policy.evidence == SecurityPolicyEvidence::Setting {
                    counts.via_setting = counts.via_setting.saturating_add(1);
                } else {
                    counts.via_file = counts.via_file.saturating_add(1);
                }
            }
            SecurityPolicyStatus::Fail => counts.missing = counts.missing.saturating_add(1),
            SecurityPolicyStatus::Unknown => counts.unknown = counts.unknown.saturating_add(1),
            SecurityPolicyStatus::NotApplicable => {
                debug_assert!(
                    false,
                    "SecurityPolicyStatus::NotApplicable observed in public-repo metrics path; \
                     caller is supposed to filter to public repos only"
                );
                warn!(
                    status = "not_applicable",
                    "unexpected status in public-repo metrics — counting as unknown"
                );
                counts.unknown = counts.unknown.saturating_add(1);
            }
        }
    }
    counts
}

/// Fold over repositories and accumulate status counts.
///
/// Extracts the common "init-default, iterate, classify, return" pattern
/// used by every per-control counting function (except `count_policy_statuses`
/// which has nested sub-bucketing).
fn count_statuses<T: Default>(
    repos: &[&RepositoryEvidence],
    classify: impl Fn(&RepositoryEvidence, &mut T),
) -> T {
    let mut counts = T::default();
    for repo in repos {
        classify(repo, &mut counts);
    }
    counts
}

/// Count secret scanning statuses across public repos.
fn count_secret_scanning_statuses(public_repos: &[&RepositoryEvidence]) -> SecretScanningCounts {
    count_statuses(
        public_repos,
        |repo, counts: &mut SecretScanningCounts| match repo.checks.secret_scanning.status {
            SecretScanningStatus::Enabled => counts.enabled = counts.enabled.saturating_add(1),
            SecretScanningStatus::Disabled => counts.disabled = counts.disabled.saturating_add(1),
            SecretScanningStatus::PermissionDenied => {
                counts.permission_denied = counts.permission_denied.saturating_add(1);
            }
            SecretScanningStatus::Unknown => counts.unknown = counts.unknown.saturating_add(1),
        },
    )
}

/// Count Dependabot security updates statuses across active repos.
fn count_dependabot_statuses(active: &[&RepositoryEvidence]) -> DependabotCounts {
    count_statuses(active, |repo, counts: &mut DependabotCounts| {
        match repo.checks.dependabot_security_updates.status {
            DependabotStatus::Enabled => counts.enabled = counts.enabled.saturating_add(1),
            DependabotStatus::Paused => counts.paused = counts.paused.saturating_add(1),
            DependabotStatus::Disabled => counts.disabled = counts.disabled.saturating_add(1),
            DependabotStatus::Unknown => counts.unknown = counts.unknown.saturating_add(1),
        }
    })
}

#[derive(Debug, Default)]
struct CollectionHealthTaxonomyCounts {
    secret_scanning_permission_denied: u32,
    branch_protection_permission_suspected: u32,
    branch_protection_not_found_absent: u32,
    branch_protection_permission_denied: u32,
    branch_protection_transient: u32,
    branch_protection_rate_limited: u32,
    branch_protection_invalid: u32,
}

impl CollectionHealthTaxonomyCounts {
    fn into_counts(self) -> Vec<CollectionHealthCount> {
        let mut counts = Vec::new();
        push_collection_health_count(
            &mut counts,
            CollectionHealthCheckKind::SecretScanning,
            CollectionFailureReason::PermissionDenied,
            self.secret_scanning_permission_denied,
        );
        push_collection_health_count(
            &mut counts,
            CollectionHealthCheckKind::BranchProtection,
            CollectionFailureReason::PermissionSuspected,
            self.branch_protection_permission_suspected,
        );
        push_collection_health_count(
            &mut counts,
            CollectionHealthCheckKind::BranchProtection,
            CollectionFailureReason::NotFoundAbsent,
            self.branch_protection_not_found_absent,
        );
        push_collection_health_count(
            &mut counts,
            CollectionHealthCheckKind::BranchProtection,
            CollectionFailureReason::PermissionDenied,
            self.branch_protection_permission_denied,
        );
        push_collection_health_count(
            &mut counts,
            CollectionHealthCheckKind::BranchProtection,
            CollectionFailureReason::Transient,
            self.branch_protection_transient,
        );
        push_collection_health_count(
            &mut counts,
            CollectionHealthCheckKind::BranchProtection,
            CollectionFailureReason::RateLimited,
            self.branch_protection_rate_limited,
        );
        push_collection_health_count(
            &mut counts,
            CollectionHealthCheckKind::BranchProtection,
            CollectionFailureReason::Invalid,
            self.branch_protection_invalid,
        );
        counts.sort_by_key(|entry| (entry.check_kind, entry.reason));
        counts
    }
}

fn push_collection_health_count(
    counts: &mut Vec<CollectionHealthCount>,
    check_kind: CollectionHealthCheckKind,
    reason: CollectionFailureReason,
    count: u32,
) {
    if count > 0 {
        counts.push(CollectionHealthCount {
            check_kind,
            reason,
            count,
        });
    }
}

fn count_collection_health_reasons(
    active: &[&RepositoryEvidence],
) -> CollectionHealthTaxonomyCounts {
    let mut counts = CollectionHealthTaxonomyCounts::default();
    for repo in active {
        if repo.checks.secret_scanning.status == SecretScanningStatus::PermissionDenied {
            counts.secret_scanning_permission_denied =
                counts.secret_scanning_permission_denied.saturating_add(1);
        }
        match repo.checks.branch_protection.details.reason_kind {
            Some(CollectionFailureReason::PermissionSuspected) => {
                counts.branch_protection_permission_suspected = counts
                    .branch_protection_permission_suspected
                    .saturating_add(1);
            }
            Some(CollectionFailureReason::NotFoundAbsent) => {
                counts.branch_protection_not_found_absent =
                    counts.branch_protection_not_found_absent.saturating_add(1);
            }
            Some(CollectionFailureReason::PermissionDenied) => {
                counts.branch_protection_permission_denied =
                    counts.branch_protection_permission_denied.saturating_add(1);
            }
            Some(CollectionFailureReason::Transient) => {
                counts.branch_protection_transient =
                    counts.branch_protection_transient.saturating_add(1);
            }
            Some(CollectionFailureReason::RateLimited) => {
                counts.branch_protection_rate_limited =
                    counts.branch_protection_rate_limited.saturating_add(1);
            }
            Some(CollectionFailureReason::Invalid) => {
                counts.branch_protection_invalid =
                    counts.branch_protection_invalid.saturating_add(1);
            }
            None => {}
        }
    }
    counts
}

/// Count branch protection statuses across active repos.
fn count_branch_protection_statuses(active: &[&RepositoryEvidence]) -> BranchProtectionCounts {
    count_statuses(
        active,
        |repo, counts: &mut BranchProtectionCounts| match repo.checks.branch_protection.tier() {
            BranchProtectionTier::AcceptBar | BranchProtectionTier::Bonus => {
                counts.pass = counts.pass.saturating_add(1);
            }
            BranchProtectionTier::Minimal => counts.partial = counts.partial.saturating_add(1),
            BranchProtectionTier::BelowBaseline => counts.fail = counts.fail.saturating_add(1),
            BranchProtectionTier::Excluded => counts.unknown = counts.unknown.saturating_add(1),
        },
    )
}

/// Count CODEOWNERS statuses across active repos.
fn count_codeowners_statuses(active: &[&RepositoryEvidence]) -> CodeownersCounts {
    count_statuses(active, |repo, counts: &mut CodeownersCounts| {
        match repo.checks.codeowners.status {
            CodeownersStatus::Conforming => {
                counts.conforming = counts.conforming.saturating_add(1);
            }
            CodeownersStatus::NonConforming => {
                counts.non_conforming = counts.non_conforming.saturating_add(1);
            }
            CodeownersStatus::Absent => counts.absent = counts.absent.saturating_add(1),
            CodeownersStatus::Unknown => counts.unknown = counts.unknown.saturating_add(1),
        }
        if repo.checks.codeowners.truncation.is_some() {
            counts.truncated = counts.truncated.saturating_add(1);
        }
    })
}

/// Count secret alert observability buckets across active repos.
///
/// **Invariant.** Numerator and denominator share the same gating predicate
/// (`status == Enabled && alerts_observable`); the gate is enforced upstream
/// in `collector::ghas_scanning::build_result` via `debug_assert!`. We
/// duplicate the `status == Enabled` check here defensively so the metric
/// stays internally consistent (numerator ≤ denominator) even if a future
/// evaluator change accidentally violates the invariant in production.
///
/// When the gate passes but `has_open_alerts` is `None` (unknown), the repo
/// is counted as "without open alerts" — unknown alert status defaults to
/// the negative case.
fn count_secret_alert_observability(active: &[&RepositoryEvidence]) -> SecretAlertCounts {
    let mut counts = SecretAlertCounts::default();
    for repo in active {
        let ss = &repo.checks.secret_scanning;
        if ss.alerts_observable && ss.status == SecretScanningStatus::Enabled {
            if ss.has_open_alerts == Some(true) {
                counts.repos_with_open_alerts = counts.repos_with_open_alerts.saturating_add(1);
            } else {
                counts.repos_without_open_alerts =
                    counts.repos_without_open_alerts.saturating_add(1);
            }
        } else {
            counts.unobservable = counts.unobservable.saturating_add(1);
        }
    }
    counts
}

/// Count repos where secret scanning is enabled AND alerts are observable.
///
/// This is the denominator for the open secret alert prevalence metric.
/// The numerator (`count_secret_alert_observability`) shares the same gate
/// (`status == Enabled && alerts_observable`); the invariant
/// `alerts_observable ⇒ status == Enabled` is enforced at construction time
/// in `collector::ghas_scanning::build_result`. The duplicated `status == Enabled`
/// check on both sides is defensive: if the invariant ever drifts in production
/// (where `debug_assert!` is compiled out), numerator ≤ denominator still holds.
fn count_alert_observable_enabled(active: &[&RepositoryEvidence]) -> u32 {
    count_as_u32(
        active
            .iter()
            .filter(|r| {
                r.checks.secret_scanning.status == SecretScanningStatus::Enabled
                    && r.checks.secret_scanning.alerts_observable
            })
            .count(),
    )
}

use crate::domain::metrics::build_owner_repo_map;

/// Build per-owner metrics from CODEOWNERS parsed data.
///
/// Iterates all non-archived repositories, collects owner references from
/// parsed CODEOWNERS data, and computes per-control pass rates for each owner.
///
/// Owner names are normalized to lowercase for deduplication, but the
/// first-seen casing is preserved as the display name.
#[must_use]
pub fn build_owner_metrics(repositories: &[RepositoryEvidence]) -> Vec<OwnerMetrics> {
    let owner_repos = build_owner_repo_map(repositories);

    let mut result: Vec<OwnerMetrics> = owner_repos
        .into_iter()
        .map(|(canonical, (display_name, repos))| {
            let total_repos = count_as_u32(repos.len());
            let coverage = build_per_control_coverage(&repos);
            let owner_type = if canonical.contains('/') {
                OwnerType::Team
            } else {
                OwnerType::User
            };

            OwnerMetrics {
                owner: canonical,
                display_name,
                owner_type,
                total_repos,
                per_control_coverage: coverage.per_control_coverage,
                score_exclusion_counts: coverage.score_exclusion_counts,
                in_org: None,
            }
        })
        .collect();

    result.sort_by(|a, b| {
        let type_rank = |t: &OwnerType| match t {
            OwnerType::Team => 0,
            OwnerType::User => 1,
        };
        type_rank(&a.owner_type)
            .cmp(&type_rank(&b.owner_type))
            .then_with(|| a.owner.cmp(&b.owner))
    });

    result
}

/// Enrich owner metrics with lifecycle-based control rates.
///
/// Computes two additional per-control coverage rates for each owner
/// and inserts them into `per_control_coverage`:
///
/// - **`non_stale`** — percentage of the owner's repos that are *not* stale
///   (i.e., `updated_at` is within [`STALE_THRESHOLD_DAYS`] of
///   `run_timestamp`, or `updated_at` is `None`). Denominator = total repos.
/// - **`alert_free`** — percentage of the owner's *observable* repos that
///   have no open secret scanning alerts. Observable = secret scanning
///   enabled *and* alerts observable. Denominator = observable repos
///   (zero denominator → N/A via [`RateMetric::new`]).
///
/// # Contract
///
/// - Must be called **after** [`aggregate_metrics`] (which invokes
///   [`build_owner_metrics`] and populates `owner_metrics`).
/// - Mutates `per_control_coverage` in-place; idempotent (overwrites on
///   repeated calls).
/// - Logs a warning if either key already exists (indicates an upstream
///   change that should be investigated).
///
/// [`STALE_THRESHOLD_DAYS`]: crate::domain::time::STALE_THRESHOLD_DAYS
pub(crate) fn enrich_owner_metrics_with_lifecycle(
    owners: &mut [OwnerMetrics],
    repositories: &[RepositoryEvidence],
    run_timestamp: &str,
) {
    use crate::domain::time::is_repo_stale;

    let owner_repo_map = build_owner_repo_map(repositories);

    for owner in owners.iter_mut() {
        let Some((_, repos)) = owner_repo_map.get(&owner.owner) else {
            continue;
        };

        let total = count_as_u32(repos.len());

        let stale_count = count_as_u32(
            repos
                .iter()
                .filter(|r| is_repo_stale(r.repository.updated_at.as_deref(), run_timestamp))
                .count(),
        );
        let non_stale_count = total.saturating_sub(stale_count);
        let non_stale = RateMetric::new(non_stale_count, total);

        let observable: Vec<_> = repos
            .iter()
            .filter(|r| {
                r.checks.secret_scanning.status
                    == crate::domain::checks::SecretScanningStatus::Enabled
                    && r.checks.secret_scanning.alerts_observable
            })
            .collect();
        let observable_count = count_as_u32(observable.len());
        let alert_free_count = count_as_u32(
            observable
                .iter()
                .filter(|r| r.checks.secret_scanning.has_open_alerts != Some(true))
                .count(),
        );
        let alert_free = RateMetric::new(alert_free_count, observable_count);

        for (key, metric) in [("non_stale", non_stale), ("alert_free", alert_free)] {
            if owner.per_control_coverage.contains_key(key) {
                warn!(
                    owner = %owner.owner,
                    control = key,
                    "per_control_coverage already contains key — overwriting"
                );
            }
            owner.per_control_coverage.insert(key.to_string(), metric);
        }
    }
}

/// Cross-check each individual-user owner's login against the org-members
/// set, setting [`OwnerMetrics::in_org`] in place (item9 Part B).
///
/// Only `OwnerType::User` owners are checked — team-type owners' canonical
/// name (`@org/team-slug`) isn't a login, so team membership (already
/// handled separately via [`crate::collector::team_membership::enrich_team_rosters_with_org_membership`])
/// is the relevant check there, not org membership of the owner string
/// itself; team-type owners are left at `in_org: None`.
///
/// `org_members` is `None` when the org-members fetch was unfetched or
/// degraded — every user-type owner's `in_org` is set to `None` in that
/// case (no flag on missing data). When `Some`, both sides of the
/// comparison are lowercased (`alice` in the set matches owner `@Alice`).
///
/// # Contract
///
/// Must be called **after** [`aggregate_metrics`] (which invokes
/// [`build_owner_metrics`] and populates `owner_metrics`). Mutates
/// `in_org` in-place; idempotent (overwrites on repeated calls).
pub(crate) fn enrich_owner_metrics_with_org_membership(
    owners: &mut [OwnerMetrics],
    org_members: Option<&HashSet<String>>,
) {
    for owner in owners.iter_mut() {
        if owner.owner_type != OwnerType::User {
            continue;
        }
        let login = owner
            .owner
            .strip_prefix('@')
            .unwrap_or(owner.owner.as_str())
            .to_lowercase();
        owner.in_org = org_members.map(|set| set.contains(&login));
    }
}

/// Result of [`build_per_control_coverage`]: pass-rate metrics for the 5
/// shared controls, plus the report-side by-reason exclusion breakdown.
struct OwnerControlCoverage {
    per_control_coverage: HashMap<String, RateMetric>,
    score_exclusion_counts: Vec<ScoreExclusionCount>,
}

/// Build per-control pass-rate metrics for a set of repositories, converged
/// to the same exclusion model `compute_repo_score` and the org-level
/// aggregate (item6-02, bd bead `adr-fmt-6mi2t`) already use: each repo's
/// control classifies via the shared `ScoreCategory` funnel (item6-01).
/// `Pass`/`Fail` count toward the rate; `Excluded(reason)` drops out of both
/// numerator and denominator and is tallied by reason into
/// `score_exclusion_counts`.
///
/// - **`security_policy`**: `Pass`/`Fail` per status; `Unknown` and
///   `NotApplicable` both exclude. No separate visibility filter — non-public
///   repos structurally classify `NotApplicable` (see
///   `collector::security_policy::evaluate`), so they drop out without one.
/// - **`secret_scanning`**: population is the owner's **public** repos only
///   (UF2-7, unchanged from before this fix), mirroring the org-page
///   population; within that population, `PermissionDenied`/`Unknown` exclude.
/// - **`dependabot_security_updates`**: `Pass`/`Fail` per status; `Unknown` excludes.
/// - **`branch_protection`**: uses `BranchProtectionResult::score_category()`
///   (tier-based); an unreadable/permission-suspected repo excludes instead of
///   folding into T0 fail (pcoqb, mirrored from item6-02's org-level fix).
/// - **`codeowners`**: numerator stays `Conforming` OR `NonConforming` (file
///   present) — deliberately NOT narrowed to `Conforming`-only; that
///   numerator-semantics axis is tracked separately in bd bead
///   `adr-fmt-pptla`. Only `Unknown` excludes from the denominator.
///
/// Single source for the population-scope doc across org/owner: bd bead
/// `adr-fmt-5dfp2`.
fn build_per_control_coverage(repos: &[&RepositoryEvidence]) -> OwnerControlCoverage {
    let mut sp_pass = 0u32;
    let mut sp_fail = 0u32;
    let mut sp_tally = ExclusionTally::default();

    let mut secret_pass = 0u32;
    let mut secret_fail = 0u32;
    let mut secret_tally = ExclusionTally::default();

    let mut db_pass = 0u32;
    let mut db_fail = 0u32;
    let mut db_tally = ExclusionTally::default();

    let mut bp_pass = 0u32;
    let mut bp_fail = 0u32;
    let mut bp_tally = ExclusionTally::default();

    let mut co_present = 0u32;
    let mut co_absent = 0u32;
    let mut co_tally = ExclusionTally::default();

    for repo in repos {
        match ScoreCategory::from(repo.checks.security_policy.status) {
            ScoreCategory::Pass => sp_pass = sp_pass.saturating_add(1),
            ScoreCategory::Fail => sp_fail = sp_fail.saturating_add(1),
            ScoreCategory::Excluded(reason) => sp_tally.record(reason),
        }

        if repo.repository.visibility == Visibility::Public {
            match ScoreCategory::from(repo.checks.secret_scanning.status) {
                ScoreCategory::Pass => secret_pass = secret_pass.saturating_add(1),
                ScoreCategory::Fail => secret_fail = secret_fail.saturating_add(1),
                ScoreCategory::Excluded(reason) => secret_tally.record(reason),
            }
        }

        match ScoreCategory::from(repo.checks.dependabot_security_updates.status) {
            ScoreCategory::Pass => db_pass = db_pass.saturating_add(1),
            ScoreCategory::Fail => db_fail = db_fail.saturating_add(1),
            ScoreCategory::Excluded(reason) => db_tally.record(reason),
        }

        match repo.checks.branch_protection.score_category() {
            ScoreCategory::Pass => bp_pass = bp_pass.saturating_add(1),
            ScoreCategory::Fail => bp_fail = bp_fail.saturating_add(1),
            ScoreCategory::Excluded(reason) => bp_tally.record(reason),
        }

        let codeowners_status = repo.checks.codeowners.status;
        match ScoreCategory::from(codeowners_status) {
            ScoreCategory::Excluded(reason) => co_tally.record(reason),
            ScoreCategory::Pass | ScoreCategory::Fail => {
                if matches!(
                    codeowners_status,
                    CodeownersStatus::Conforming | CodeownersStatus::NonConforming
                ) {
                    co_present = co_present.saturating_add(1);
                } else {
                    co_absent = co_absent.saturating_add(1);
                }
            }
        }
    }

    let mut per_control_coverage = HashMap::new();
    per_control_coverage.insert(
        "security_policy".to_string(),
        coverage_metric(sp_pass, sp_fail, &sp_tally),
    );
    per_control_coverage.insert(
        "secret_scanning".to_string(),
        coverage_metric(secret_pass, secret_fail, &secret_tally),
    );
    per_control_coverage.insert(
        "dependabot_security_updates".to_string(),
        coverage_metric(db_pass, db_fail, &db_tally),
    );
    per_control_coverage.insert(
        "branch_protection".to_string(),
        coverage_metric(bp_pass, bp_fail, &bp_tally),
    );
    per_control_coverage.insert(
        "codeowners".to_string(),
        coverage_metric(co_present, co_absent, &co_tally),
    );

    let mut score_exclusion_counts = Vec::new();
    sp_tally.push_counts(
        CollectionHealthCheckKind::SecurityPolicy,
        &mut score_exclusion_counts,
    );
    secret_tally.push_counts(
        CollectionHealthCheckKind::SecretScanning,
        &mut score_exclusion_counts,
    );
    db_tally.push_counts(
        CollectionHealthCheckKind::Dependabot,
        &mut score_exclusion_counts,
    );
    bp_tally.push_counts(
        CollectionHealthCheckKind::BranchProtection,
        &mut score_exclusion_counts,
    );
    co_tally.push_counts(
        CollectionHealthCheckKind::Codeowners,
        &mut score_exclusion_counts,
    );

    OwnerControlCoverage {
        per_control_coverage,
        score_exclusion_counts,
    }
}

/// Builds the secret scanning observability summary for the organization.
///
/// Combines the org-level alert summary (if collected) with per-repository
/// check results to produce the final observability overview.
#[must_use]
pub fn build_secret_scanning_observability_summary(
    repositories: &[RepositoryEvidence],
    org_alert_summary: Option<&OrgAlertSummary>,
) -> SecretScanningObservability {
    let active: Vec<_> = repositories
        .iter()
        .filter(|r| !r.repository.archived)
        .collect();

    let mut summary = new_observability_summary();

    if let Some(alert_summary) = org_alert_summary {
        summary.collection_status = alert_summary.collection_status;
        summary
            .collection_reason
            .clone_from(&alert_summary.collection_reason);
        summary.total_open_secret_alerts =
            u32::try_from(alert_summary.total_open_secret_alerts).unwrap_or(u32::MAX);
        summary.open_secret_alert_age_buckets =
            merge_age_buckets(&alert_summary.open_secret_alert_age_buckets);
        summary
            .oldest_open_secret_alert_created_at
            .clone_from(&alert_summary.oldest_open_secret_alert_created_at);
        summary
            .newest_open_secret_alert_created_at
            .clone_from(&alert_summary.newest_open_secret_alert_created_at);
    }

    let per_repo = org_alert_summary.map_or(&*EMPTY_PER_REPO, |s| &s.per_repo);

    let mut mismatch_count: u32 = 0;
    let mut observable_enabled: u32 = 0;
    let mut unobservable: u32 = 0;

    for repo in &active {
        let ss = &repo.checks.secret_scanning;
        let repo_summary = per_repo.get(&repo.repository.inventory_key);
        let open_alert_count = repo_summary.map_or(0, |s| s.open_alert_count);

        if ss.status == SecretScanningStatus::Disabled && open_alert_count > 0 {
            mismatch_count = mismatch_count.saturating_add(1);
        }
        if ss.status == SecretScanningStatus::Enabled && ss.alerts_observable {
            observable_enabled = observable_enabled.saturating_add(1);
        }
        if !ss.alerts_observable {
            unobservable = unobservable.saturating_add(1);
        }
    }

    summary.status_mismatch_count = mismatch_count;
    summary.observable_enabled_repositories = observable_enabled;
    summary.unobservable_repositories = unobservable;

    summary
}

/// Empty per-repo map (used when no org alert summary is provided).
static EMPTY_PER_REPO: std::sync::LazyLock<HashMap<String, RepoAlertSummary>> =
    std::sync::LazyLock::new(HashMap::new);

/// Create a new observability summary with default `not_collected` state.
pub(crate) fn new_observability_summary() -> SecretScanningObservability {
    SecretScanningObservability {
        collection_status: CollectionStatus::NotCollected,
        collection_reason: None,
        total_open_secret_alerts: 0,
        open_secret_alert_age_buckets: empty_age_buckets_u32(),
        oldest_open_secret_alert_created_at: None,
        newest_open_secret_alert_created_at: None,
        status_mismatch_count: 0,
        observable_enabled_repositories: 0,
        unobservable_repositories: 0,
    }
}

/// Create an empty age-bucket map with `u32` values.
fn empty_age_buckets_u32() -> HashMap<String, u32> {
    config::empty_age_buckets()
}

/// Merge org-level age buckets (u64) into the summary format (u32),
/// ensuring all expected bucket labels are present.
///
/// Accepts arbitrary keys from `source` (forward-compatibility): if GitHub
/// introduces new bucket labels in the future, they will pass through to the
/// output rather than being silently dropped.
fn merge_age_buckets(source: &HashMap<String, u64>) -> HashMap<String, u32> {
    let mut buckets = empty_age_buckets_u32();
    for (key, &value) in source {
        let clamped = u32::try_from(value).unwrap_or(u32::MAX);
        buckets.insert(key.clone(), clamped);
    }
    buckets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::checks::{
        BranchProtectionDetails, BranchProtectionResult, BranchProtectionStatus,
        SecretScanningResult, SecretScanningStatus, SecurityPolicyResult,
    };
    use crate::domain::metrics::{
        CollectionHealthCheckKind, CollectionHealthCount, ScoreExclusionCount,
    };
    use crate::domain::repository::Visibility;
    use crate::domain::status::CollectionStatus;
    use crate::test_fixtures::*;

    /// Create a standard set of repos for testing metrics aggregation.
    ///
    /// Returns 5 non-archived repos (3 public, 1 internal, 1 private) + 1 archived:
    /// - public-1: all checks pass
    /// - public-2: policy fail, secret disabled, dependabot disabled, branch partial, codeowners non-conforming
    /// - public-3: policy unknown, secret `permission_denied`, dependabot unknown, branch unknown, codeowners unknown
    /// - internal-1: all checks pass (but policy not counted since non-public)
    /// - private-1: secret unknown, branch fail, codeowners absent
    /// - archived-1: should be excluded from all counts
    fn sample_repos() -> Vec<RepositoryEvidence> {
        vec![
            make_repository_evidence(
                "public-1",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_conforming(),
                ),
            ),
            make_repository_evidence(
                "public-2",
                Visibility::Public,
                false,
                make_checks(
                    policy_fail(),
                    secret_disabled(),
                    dependabot_disabled(),
                    branch_partial(),
                    codeowners_non_conforming(),
                ),
            ),
            make_repository_evidence(
                "public-3",
                Visibility::Public,
                false,
                make_checks(
                    policy_unknown(),
                    secret_permission_denied(),
                    dependabot_unknown(),
                    branch_unknown(),
                    codeowners_unknown(),
                ),
            ),
            make_repository_evidence(
                "internal-1",
                Visibility::Internal,
                false,
                make_checks(
                    policy_pass_file(),
                    secret_enabled_observable(true),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_conforming(),
                ),
            ),
            make_repository_evidence(
                "private-1",
                Visibility::Private,
                false,
                make_checks(
                    policy_fail(),
                    secret_unknown(),
                    dependabot_enabled(),
                    branch_fail(),
                    codeowners_absent(),
                ),
            ),
            make_repository_evidence(
                "archived-1",
                Visibility::Public,
                true,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_conforming(),
                ),
            ),
        ]
    }

    fn branch_protection_result_with_reason(
        reason_kind: CollectionFailureReason,
    ) -> BranchProtectionResult {
        BranchProtectionResult {
            status: BranchProtectionStatus::Unknown,
            details: BranchProtectionDetails {
                default_branch: "main".to_string(),
                has_pr: None,
                required_reviewers: None,
                has_status_checks: None,
                admin_equivalent: None,
                has_broad_bypass: None,
                reason: None,
                reason_kind: Some(reason_kind),
                http_status: None,
                force_push_blocked: None,
                deletion_blocked: None,
            },
            timestamp: make_timestamp(),
        }
    }

    fn repo_with_branch_protection_reason(
        reason_kind: CollectionFailureReason,
    ) -> RepositoryEvidence {
        make_repository_evidence(
            "reason-kind-probe",
            Visibility::Public,
            false,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_protection_result_with_reason(reason_kind),
                codeowners_conforming(),
            ),
        )
    }

    fn three_pass_two_fail_one_excluded_branch_protection() -> Vec<RepositoryEvidence> {
        let passing = |name: &str| all_passing_evidence(name);
        let failing = |name: &str, branch: BranchProtectionResult| {
            make_repository_evidence(
                name,
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch,
                    codeowners_conforming(),
                ),
            )
        };
        vec![
            passing("pass-1"),
            passing("pass-2"),
            passing("pass-3"),
            failing("fail-1", branch_partial()),
            failing("fail-2", branch_fail()),
            failing(
                "excluded-1",
                branch_protection_result_with_reason(CollectionFailureReason::PermissionDenied),
            ),
        ]
    }

    #[test]
    fn branch_protection_excluded_repo_leaves_denominator_and_counted_by_reason() {
        let repos = three_pass_two_fail_one_excluded_branch_protection();
        let metrics = aggregate_metrics(&repos);

        assert_eq!(metrics.branch_protection_coverage.numerator, 3);
        assert_eq!(
            metrics.branch_protection_coverage.denominator, 5,
            "pcoqb: the excluded repo must leave the denominator (5, not 6 folded into fail)"
        );
        assert_eq!(metrics.branch_protection_coverage.rate, Some(60.0));
        assert_eq!(metrics.branch_protection_counts.fail, 1);
        assert_eq!(metrics.branch_protection_counts.unknown, 1);

        assert!(
            metrics
                .score_exclusion_counts
                .contains(&ScoreExclusionCount {
                    check_kind: CollectionHealthCheckKind::BranchProtection,
                    reason: ExclusionReason::PermissionDenied,
                    count: 1,
                }),
            "expected a BranchProtection/PermissionDenied/1 exclusion row, got {:?}",
            metrics.score_exclusion_counts
        );
    }

    #[test]
    fn secret_scanning_excluded_repo_leaves_denominator_mission_fixture_shape() {
        let repos = vec![
            make_repository_evidence(
                "pub-1",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_conforming(),
                ),
            ),
            make_repository_evidence(
                "pub-2",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_conforming(),
                ),
            ),
            make_repository_evidence(
                "pub-3",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_conforming(),
                ),
            ),
            make_repository_evidence(
                "pub-4",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_disabled(),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_conforming(),
                ),
            ),
            make_repository_evidence(
                "pub-5",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_disabled(),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_conforming(),
                ),
            ),
            make_repository_evidence(
                "pub-6-excluded",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_permission_denied(),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_conforming(),
                ),
            ),
        ];
        let metrics = aggregate_metrics(&repos);

        assert_eq!(metrics.secret_scanning_coverage.numerator, 3);
        assert_eq!(metrics.secret_scanning_coverage.denominator, 5);
        assert_eq!(metrics.secret_scanning_coverage.rate, Some(60.0));

        assert!(
            metrics
                .score_exclusion_counts
                .contains(&ScoreExclusionCount {
                    check_kind: CollectionHealthCheckKind::SecretScanning,
                    reason: ExclusionReason::PermissionDenied,
                    count: 1,
                })
        );
    }

    #[test]
    fn score_exclusion_counts_by_reason_breakdown_sums_to_excluded_total() {
        let repos = vec![
            repo_with_branch_protection_reason(CollectionFailureReason::PermissionDenied),
            repo_with_branch_protection_reason(CollectionFailureReason::NotFoundAbsent),
            repo_with_branch_protection_reason(CollectionFailureReason::Transient),
        ];
        let metrics = aggregate_metrics(&repos);

        assert_eq!(metrics.branch_protection_coverage.denominator, 1);
        assert_eq!(metrics.branch_protection_coverage.rate, Some(0.0));

        let branch_protection_exclusions: Vec<_> = metrics
            .score_exclusion_counts
            .iter()
            .filter(|c| c.check_kind == CollectionHealthCheckKind::BranchProtection)
            .collect();
        let total: u32 = branch_protection_exclusions.iter().map(|c| c.count).sum();
        assert_eq!(total, 3, "all 3 repos are excluded, none dropped silently");
        assert!(
            branch_protection_exclusions.contains(&&ScoreExclusionCount {
                check_kind: CollectionHealthCheckKind::BranchProtection,
                reason: ExclusionReason::PermissionDenied,
                count: 1,
            })
        );
        assert!(
            branch_protection_exclusions.contains(&&ScoreExclusionCount {
                check_kind: CollectionHealthCheckKind::BranchProtection,
                reason: ExclusionReason::Unknown,
                count: 1,
            })
        );
        assert!(
            branch_protection_exclusions.contains(&&ScoreExclusionCount {
                check_kind: CollectionHealthCheckKind::BranchProtection,
                reason: ExclusionReason::Other,
                count: 1,
            })
        );
    }

    #[test]
    fn score_exclusion_counts_empty_when_no_exclusions() {
        let repos = vec![all_passing_evidence("clean")];
        let metrics = aggregate_metrics(&repos);
        assert!(metrics.score_exclusion_counts.is_empty());
    }

    #[test]
    fn codeowners_org_numerator_preserves_file_present_semantics() {
        let repos = vec![
            make_repository_evidence(
                "conforming",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_conforming(),
                ),
            ),
            make_repository_evidence(
                "non-conforming",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_non_conforming(),
                ),
            ),
            make_repository_evidence(
                "excluded",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_unknown(),
                ),
            ),
        ];
        let metrics = aggregate_metrics(&repos);

        assert_eq!(
            metrics.codeowners_coverage.numerator, 2,
            "non-conforming still counts as present in the org numerator (unchanged)"
        );
        assert_eq!(
            metrics.codeowners_coverage.denominator, 2,
            "the 1 excluded (unknown) repo drops out of the denominator: 2, not 3"
        );
        assert_eq!(metrics.codeowners_coverage.rate, Some(100.0));
        assert!(
            metrics
                .score_exclusion_counts
                .contains(&ScoreExclusionCount {
                    check_kind: CollectionHealthCheckKind::Codeowners,
                    reason: ExclusionReason::Unknown,
                    count: 1,
                })
        );
    }

    #[test]
    fn collection_statistics_counts_active_repos() {
        let repos = sample_repos();
        let stats = build_collection_statistics(&repos);
        assert_eq!(stats.total_repos, 5);
        assert_eq!(stats.public_repos, 3);
        assert_eq!(stats.internal_repos, 1);
        assert_eq!(stats.private_repos, 1);
    }

    #[test]
    fn aggregate_metrics_exclude_deleted_projection_rows_from_denominators() {
        let active = make_repository_evidence(
            "active-denominator",
            Visibility::Public,
            false,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_conforming(),
            ),
        );
        let mut projection = crate::projection::EvidenceProjection::default();
        projection
            .repositories
            .insert(active.repository.inventory_key.clone(), active.clone());
        projection.deleted.insert(
            "id-deleted-denominator".to_string(),
            crate::projection::DeletedRepoRecord {
                repo_name: "deleted-denominator".to_string(),
                detected_at: "2026-06-24T00:00:00Z".to_string(),
            },
        );
        let repos = projection.sorted_snapshot();

        let stats = build_collection_statistics(&repos);
        let metrics = aggregate_metrics(&repos);

        assert_eq!(stats.total_repos, 1);
        assert_eq!(metrics.security_policy_coverage.denominator, 1);
        assert_eq!(metrics.secret_scanning_coverage.denominator, 1);
        assert_eq!(metrics.dependabot_security_updates_coverage.denominator, 1);
        assert_eq!(metrics.branch_protection_coverage.denominator, 1);
        assert_eq!(metrics.codeowners_coverage.denominator, 1);
    }

    #[test]
    fn collection_statistics_empty_repos() {
        let stats = build_collection_statistics(&[]);
        assert_eq!(stats.total_repos, 0);
        assert_eq!(stats.public_repos, 0);
        assert_eq!(stats.internal_repos, 0);
        assert_eq!(stats.private_repos, 0);
    }

    #[test]
    fn collection_statistics_all_archived() {
        let repos = vec![make_repository_evidence(
            "archived",
            Visibility::Public,
            true,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_conforming(),
            ),
        )];
        let stats = build_collection_statistics(&repos);
        assert_eq!(stats.total_repos, 0);
    }

    #[test]
    fn aggregate_metrics_policy_counts_only_public_repos() {
        let repos = sample_repos();
        let metrics = aggregate_metrics(&repos);

        assert_eq!(metrics.policy_counts.via_setting, 2);
        assert_eq!(metrics.policy_counts.via_file, 0);
        assert_eq!(metrics.policy_counts.missing, 1);
        assert_eq!(metrics.policy_counts.unknown, 1);

        assert_eq!(metrics.security_policy_coverage.numerator, 2);
        assert_eq!(
            metrics.security_policy_coverage.denominator, 3,
            "public-3's policy_unknown() excludes it from the denominator (3, not 4)"
        );
        assert_eq!(
            metrics.security_policy_coverage.rate,
            Some(66.7),
            "excluding the 1 unmeasured repo raises the rate from 50.0% (2/4) to 66.7% (2/3)"
        );
    }

    #[test]
    fn aggregate_metrics_secret_scanning_denominator_counts_public_including_archived() {
        let repos = sample_repos();
        let metrics = aggregate_metrics(&repos);
        let public_repo_count = count_as_u32(
            repos
                .iter()
                .filter(|r| r.repository.visibility == Visibility::Public)
                .count(),
        );
        let active_repo_count =
            count_as_u32(repos.iter().filter(|r| !r.repository.archived).count());
        let archived_public_count = count_as_u32(
            repos
                .iter()
                .filter(|r| r.repository.visibility == Visibility::Public && r.repository.archived)
                .count(),
        );

        assert!(archived_public_count > 0);
        assert_ne!(public_repo_count, active_repo_count);

        assert_eq!(
            metrics.security_policy_coverage.numerator, 2,
            "archived-1 (a passing archived public repo) contributes to the numerator, \
             proving archived-public inclusion directly rather than via a raw \
             denominator == total_public equality"
        );
        assert_eq!(metrics.security_policy_coverage.denominator, 3);
        assert_eq!(metrics.secret_scanning_coverage.numerator, 2);
        assert_eq!(
            metrics.secret_scanning_coverage.denominator,
            public_repo_count - 1,
            "denominator = public repos minus the 1 excluded (permission_denied); \
             archived-1 (a passing archived public repo) still contributes to the numerator"
        );
    }

    #[test]
    fn aggregate_metrics_secret_scanning_counts() {
        let repos = sample_repos();
        let metrics = aggregate_metrics(&repos);

        assert_eq!(metrics.secret_scanning_counts.enabled, 2);
        assert_eq!(metrics.secret_scanning_counts.disabled, 1);
        assert_eq!(metrics.secret_scanning_counts.permission_denied, 1);
        assert_eq!(metrics.secret_scanning_counts.unknown, 0);

        assert_eq!(metrics.secret_scanning_coverage.numerator, 2);
        assert_eq!(
            metrics.secret_scanning_coverage.denominator, 3,
            "public-3's secret_permission_denied() excludes it from the denominator (3, not 4)"
        );
        assert_eq!(
            metrics.secret_scanning_coverage.rate,
            Some(66.7),
            "excluding the 1 unmeasured repo raises the rate from 50.0% (2/4) to 66.7% (2/3)"
        );
    }

    #[test]
    fn aggregate_metrics_dependabot_counts() {
        let repos = sample_repos();
        let metrics = aggregate_metrics(&repos);

        assert_eq!(metrics.dependabot_security_updates_counts.enabled, 3);
        assert_eq!(metrics.dependabot_security_updates_counts.disabled, 1);
        assert_eq!(metrics.dependabot_security_updates_counts.unknown, 1);

        assert_eq!(metrics.dependabot_security_updates_coverage.numerator, 3);
        assert_eq!(
            metrics.dependabot_security_updates_coverage.denominator, 4,
            "public-3's dependabot_unknown() excludes it from the denominator (4, not 5)"
        );
        assert_eq!(
            metrics.dependabot_security_updates_coverage.rate,
            Some(75.0),
            "excluding the 1 unmeasured repo raises the rate from 60.0% (3/5) to 75.0% (3/4)"
        );
    }

    #[test]
    fn aggregate_metrics_branch_protection_counts() {
        let repos = sample_repos();
        let metrics = aggregate_metrics(&repos);

        assert_eq!(metrics.branch_protection_counts.pass, 2);
        assert_eq!(metrics.branch_protection_counts.partial, 1);
        assert_eq!(
            metrics.branch_protection_counts.fail, 1,
            "pcoqb fix: public-3's excluded tier routes to .unknown, not folded into .fail \
             (only private-1's genuine BelowBaseline remains)"
        );
        assert_eq!(metrics.branch_protection_counts.unknown, 1);

        assert_eq!(metrics.branch_protection_coverage.numerator, 2);
        assert_eq!(metrics.branch_protection_coverage.denominator, 4);
        assert_eq!(
            metrics.branch_protection_coverage.rate,
            Some(50.0),
            "excluding the 1 unmeasured repo raises the rate from 40.0% (2/5) to 50.0% (2/4)"
        );
    }

    #[test]
    fn aggregate_metrics_collection_health_taxonomy_counts() {
        let mut repos = sample_repos();
        repos[0].checks.branch_protection.details.reason_kind =
            Some(CollectionFailureReason::PermissionSuspected);

        let metrics = aggregate_metrics(&repos);
        assert_eq!(
            metrics
                .branch_protection_coverage
                .extra
                .get("collection_health_branch_protection_permission_suspected"),
            Some(&serde_json::Value::from(1))
        );
        assert_eq!(
            metrics
                .secret_scanning_coverage
                .extra
                .get("collection_health_secret_scanning_permission_denied"),
            Some(&serde_json::Value::from(1))
        );
    }

    #[test]
    fn aggregate_metrics_exposes_typed_collection_health_counts() {
        let mut repos = sample_repos();
        repos[0].checks.branch_protection.details.reason_kind =
            Some(CollectionFailureReason::PermissionSuspected);

        let metrics = aggregate_metrics(&repos);

        assert!(
            metrics
                .collection_health_counts
                .contains(&CollectionHealthCount {
                    check_kind: CollectionHealthCheckKind::BranchProtection,
                    reason: CollectionFailureReason::PermissionSuspected,
                    count: 1,
                })
        );
        assert!(
            metrics
                .collection_health_counts
                .contains(&CollectionHealthCount {
                    check_kind: CollectionHealthCheckKind::SecretScanning,
                    reason: CollectionFailureReason::PermissionDenied,
                    count: 1,
                })
        );
        assert_eq!(
            serde_json::to_string(&CollectionHealthCheckKind::Rulesets).unwrap(),
            "\"rulesets\""
        );
    }

    #[test]
    fn collection_health_taxonomy_pre_extension_three_rows_contain_expected_counts() {
        let mut repos = sample_repos();
        repos[0].checks.branch_protection.details.reason_kind =
            Some(CollectionFailureReason::PermissionSuspected);
        repos[3].checks.branch_protection.details.reason_kind =
            Some(CollectionFailureReason::NotFoundAbsent);

        let metrics = aggregate_metrics(&repos);

        assert!(
            metrics
                .collection_health_counts
                .contains(&CollectionHealthCount {
                    check_kind: CollectionHealthCheckKind::SecretScanning,
                    reason: CollectionFailureReason::PermissionDenied,
                    count: 1,
                })
        );
        assert!(
            metrics
                .collection_health_counts
                .contains(&CollectionHealthCount {
                    check_kind: CollectionHealthCheckKind::BranchProtection,
                    reason: CollectionFailureReason::PermissionSuspected,
                    count: 1,
                })
        );
        assert!(
            metrics
                .collection_health_counts
                .contains(&CollectionHealthCount {
                    check_kind: CollectionHealthCheckKind::BranchProtection,
                    reason: CollectionFailureReason::NotFoundAbsent,
                    count: 1,
                })
        );
    }

    #[test]
    fn branch_protection_additional_reasons_counted_when_present_absent_when_not() {
        let reasons = [
            CollectionFailureReason::RateLimited,
            CollectionFailureReason::Transient,
            CollectionFailureReason::Invalid,
            CollectionFailureReason::PermissionDenied,
        ];
        for reason in reasons {
            let present = aggregate_metrics(&[repo_with_branch_protection_reason(reason)]);
            assert!(
                present
                    .collection_health_counts
                    .contains(&CollectionHealthCount {
                        check_kind: CollectionHealthCheckKind::BranchProtection,
                        reason,
                        count: 1,
                    }),
                "expected a BranchProtection/{reason:?} row when the reason is present"
            );

            let absent = aggregate_metrics(&[all_passing_evidence("clean")]);
            assert!(
                !absent.collection_health_counts.iter().any(|c| c.check_kind
                    == CollectionHealthCheckKind::BranchProtection
                    && c.reason == reason),
                "did not expect a BranchProtection/{reason:?} row when the reason is absent"
            );
        }
    }

    #[test]
    fn aggregate_metrics_codeowners_counts() {
        let repos = sample_repos();
        let metrics = aggregate_metrics(&repos);

        assert_eq!(metrics.codeowners_counts.conforming, 2);
        assert_eq!(metrics.codeowners_counts.non_conforming, 1);
        assert_eq!(metrics.codeowners_counts.absent, 1);
        assert_eq!(metrics.codeowners_counts.unknown, 1);

        assert_eq!(metrics.codeowners_coverage.numerator, 3);
        assert_eq!(
            metrics.codeowners_coverage.denominator, 4,
            "public-3's codeowners_unknown() excludes it from the denominator (4, not 5)"
        );
        assert_eq!(
            metrics.codeowners_coverage.rate,
            Some(75.0),
            "excluding the 1 unmeasured repo raises the rate from 60.0% (3/5) to 75.0% (3/4)"
        );
    }

    #[test]
    fn aggregate_metrics_secret_alert_prevalence() {
        let repos = sample_repos();
        let metrics = aggregate_metrics(&repos);

        assert_eq!(metrics.secret_alert_counts.repos_with_open_alerts, 1);
        assert_eq!(metrics.secret_alert_counts.repos_without_open_alerts, 1);
        assert_eq!(metrics.secret_alert_counts.unobservable, 3);

        assert_eq!(metrics.open_secret_alert_prevalence.numerator, 1);
        assert_eq!(metrics.open_secret_alert_prevalence.denominator, 2);
        assert_eq!(metrics.open_secret_alert_prevalence.rate, Some(50.0));
    }

    #[test]
    fn aggregate_metrics_empty_repos() {
        let metrics = aggregate_metrics(&[]);
        assert_eq!(metrics.security_policy_coverage.numerator, 0);
        assert_eq!(metrics.security_policy_coverage.denominator, 0);
        assert_eq!(metrics.security_policy_coverage.rate, None);
    }

    #[test]
    fn aggregate_metrics_archived_public_repo_included_in_security_policy() {
        let repos = vec![make_repository_evidence(
            "archived",
            Visibility::Public,
            true,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_conforming(),
            ),
        )];
        let metrics = aggregate_metrics(&repos);
        assert_eq!(metrics.security_policy_coverage.numerator, 1);
        assert_eq!(metrics.security_policy_coverage.denominator, 1);
        assert_eq!(metrics.branch_protection_coverage.denominator, 0);
    }

    #[test]
    fn aggregate_metrics_policy_file_evidence() {
        let repos = vec![make_repository_evidence(
            "pub-file",
            Visibility::Public,
            false,
            make_checks(
                policy_pass_file(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_conforming(),
            ),
        )];
        let metrics = aggregate_metrics(&repos);
        assert_eq!(metrics.policy_counts.via_setting, 0);
        assert_eq!(metrics.policy_counts.via_file, 1);
        assert_eq!(metrics.security_policy_coverage.numerator, 1);
        assert_eq!(metrics.security_policy_coverage.denominator, 1);
        assert_eq!(metrics.security_policy_coverage.rate, Some(100.0));
    }

    #[test]
    fn observability_summary_no_org_alert_data() {
        let repos = sample_repos();
        let summary = build_secret_scanning_observability_summary(&repos, None);

        assert_eq!(summary.collection_status, CollectionStatus::NotCollected);
        assert!(summary.collection_reason.is_none());
        assert_eq!(summary.total_open_secret_alerts, 0);
        assert_eq!(summary.observable_enabled_repositories, 2);
        assert_eq!(summary.unobservable_repositories, 3);
        assert_eq!(summary.status_mismatch_count, 0);
    }

    #[test]
    fn observability_summary_with_org_alert_data() {
        let repos = sample_repos();

        let mut per_repo = HashMap::new();
        per_repo.insert(
            "id-public-2".to_string(),
            RepoAlertSummary {
                open_alert_count: 2,
                oldest_open_alert_created_at: Some("2025-01-01T00:00:00+00:00".to_string()),
                newest_open_alert_created_at: Some("2026-03-15T00:00:00+00:00".to_string()),
            },
        );

        let mut age_buckets = HashMap::new();
        age_buckets.insert("0_7_days".to_string(), 1_u64);
        age_buckets.insert("91_plus_days".to_string(), 1_u64);

        let org_summary = OrgAlertSummary {
            collection_status: CollectionStatus::Success,
            collection_reason: None,
            per_repo,
            open_secret_alert_age_buckets: age_buckets,
            total_open_secret_alerts: 2,
            oldest_open_secret_alert_created_at: Some("2025-01-01T00:00:00+00:00".to_string()),
            newest_open_secret_alert_created_at: Some("2026-03-15T00:00:00+00:00".to_string()),
        };

        let summary = build_secret_scanning_observability_summary(&repos, Some(&org_summary));

        assert_eq!(summary.collection_status, CollectionStatus::Success);
        assert_eq!(summary.total_open_secret_alerts, 2);
        assert_eq!(
            summary.oldest_open_secret_alert_created_at,
            Some("2025-01-01T00:00:00+00:00".to_string())
        );
        assert_eq!(summary.status_mismatch_count, 1);
        assert_eq!(summary.observable_enabled_repositories, 2);
        assert_eq!(summary.unobservable_repositories, 3);
        assert_eq!(
            summary.open_secret_alert_age_buckets.get("0_7_days"),
            Some(&1)
        );
        assert_eq!(
            summary.open_secret_alert_age_buckets.get("91_plus_days"),
            Some(&1)
        );
    }

    #[test]
    fn observability_summary_permission_denied() {
        let repos = sample_repos();

        let org_summary = OrgAlertSummary {
            collection_status: CollectionStatus::PermissionDenied,
            collection_reason: Some("permission_denied".to_string()),
            per_repo: HashMap::new(),
            open_secret_alert_age_buckets: HashMap::new(),
            total_open_secret_alerts: 0,
            oldest_open_secret_alert_created_at: None,
            newest_open_secret_alert_created_at: None,
        };

        let summary = build_secret_scanning_observability_summary(&repos, Some(&org_summary));

        assert_eq!(
            summary.collection_status,
            CollectionStatus::PermissionDenied
        );
        assert_eq!(
            summary.collection_reason,
            Some("permission_denied".to_string())
        );
        assert_eq!(summary.status_mismatch_count, 0);
    }

    #[test]
    fn observability_summary_excludes_archived_repos() {
        let repos = vec![make_repository_evidence(
            "archived",
            Visibility::Public,
            true,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_conforming(),
            ),
        )];
        let summary = build_secret_scanning_observability_summary(&repos, None);
        assert_eq!(summary.observable_enabled_repositories, 0);
        assert_eq!(summary.unobservable_repositories, 0);
    }

    #[test]
    fn observability_summary_age_buckets_include_all_labels() {
        let summary = build_secret_scanning_observability_summary(&[], None);
        assert!(
            summary
                .open_secret_alert_age_buckets
                .contains_key("0_7_days")
        );
        assert!(
            summary
                .open_secret_alert_age_buckets
                .contains_key("8_30_days")
        );
        assert!(
            summary
                .open_secret_alert_age_buckets
                .contains_key("31_90_days")
        );
        assert!(
            summary
                .open_secret_alert_age_buckets
                .contains_key("91_plus_days")
        );
        assert!(
            summary
                .open_secret_alert_age_buckets
                .contains_key("unknown")
        );
    }

    #[test]
    fn aggregate_metrics_extra_fields_present() {
        let repos = sample_repos();
        let metrics = aggregate_metrics(&repos);

        assert!(
            metrics
                .security_policy_coverage
                .extra
                .contains_key("observable_repositories")
        );
        assert!(
            metrics
                .security_policy_coverage
                .extra
                .contains_key("unknown")
        );
        assert!(
            metrics
                .secret_scanning_coverage
                .extra
                .contains_key("disabled")
        );
        assert!(
            metrics
                .secret_scanning_coverage
                .extra
                .contains_key("permission_denied")
        );
        assert!(
            metrics
                .branch_protection_coverage
                .extra
                .contains_key("insufficient")
        );
        assert!(
            metrics
                .codeowners_coverage
                .extra
                .contains_key("non_conforming")
        );
        assert!(metrics.codeowners_coverage.extra.contains_key("absent"));
    }

    #[test]
    fn has_open_alerts_none_with_observable_counted_as_without() {
        let repos = vec![make_repository_evidence(
            "repo-1",
            Visibility::Private,
            false,
            make_checks(
                policy_fail(),
                SecretScanningResult {
                    status: SecretScanningStatus::Enabled,
                    has_open_alerts: None,
                    alerts_observable: true,
                    reason: None,
                    timestamp: make_timestamp(),
                },
                dependabot_enabled(),
                branch_pass(),
                codeowners_conforming(),
            ),
        )];
        let metrics = aggregate_metrics(&repos);
        assert_eq!(metrics.secret_alert_counts.repos_with_open_alerts, 0);
        assert_eq!(metrics.secret_alert_counts.repos_without_open_alerts, 1);
        assert_eq!(metrics.secret_alert_counts.unobservable, 0);
    }

    #[test]
    fn merge_age_buckets_preserves_extra_keys() {
        let mut source = HashMap::new();
        source.insert("0_7_days".to_string(), 5_u64);
        source.insert("future_bucket_label".to_string(), 3_u64);
        let merged = merge_age_buckets(&source);
        assert_eq!(merged.get("0_7_days"), Some(&5));
        assert_eq!(merged.get("future_bucket_label"), Some(&3));
        assert_eq!(merged.get("8_30_days"), Some(&0));
    }

    #[test]
    fn alert_prevalence_denominator_matches_observable_enabled() {
        let repos = sample_repos();
        let metrics = aggregate_metrics(&repos);
        let expected_denominator = metrics
            .secret_alert_counts
            .repos_with_open_alerts
            .saturating_add(metrics.secret_alert_counts.repos_without_open_alerts);
        assert_eq!(
            metrics.open_secret_alert_prevalence.denominator,
            expected_denominator
        );
    }

    #[test]
    fn disabled_with_observable_excluded_from_both_numerator_and_denominator() {
        let repos = vec![
            make_repository_evidence(
                "disabled-but-observable",
                Visibility::Private,
                false,
                make_checks(
                    policy_fail(),
                    SecretScanningResult {
                        status: SecretScanningStatus::Disabled,
                        has_open_alerts: Some(true),
                        alerts_observable: true,
                        reason: None,
                        timestamp: make_timestamp(),
                    },
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_conforming(),
                ),
            ),
            make_repository_evidence(
                "enabled-observable",
                Visibility::Private,
                false,
                make_checks(
                    policy_fail(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_conforming(),
                ),
            ),
        ];
        let metrics = aggregate_metrics(&repos);
        assert_eq!(metrics.secret_alert_counts.repos_with_open_alerts, 0);
        assert_eq!(metrics.secret_alert_counts.repos_without_open_alerts, 1);
        assert_eq!(metrics.secret_alert_counts.unobservable, 1);
        assert_eq!(metrics.open_secret_alert_prevalence.denominator, 1);
        assert_eq!(metrics.open_secret_alert_prevalence.rate, Some(0.0));
    }

    #[test]
    fn aggregate_metrics_extra_field_values_correct() {
        let repos = sample_repos();
        let metrics = aggregate_metrics(&repos);

        assert_eq!(
            metrics
                .security_policy_coverage
                .extra
                .get("observable_repositories"),
            Some(&serde_json::json!(3))
        );
        assert_eq!(
            metrics.security_policy_coverage.extra.get("unknown"),
            Some(&serde_json::json!(1))
        );

        assert_eq!(
            metrics
                .secret_scanning_coverage
                .extra
                .get("observable_repositories"),
            Some(&serde_json::json!(3))
        );

        assert_eq!(
            metrics.branch_protection_coverage.extra.get("insufficient"),
            Some(&serde_json::json!(2)),
            "insufficient = partial(1) + fail(1); public-3's excluded tier no longer folds \
             into fail (pcoqb fix), so this drops from 3 to 2"
        );
        assert_eq!(
            metrics
                .branch_protection_coverage
                .extra
                .get("observable_repositories"),
            Some(&serde_json::json!(4)),
            "observable = pass(2) + partial(1) + fail(1); excludes public-3, dropping from 5 to 4"
        );

        assert_eq!(
            metrics
                .codeowners_coverage
                .extra
                .get("observable_repositories"),
            Some(&serde_json::json!(4))
        );
    }

    #[test]
    fn merge_age_buckets_clamps_large_u64() {
        let mut source = HashMap::new();
        source.insert("0_7_days".to_string(), u64::MAX);
        let merged = merge_age_buckets(&source);
        assert_eq!(merged.get("0_7_days"), Some(&u32::MAX));
    }

    use crate::collector::codeowners_parser::parse_codeowners;
    use crate::domain::checks::CodeownersResult;
    use crate::domain::metrics::OwnerType;

    /// Create a `CodeownersResult` by parsing raw CODEOWNERS content.
    fn codeowners_from_content(content: &str) -> CodeownersResult {
        CodeownersResult {
            status: CodeownersStatus::Conforming,
            path: Some(".github/CODEOWNERS".to_string()),
            timestamp: make_timestamp(),
            parsed: Some(parse_codeowners(content)),
            truncation: None,
        }
    }

    #[test]
    fn owner_metrics_empty_repos() {
        let result = build_owner_metrics(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn owner_metrics_no_parsed_data() {
        let repos = vec![make_repository_evidence(
            "repo-1",
            Visibility::Public,
            false,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_conforming(),
            ),
        )];
        let result = build_owner_metrics(&repos);
        assert!(result.is_empty());
    }

    #[test]
    fn owner_metrics_single_owner() {
        let repos = vec![make_repository_evidence(
            "repo-1",
            Visibility::Public,
            false,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_with_owners(&["@org/team-a"]),
            ),
        )];
        let result = build_owner_metrics(&repos);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].owner, "@org/team-a");
        assert_eq!(result[0].display_name, "@org/team-a");
        assert_eq!(result[0].owner_type, OwnerType::Team);
        assert_eq!(result[0].total_repos, 1);
        assert_eq!(
            result[0]
                .per_control_coverage
                .get("security_policy")
                .unwrap()
                .rate,
            Some(100.0)
        );
        assert_eq!(
            result[0]
                .per_control_coverage
                .get("secret_scanning")
                .unwrap()
                .rate,
            Some(100.0)
        );
    }

    #[test]
    fn owner_metrics_multi_owner_repo() {
        let repos = vec![make_repository_evidence(
            "repo-1",
            Visibility::Public,
            false,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_with_owners(&["@org/team-a", "@alice"]),
            ),
        )];
        let result = build_owner_metrics(&repos);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].owner, "@org/team-a");
        assert_eq!(result[0].owner_type, OwnerType::Team);
        assert_eq!(result[0].total_repos, 1);
        assert_eq!(result[1].owner, "@alice");
        assert_eq!(result[1].owner_type, OwnerType::User);
        assert_eq!(result[1].total_repos, 1);
    }

    #[test]
    fn owner_metrics_sort_teams_before_users_alphabetically() {
        let repos = vec![make_repository_evidence(
            "repo-1",
            Visibility::Public,
            false,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_with_owners(&["@bob", "@org/beta", "@alice", "@org/alpha"]),
            ),
        )];
        let result = build_owner_metrics(&repos);
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].owner, "@org/alpha");
        assert_eq!(result[0].owner_type, OwnerType::Team);
        assert_eq!(result[1].owner, "@org/beta");
        assert_eq!(result[1].owner_type, OwnerType::Team);
        assert_eq!(result[2].owner, "@alice");
        assert_eq!(result[2].owner_type, OwnerType::User);
        assert_eq!(result[3].owner, "@bob");
        assert_eq!(result[3].owner_type, OwnerType::User);
    }

    #[test]
    fn owner_metrics_owner_type_detection() {
        let repos = vec![make_repository_evidence(
            "repo-1",
            Visibility::Public,
            false,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_with_owners(&["@org/team-name", "@individual"]),
            ),
        )];
        let result = build_owner_metrics(&repos);
        let team = result.iter().find(|o| o.owner == "@org/team-name").unwrap();
        let user = result.iter().find(|o| o.owner == "@individual").unwrap();
        assert_eq!(team.owner_type, OwnerType::Team);
        assert_eq!(user.owner_type, OwnerType::User);
    }

    #[test]
    fn owner_metrics_case_insensitive_dedup() {
        let repos = vec![
            make_repository_evidence(
                "repo-1",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_with_owners(&["@Org/Team"]),
                ),
            ),
            make_repository_evidence(
                "repo-2",
                Visibility::Public,
                false,
                make_checks(
                    policy_fail(),
                    secret_disabled(),
                    dependabot_disabled(),
                    branch_fail(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            ),
        ];
        let result = build_owner_metrics(&repos);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].owner, "@org/team");
        assert_eq!(result[0].display_name, "@Org/Team");
        assert_eq!(result[0].total_repos, 2);
        assert_eq!(
            result[0]
                .per_control_coverage
                .get("security_policy")
                .unwrap()
                .rate,
            Some(50.0)
        );
    }

    #[test]
    fn owner_metrics_excludes_archived() {
        let repos = vec![make_repository_evidence(
            "archived",
            Visibility::Public,
            true,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_with_owners(&["@org/team"]),
            ),
        )];
        let result = build_owner_metrics(&repos);
        assert!(result.is_empty());
    }

    #[test]
    fn owner_metrics_per_control_coverage() {
        let repos = vec![
            make_repository_evidence(
                "repo-1",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            ),
            make_repository_evidence(
                "repo-2",
                Visibility::Private,
                false,
                make_checks(
                    policy_fail(),
                    secret_disabled(),
                    dependabot_disabled(),
                    branch_fail(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            ),
        ];
        let result = build_owner_metrics(&repos);
        assert_eq!(result.len(), 1);
        let team = &result[0];
        assert_eq!(team.total_repos, 2);
        assert_eq!(
            team.per_control_coverage
                .get("security_policy")
                .unwrap()
                .numerator,
            1
        );
        assert_eq!(
            team.per_control_coverage
                .get("secret_scanning")
                .unwrap()
                .numerator,
            1
        );
        assert_eq!(
            team.per_control_coverage
                .get("dependabot_security_updates")
                .unwrap()
                .numerator,
            1
        );
        assert_eq!(
            team.per_control_coverage
                .get("branch_protection")
                .unwrap()
                .numerator,
            1
        );
        assert_eq!(
            team.per_control_coverage
                .get("codeowners")
                .unwrap()
                .numerator,
            2
        );
    }

    /// adr-fmt-voxg6 item1: a control where every repo is `Excluded` for a
    /// measurement-failure reason (`Unknown`/`PermissionDenied`/`Other`) must
    /// floor to a genuine failure (`Some(0.0)`), not vanish to `None` — an
    /// all-unmeasured control silently inflating Team Health to a vacuous
    /// 100% (adr-fmt-doyc8/e861p) is the bug this guards against.
    #[test]
    fn owner_control_coverage_all_measurement_failure_floors_to_zero_rate() {
        let repos = vec![
            make_repository_evidence(
                "repo-1",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_unknown(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            ),
            make_repository_evidence(
                "repo-2",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_unknown(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            ),
        ];
        let result = build_owner_metrics(&repos);
        let team = &result[0];
        let branch_protection = team.per_control_coverage.get("branch_protection").unwrap();
        assert_eq!(branch_protection.numerator, 0);
        assert_eq!(branch_protection.denominator, 1);
        assert_eq!(branch_protection.rate, Some(0.0));
    }

    /// adr-fmt-voxg6 item1: a control where every repo is `Excluded` for
    /// `NotApplicable` — no observable population — legitimately vanishes to
    /// `None`. Security policy is `NotApplicable` on non-public repos.
    #[test]
    fn owner_control_coverage_all_not_applicable_stays_none() {
        let not_applicable_policy = || SecurityPolicyResult {
            status: SecurityPolicyStatus::NotApplicable,
            evidence: SecurityPolicyEvidence::NotApplicable,
            path: None,
            timestamp: make_timestamp(),
        };
        let repos = vec![
            make_repository_evidence(
                "repo-1",
                Visibility::Private,
                false,
                make_checks(
                    not_applicable_policy(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            ),
            make_repository_evidence(
                "repo-2",
                Visibility::Private,
                false,
                make_checks(
                    not_applicable_policy(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            ),
        ];
        let result = build_owner_metrics(&repos);
        let team = &result[0];
        let security_policy = team.per_control_coverage.get("security_policy").unwrap();
        assert_eq!(security_policy.numerator, 0);
        assert_eq!(security_policy.denominator, 0);
        assert_eq!(security_policy.rate, None);
    }

    /// adr-fmt-voxg6 item1 REGRESSION GUARD: a mixed team (pass + fail +
    /// excluded) must keep excluding the excluded repo from the denominator
    /// and compute the rate over the measured population only — denom > 0,
    /// so `coverage_metric`'s reason-aware floor never engages. Matches the
    /// pre-change value.
    #[test]
    fn owner_control_coverage_mixed_team_excludes_not_floors() {
        let repos = vec![
            make_repository_evidence(
                "pass-1",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            ),
            make_repository_evidence(
                "fail-1",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_fail(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            ),
            make_repository_evidence(
                "excluded-1",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_unknown(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            ),
        ];
        let result = build_owner_metrics(&repos);
        let team = &result[0];
        let branch_protection = team.per_control_coverage.get("branch_protection").unwrap();
        assert_eq!(branch_protection.numerator, 1);
        assert_eq!(
            branch_protection.denominator, 2,
            "excluded repo must leave the denominator (2), not be floored/counted"
        );
        assert_eq!(branch_protection.rate, Some(50.0));
    }

    /// adr-fmt-voxg6 item1 CRITICAL GUARD: `secret_scanning` filters non-public
    /// repos *before* classification (metrics.rs guard at the top of
    /// `build_per_control_coverage`'s loop), so an all-private team never
    /// records any `secret_scanning` outcome — the tally is empty, not a
    /// measurement failure — and the rate must stay `None`, not floor to 0.0.
    #[test]
    fn owner_control_coverage_all_private_secret_scanning_stays_none() {
        let repos = vec![
            make_repository_evidence(
                "repo-1",
                Visibility::Private,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            ),
            make_repository_evidence(
                "repo-2",
                Visibility::Private,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            ),
        ];
        let result = build_owner_metrics(&repos);
        let team = &result[0];
        let secret_scanning = team.per_control_coverage.get("secret_scanning").unwrap();
        assert_eq!(secret_scanning.numerator, 0);
        assert_eq!(secret_scanning.denominator, 0);
        assert_eq!(secret_scanning.rate, None);
    }

    /// item9 Part B (M1, adr-fmt-jlfs1): mirrors
    /// `enrich_team_rosters_flags_departed_member_and_clears_present_member`
    /// (`crate::collector::team_membership`) at the `OwnerMetrics` seam —
    /// a `User`-type owner NOT in the org-members set is flagged
    /// `Some(false)`; one IN the set is `Some(true)` (lowercase
    /// cross-match: set entry `"alice"` matches owner login `@Alice`). A
    /// `Team`-type owner is left `None` regardless of set contents — team
    /// membership is cross-checked separately via `TeamMember::in_org`,
    /// not via the owner string itself.
    #[test]
    fn enrich_owner_metrics_flags_departed_user_clears_present_user_skips_team() {
        let mut owners = vec![
            OwnerMetrics {
                owner: "@Alice".to_string(),
                display_name: "@Alice".to_string(),
                owner_type: OwnerType::User,
                total_repos: 1,
                per_control_coverage: HashMap::new(),
                score_exclusion_counts: Vec::new(),
                in_org: None,
            },
            OwnerMetrics {
                owner: "@departed-carol".to_string(),
                display_name: "@departed-carol".to_string(),
                owner_type: OwnerType::User,
                total_repos: 1,
                per_control_coverage: HashMap::new(),
                score_exclusion_counts: Vec::new(),
                in_org: None,
            },
            OwnerMetrics {
                owner: "@org/some-team".to_string(),
                display_name: "@org/some-team".to_string(),
                owner_type: OwnerType::Team,
                total_repos: 1,
                per_control_coverage: HashMap::new(),
                score_exclusion_counts: Vec::new(),
                in_org: None,
            },
        ];
        let org_members: HashSet<String> = ["alice".to_string()].into_iter().collect();

        enrich_owner_metrics_with_org_membership(&mut owners, Some(&org_members));

        assert_eq!(
            owners[0].in_org,
            Some(true),
            "'@Alice' must match lowercased set entry 'alice'"
        );
        assert_eq!(
            owners[1].in_org,
            Some(false),
            "'@departed-carol' is absent from the set — flagged departed"
        );
        assert_eq!(
            owners[2].in_org, None,
            "a Team-type owner is never cross-checked against the org-members set"
        );
    }

    /// item9 Part B (M1, adr-fmt-jlfs1): mirrors
    /// `enrich_team_rosters_flags_nobody_when_org_members_degraded` — when
    /// the org-members fetch degraded (`org_members: None`), a `User`-type
    /// owner stays `None` (no flag on missing data); a `Team`-type owner
    /// stays `None` too (always skipped, independent of degrade state).
    #[test]
    fn enrich_owner_metrics_flags_nobody_when_org_members_degraded() {
        let mut owners = vec![
            OwnerMetrics {
                owner: "@alice".to_string(),
                display_name: "@alice".to_string(),
                owner_type: OwnerType::User,
                total_repos: 1,
                per_control_coverage: HashMap::new(),
                score_exclusion_counts: Vec::new(),
                in_org: None,
            },
            OwnerMetrics {
                owner: "@org/some-team".to_string(),
                display_name: "@org/some-team".to_string(),
                owner_type: OwnerType::Team,
                total_repos: 1,
                per_control_coverage: HashMap::new(),
                score_exclusion_counts: Vec::new(),
                in_org: None,
            },
        ];

        enrich_owner_metrics_with_org_membership(&mut owners, None);

        assert_eq!(
            owners[0].in_org, None,
            "degraded org-members fetch must not flag a User-type owner"
        );
        assert_eq!(
            owners[1].in_org, None,
            "a Team-type owner stays None regardless of degrade state"
        );
    }

    /// UF2-7: an owner's secret-scanning denominator must count only the
    /// owner's PUBLIC repos, mirroring the org-page population and the
    /// existing `sp_total` custom-denominator idiom for `security_policy`.
    /// Before UF2-7 this used the shared `total` (all of the owner's
    /// non-archived repos, every visibility) — this test pins the flip.
    #[test]
    fn owner_secret_scanning_denominator_counts_public_only() {
        let repos = vec![
            make_repository_evidence(
                "pub-repo",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            ),
            make_repository_evidence(
                "priv-repo",
                Visibility::Private,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            ),
        ];
        let result = build_owner_metrics(&repos);
        assert_eq!(result.len(), 1);
        let team = &result[0];
        assert_eq!(
            team.total_repos, 2,
            "owner set still counts all visibilities"
        );

        let secret_scanning = team.per_control_coverage.get("secret_scanning").unwrap();
        assert_eq!(
            secret_scanning.denominator, 1,
            "secret_scanning denominator must count only the owner's public \
             repos (mirrors sp_total idiom), got {secret_scanning:?}"
        );
        assert_eq!(secret_scanning.numerator, 1);

        let dependabot = team
            .per_control_coverage
            .get("dependabot_security_updates")
            .unwrap();
        assert_eq!(
            dependabot.denominator, 2,
            "dependabot denominator must remain all-visibilities (contrast control)"
        );
    }

    /// UF2-8: cross-page consistency guard for the DECIDED population matrix
    /// (single source: bd bead `adr-fmt-5dfp2`). Builds ONE repo set — an
    /// owner with one public and one private non-archived repo — and
    /// asserts the VISIBILITY axis holds on BOTH the org page
    /// (`aggregate_metrics`) and the owner page (`build_owner_metrics`)
    /// from the SAME underlying data: `security_policy` + `secret_scanning`
    /// are public-only on both scopes; `dependabot_security_updates` +
    /// `branch_protection` + `codeowners` are all-visibilities on both
    /// scopes. Does NOT assert archived-parity between org-wide and
    /// owner-attributed scopes — those are legitimately different
    /// denominator universes, guarded instead by the four named
    /// archived-axis regression tests
    /// (`warm_start_replay_preserves_archived_public_security_policy_in_aggregate_metrics`,
    /// `aggregate_metrics_archived_public_repo_included_in_security_policy`,
    /// `aggregate_metrics_secret_scanning_denominator_counts_public_including_archived`,
    /// `aggregate_metrics_includes_archived_public_repos_with_security_policy`)
    /// plus `owner_metrics_excludes_archived` for the owner-side exclusion.
    #[test]
    fn population_matrix_visibility_axis_consistent_org_and_owner() {
        let repos = vec![
            make_repository_evidence(
                "pub-repo",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            ),
            make_repository_evidence(
                "priv-repo",
                Visibility::Private,
                false,
                make_checks(
                    crate::domain::checks::SecurityPolicyResult {
                        status: SecurityPolicyStatus::NotApplicable,
                        evidence: SecurityPolicyEvidence::NotApplicable,
                        path: None,
                        timestamp: make_timestamp(),
                    },
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            ),
        ];

        let org_metrics = aggregate_metrics(&repos);
        assert_eq!(
            org_metrics.security_policy_coverage.denominator, 1,
            "org security_policy must be public-only"
        );
        assert_eq!(
            org_metrics.secret_scanning_coverage.denominator, 1,
            "org secret_scanning must be public-only"
        );
        assert_eq!(
            org_metrics.dependabot_security_updates_coverage.denominator, 2,
            "org dependabot must be all-visibilities"
        );
        assert_eq!(
            org_metrics.branch_protection_coverage.denominator, 2,
            "org branch_protection must be all-visibilities"
        );
        assert_eq!(
            org_metrics.codeowners_coverage.denominator, 2,
            "org codeowners must be all-visibilities"
        );

        let owners = build_owner_metrics(&repos);
        assert_eq!(owners.len(), 1, "both repos attribute to the same owner");
        let coverage = &owners[0].per_control_coverage;
        assert_eq!(
            coverage.get("security_policy").unwrap().denominator,
            1,
            "owner security_policy must be public-only"
        );
        assert_eq!(
            coverage.get("secret_scanning").unwrap().denominator,
            1,
            "owner secret_scanning must be public-only"
        );
        assert_eq!(
            coverage
                .get("dependabot_security_updates")
                .unwrap()
                .denominator,
            2,
            "owner dependabot must be all-visibilities"
        );
        assert_eq!(
            coverage.get("branch_protection").unwrap().denominator,
            2,
            "owner branch_protection must be all-visibilities"
        );
        assert_eq!(
            coverage.get("codeowners").unwrap().denominator,
            2,
            "owner codeowners must be all-visibilities"
        );
    }

    #[test]
    fn owner_security_policy_excludes_unknown_from_denominator() {
        let repo = |name: &str, policy: crate::domain::checks::SecurityPolicyResult| {
            make_repository_evidence(
                name,
                Visibility::Public,
                false,
                make_checks(
                    policy,
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            )
        };
        let repos = vec![
            repo("pass-1", policy_pass_setting()),
            repo("pass-2", policy_pass_setting()),
            repo("pass-3", policy_pass_setting()),
            repo("fail-1", policy_fail()),
            repo("fail-2", policy_fail()),
            repo("unknown-1", policy_unknown()),
        ];
        let owners = build_owner_metrics(&repos);
        assert_eq!(owners.len(), 1);
        let security_policy = owners[0]
            .per_control_coverage
            .get("security_policy")
            .unwrap();
        assert_eq!(security_policy.numerator, 3);
        assert_eq!(
            security_policy.denominator, 5,
            "the 1 unknown repo must leave the denominator (5, not 6 folded into fail)"
        );
        assert_eq!(security_policy.rate, Some(60.0));

        assert!(
            owners[0]
                .score_exclusion_counts
                .contains(&ScoreExclusionCount {
                    check_kind: CollectionHealthCheckKind::SecurityPolicy,
                    reason: ExclusionReason::Unknown,
                    count: 1,
                }),
            "expected a SecurityPolicy/Unknown/1 exclusion row, got {:?}",
            owners[0].score_exclusion_counts
        );
    }

    #[test]
    fn owner_secret_scanning_excludes_permission_denied_and_unknown_from_denominator() {
        let repo = |name: &str, secret: SecretScanningResult| {
            make_repository_evidence(
                name,
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret,
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            )
        };
        let repos = vec![
            repo("pass-1", secret_enabled_observable(false)),
            repo("pass-2", secret_enabled_observable(false)),
            repo("fail-1", secret_disabled()),
            repo("excluded-permission", secret_permission_denied()),
            repo("excluded-unknown", secret_unknown()),
        ];
        let owners = build_owner_metrics(&repos);
        assert_eq!(owners.len(), 1);
        let secret_scanning = owners[0]
            .per_control_coverage
            .get("secret_scanning")
            .unwrap();
        assert_eq!(secret_scanning.numerator, 2);
        assert_eq!(
            secret_scanning.denominator, 3,
            "the 2 excluded (permission_denied + unknown) repos must leave the \
             denominator (3, not 5 folded into fail)"
        );
        assert_eq!(secret_scanning.rate, Some(66.7));

        let exclusions: Vec<_> = owners[0]
            .score_exclusion_counts
            .iter()
            .filter(|c| c.check_kind == CollectionHealthCheckKind::SecretScanning)
            .collect();
        assert!(exclusions.contains(&&ScoreExclusionCount {
            check_kind: CollectionHealthCheckKind::SecretScanning,
            reason: ExclusionReason::PermissionDenied,
            count: 1,
        }));
        assert!(exclusions.contains(&&ScoreExclusionCount {
            check_kind: CollectionHealthCheckKind::SecretScanning,
            reason: ExclusionReason::Unknown,
            count: 1,
        }));
    }

    #[test]
    fn owner_dependabot_excludes_unknown_from_denominator() {
        let repo = |name: &str, dependabot: crate::domain::checks::DependabotResult| {
            make_repository_evidence(
                name,
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot,
                    branch_pass(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            )
        };
        let repos = vec![
            repo("pass-1", dependabot_enabled()),
            repo("pass-2", dependabot_enabled()),
            repo("pass-3", dependabot_enabled()),
            repo("fail-1", dependabot_disabled()),
            repo("fail-2", dependabot_disabled()),
            repo("unknown-1", dependabot_unknown()),
        ];
        let owners = build_owner_metrics(&repos);
        assert_eq!(owners.len(), 1);
        let dependabot = owners[0]
            .per_control_coverage
            .get("dependabot_security_updates")
            .unwrap();
        assert_eq!(dependabot.numerator, 3);
        assert_eq!(
            dependabot.denominator, 5,
            "the 1 unknown repo must leave the denominator (5, not 6 folded into fail)"
        );
        assert_eq!(dependabot.rate, Some(60.0));

        assert!(
            owners[0]
                .score_exclusion_counts
                .contains(&ScoreExclusionCount {
                    check_kind: CollectionHealthCheckKind::Dependabot,
                    reason: ExclusionReason::Unknown,
                    count: 1,
                })
        );
    }

    #[test]
    fn owner_branch_protection_excluded_tier_leaves_denominator() {
        let repo = |name: &str, branch: BranchProtectionResult| {
            make_repository_evidence(
                name,
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch,
                    codeowners_with_owners(&["@org/team"]),
                ),
            )
        };
        let repos = vec![
            repo("pass-1", branch_pass()),
            repo("pass-2", branch_pass()),
            repo("pass-3", branch_pass()),
            repo("fail-1", branch_partial()),
            repo("fail-2", branch_fail()),
            repo(
                "excluded-1",
                branch_protection_result_with_reason(CollectionFailureReason::PermissionDenied),
            ),
        ];
        let owners = build_owner_metrics(&repos);
        assert_eq!(owners.len(), 1);
        let branch_protection = owners[0]
            .per_control_coverage
            .get("branch_protection")
            .unwrap();
        assert_eq!(branch_protection.numerator, 3);
        assert_eq!(
            branch_protection.denominator, 5,
            "pcoqb-equivalent at owner scope: the excluded repo must leave the \
             denominator (5, not 6 folded into fail)"
        );
        assert_eq!(branch_protection.rate, Some(60.0));

        assert!(
            owners[0]
                .score_exclusion_counts
                .contains(&ScoreExclusionCount {
                    check_kind: CollectionHealthCheckKind::BranchProtection,
                    reason: ExclusionReason::PermissionDenied,
                    count: 1,
                }),
            "expected a BranchProtection/PermissionDenied/1 exclusion row, got {:?}",
            owners[0].score_exclusion_counts
        );
    }

    #[test]
    fn owner_score_exclusion_counts_by_reason_breakdown_sums_to_excluded_total() {
        let repo = |name: &str, reason_kind: CollectionFailureReason| {
            make_repository_evidence(
                name,
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_protection_result_with_reason(reason_kind),
                    codeowners_with_owners(&["@org/team"]),
                ),
            )
        };
        let repos = vec![
            repo("r1", CollectionFailureReason::PermissionDenied),
            repo("r2", CollectionFailureReason::NotFoundAbsent),
            repo("r3", CollectionFailureReason::Transient),
        ];
        let owners = build_owner_metrics(&repos);
        assert_eq!(owners.len(), 1);
        let branch_protection = owners[0]
            .per_control_coverage
            .get("branch_protection")
            .unwrap();
        assert_eq!(
            branch_protection.denominator, 1,
            "all 3 repos excluded for measurement failure floors to 0.0, not None"
        );
        assert_eq!(branch_protection.rate, Some(0.0));

        let exclusions: Vec<_> = owners[0]
            .score_exclusion_counts
            .iter()
            .filter(|c| c.check_kind == CollectionHealthCheckKind::BranchProtection)
            .collect();
        let total: u32 = exclusions.iter().map(|c| c.count).sum();
        assert_eq!(total, 3, "all 3 repos are excluded, none dropped silently");
        assert!(exclusions.contains(&&ScoreExclusionCount {
            check_kind: CollectionHealthCheckKind::BranchProtection,
            reason: ExclusionReason::PermissionDenied,
            count: 1,
        }));
        assert!(exclusions.contains(&&ScoreExclusionCount {
            check_kind: CollectionHealthCheckKind::BranchProtection,
            reason: ExclusionReason::Unknown,
            count: 1,
        }));
        assert!(exclusions.contains(&&ScoreExclusionCount {
            check_kind: CollectionHealthCheckKind::BranchProtection,
            reason: ExclusionReason::Other,
            count: 1,
        }));
    }

    #[test]
    fn owner_codeowners_numerator_preserves_file_present_semantics() {
        let repos = vec![
            make_repository_evidence(
                "conforming",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            ),
            make_repository_evidence(
                "non-conforming",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    crate::domain::checks::CodeownersResult {
                        status: CodeownersStatus::NonConforming,
                        ..codeowners_with_owners(&["@org/team"])
                    },
                ),
            ),
            make_repository_evidence(
                "excluded",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    crate::domain::checks::CodeownersResult {
                        status: CodeownersStatus::Unknown,
                        ..codeowners_with_owners(&["@org/team"])
                    },
                ),
            ),
        ];
        let owners = build_owner_metrics(&repos);
        assert_eq!(owners.len(), 1);
        let codeowners = owners[0].per_control_coverage.get("codeowners").unwrap();
        assert_eq!(
            codeowners.numerator, 2,
            "non-conforming still counts as present in the owner numerator \
             (unchanged, adr-fmt-pptla)"
        );
        assert_eq!(
            codeowners.denominator, 2,
            "the 1 excluded (unknown) repo drops out of the denominator: 2, not 3"
        );
        assert_eq!(codeowners.rate, Some(100.0));
        assert!(
            owners[0]
                .score_exclusion_counts
                .contains(&ScoreExclusionCount {
                    check_kind: CollectionHealthCheckKind::Codeowners,
                    reason: ExclusionReason::Unknown,
                    count: 1,
                })
        );
    }

    #[test]
    fn owner_metrics_integrated_in_aggregate() {
        let repos = vec![make_repository_evidence(
            "repo-1",
            Visibility::Public,
            false,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_with_owners(&["@org/team"]),
            ),
        )];
        let metrics = aggregate_metrics(&repos);
        assert_eq!(metrics.owner_metrics.len(), 1);
        assert_eq!(metrics.owner_metrics[0].owner, "@org/team");
    }

    #[test]
    fn owner_metrics_include_default_wildcard_owner() {
        let repos = vec![make_repository_evidence(
            "repo-1",
            Visibility::Public,
            false,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_from_content("* @org/default-team\n"),
            ),
        )];

        let metrics = aggregate_metrics(&repos);

        assert_eq!(metrics.owner_metrics.len(), 1);
        assert_eq!(metrics.owner_metrics[0].owner, "@org/default-team");
        assert_eq!(metrics.owner_metrics[0].total_repos, 1);
    }

    #[test]
    fn owner_repo_map_empty_repos() {
        let map = build_owner_repo_map(&[]);
        assert!(map.is_empty());
    }

    #[test]
    fn owner_repo_map_no_parsed_codeowners() {
        let repos = vec![make_repository_evidence(
            "repo-1",
            Visibility::Public,
            false,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_conforming(),
            ),
        )];
        let map = build_owner_repo_map(&repos);
        assert!(map.is_empty());
    }

    #[test]
    fn owner_repo_map_single_owner_single_repo() {
        let repos = vec![make_repository_evidence(
            "repo-1",
            Visibility::Public,
            false,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_with_owners(&["@org/team-a"]),
            ),
        )];
        let map = build_owner_repo_map(&repos);
        assert_eq!(map.len(), 1);
        let (display, repos_list) = &map["@org/team-a"];
        assert_eq!(display, "@org/team-a");
        assert_eq!(repos_list.len(), 1);
        assert_eq!(repos_list[0].repository.name, "repo-1");
    }

    #[test]
    fn team_owner_slugs_excludes_user_owners_and_extracts_team_slug() {
        use crate::domain::metrics::team_owner_slugs;

        let repos = vec![make_repository_evidence(
            "repo-1",
            Visibility::Public,
            false,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_with_owners(&["@org/team-a", "@individual-user"]),
            ),
        )];

        let pairs = team_owner_slugs(&repos);

        assert_eq!(
            pairs,
            vec![("@org/team-a".to_string(), "team-a".to_string())],
            "user-type owner (@individual-user) must not surface as a team"
        );
    }

    #[test]
    fn owner_repo_map_multi_owner_repo() {
        let repos = vec![make_repository_evidence(
            "shared-repo",
            Visibility::Public,
            false,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_with_owners(&["@org/team-a", "@org/team-b"]),
            ),
        )];
        let map = build_owner_repo_map(&repos);
        assert_eq!(map.len(), 2);
        assert_eq!(map["@org/team-a"].1.len(), 1);
        assert_eq!(map["@org/team-b"].1.len(), 1);
        assert_eq!(map["@org/team-a"].1[0].repository.name, "shared-repo");
        assert_eq!(map["@org/team-b"].1[0].repository.name, "shared-repo");
    }

    #[test]
    fn owner_repo_map_excludes_archived() {
        let repos = vec![make_repository_evidence(
            "archived-repo",
            Visibility::Public,
            true,
            make_checks(
                policy_pass_setting(),
                secret_enabled_observable(false),
                dependabot_enabled(),
                branch_pass(),
                codeowners_with_owners(&["@org/team"]),
            ),
        )];
        let map = build_owner_repo_map(&repos);
        assert!(map.is_empty());
    }

    #[test]
    fn owner_repo_map_case_insensitive_dedup() {
        let repos = vec![
            make_repository_evidence(
                "repo-1",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_with_owners(&["@Org/Team"]),
                ),
            ),
            make_repository_evidence(
                "repo-2",
                Visibility::Public,
                false,
                make_checks(
                    policy_fail(),
                    secret_disabled(),
                    dependabot_disabled(),
                    branch_fail(),
                    codeowners_with_owners(&["@org/team"]),
                ),
            ),
        ];
        let map = build_owner_repo_map(&repos);
        assert_eq!(map.len(), 1);
        let (display, repos_list) = &map["@org/team"];
        assert_eq!(display, "@Org/Team");
        assert_eq!(repos_list.len(), 2);
    }

    #[test]
    fn owner_repo_map_multiple_owners_multiple_repos() {
        let repos = vec![
            make_repository_evidence(
                "repo-1",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_with_owners(&["@org/alpha", "@alice"]),
                ),
            ),
            make_repository_evidence(
                "repo-2",
                Visibility::Private,
                false,
                make_checks(
                    policy_fail(),
                    secret_disabled(),
                    dependabot_disabled(),
                    branch_fail(),
                    codeowners_with_owners(&["@org/alpha"]),
                ),
            ),
        ];
        let map = build_owner_repo_map(&repos);
        assert_eq!(map.len(), 2);
        assert_eq!(map["@org/alpha"].1.len(), 2);
        assert_eq!(map["@alice"].1.len(), 1);
        assert_eq!(map["@alice"].1[0].repository.name, "repo-1");
    }

    #[test]
    fn enrich_lifecycle_zero_repos_leaves_owner_unchanged() {
        let repos: Vec<RepositoryEvidence> = vec![];
        let mut owners = build_owner_metrics(&repos);
        enrich_owner_metrics_with_lifecycle(&mut owners, &repos, &make_timestamp());
        assert!(owners.is_empty());
    }

    #[test]
    fn enrich_lifecycle_all_fresh_repos_non_stale_is_100() {
        let repos = vec![
            make_repo_with_updated_at(
                "fresh-1",
                Some("2026-04-01T12:00:00+00:00"),
                true,
                Some(false),
                true,
                &["@org/team-a"],
            ),
            make_repo_with_updated_at(
                "fresh-2",
                Some("2026-04-08T12:00:00+00:00"),
                true,
                Some(false),
                true,
                &["@org/team-a"],
            ),
        ];
        let mut owners = build_owner_metrics(&repos);
        enrich_owner_metrics_with_lifecycle(&mut owners, &repos, &make_timestamp());

        let om = &owners[0];
        let non_stale = om.per_control_coverage.get("non_stale").unwrap();
        assert_eq!(non_stale.rate, Some(100.0));
        assert_eq!(non_stale.numerator, 2);
        assert_eq!(non_stale.denominator, 2);
    }

    #[test]
    fn enrich_lifecycle_mixed_stale_and_fresh() {
        let repos = vec![
            make_repo_with_updated_at(
                "fresh",
                Some("2026-04-01T12:00:00+00:00"),
                true,
                Some(false),
                true,
                &["@org/team-a"],
            ),
            make_repo_with_updated_at(
                "stale",
                Some("2023-01-01T00:00:00+00:00"),
                true,
                Some(false),
                true,
                &["@org/team-a"],
            ),
        ];
        let mut owners = build_owner_metrics(&repos);
        enrich_owner_metrics_with_lifecycle(&mut owners, &repos, &make_timestamp());

        let non_stale = owners[0].per_control_coverage.get("non_stale").unwrap();
        assert_eq!(non_stale.numerator, 1);
        assert_eq!(non_stale.denominator, 2);
        assert_eq!(non_stale.rate, Some(50.0));
    }

    #[test]
    fn enrich_lifecycle_none_updated_at_treated_as_not_stale() {
        let repos = vec![make_repo_with_updated_at(
            "no-ts",
            None,
            true,
            Some(false),
            true,
            &["@org/team-a"],
        )];
        let mut owners = build_owner_metrics(&repos);
        enrich_owner_metrics_with_lifecycle(&mut owners, &repos, &make_timestamp());

        let non_stale = owners[0].per_control_coverage.get("non_stale").unwrap();
        assert_eq!(non_stale.numerator, 1);
        assert_eq!(non_stale.denominator, 1);
    }

    #[test]
    fn enrich_lifecycle_alert_free_zero_observable_gives_na() {
        let repos = vec![make_repo_with_updated_at(
            "no-scanning",
            Some("2026-04-01T12:00:00+00:00"),
            false,
            None,
            false,
            &["@org/team-a"],
        )];
        let mut owners = build_owner_metrics(&repos);
        enrich_owner_metrics_with_lifecycle(&mut owners, &repos, &make_timestamp());

        let alert_free = owners[0].per_control_coverage.get("alert_free").unwrap();
        assert_eq!(alert_free.denominator, 0);
        assert_eq!(alert_free.rate, None);
    }

    #[test]
    fn enrich_lifecycle_alert_free_all_clean() {
        let repos = vec![
            make_repo_with_updated_at(
                "clean-1",
                Some("2026-04-01T12:00:00+00:00"),
                true,
                Some(false),
                true,
                &["@org/team-a"],
            ),
            make_repo_with_updated_at(
                "clean-2",
                Some("2026-04-01T12:00:00+00:00"),
                true,
                None,
                true,
                &["@org/team-a"],
            ),
        ];
        let mut owners = build_owner_metrics(&repos);
        enrich_owner_metrics_with_lifecycle(&mut owners, &repos, &make_timestamp());

        let alert_free = owners[0].per_control_coverage.get("alert_free").unwrap();
        assert_eq!(alert_free.numerator, 2);
        assert_eq!(alert_free.denominator, 2);
        assert_eq!(alert_free.rate, Some(100.0));
    }

    #[test]
    fn enrich_lifecycle_alert_free_mixed_alerts() {
        let repos = vec![
            make_repo_with_updated_at(
                "clean",
                Some("2026-04-01T12:00:00+00:00"),
                true,
                Some(false),
                true,
                &["@org/team-a"],
            ),
            make_repo_with_updated_at(
                "has-alerts",
                Some("2026-04-01T12:00:00+00:00"),
                true,
                Some(true),
                true,
                &["@org/team-a"],
            ),
            make_repo_with_updated_at(
                "not-observable",
                Some("2026-04-01T12:00:00+00:00"),
                true,
                Some(false),
                false,
                &["@org/team-a"],
            ),
        ];
        let mut owners = build_owner_metrics(&repos);
        enrich_owner_metrics_with_lifecycle(&mut owners, &repos, &make_timestamp());

        let alert_free = owners[0].per_control_coverage.get("alert_free").unwrap();
        assert_eq!(alert_free.denominator, 2);
        assert_eq!(alert_free.numerator, 1);
        assert_eq!(alert_free.rate, Some(50.0));
    }

    #[test]
    fn enrich_lifecycle_idempotent() {
        let repos = vec![make_repo_with_updated_at(
            "repo",
            Some("2026-04-01T12:00:00+00:00"),
            true,
            Some(false),
            true,
            &["@org/team-a"],
        )];
        let mut owners = build_owner_metrics(&repos);
        let ts = make_timestamp();
        enrich_owner_metrics_with_lifecycle(&mut owners, &repos, &ts);
        let non_stale_1 = owners[0]
            .per_control_coverage
            .get("non_stale")
            .unwrap()
            .clone();

        enrich_owner_metrics_with_lifecycle(&mut owners, &repos, &ts);
        let non_stale_2 = owners[0]
            .per_control_coverage
            .get("non_stale")
            .unwrap()
            .clone();

        assert_eq!(non_stale_1.numerator, non_stale_2.numerator);
        assert_eq!(non_stale_1.denominator, non_stale_2.denominator);
        assert_eq!(non_stale_1.rate, non_stale_2.rate);
    }

    #[test]
    fn collection_statistics_counts_archived_repos_from_input() {
        let repos = vec![
            make_repository_evidence(
                "active-pub",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_conforming(),
                ),
            ),
            make_repository_evidence(
                "archived-pub-1",
                Visibility::Public,
                true,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_conforming(),
                ),
            ),
            make_repository_evidence(
                "archived-priv",
                Visibility::Private,
                true,
                make_checks(
                    policy_fail(),
                    secret_disabled(),
                    dependabot_disabled(),
                    branch_fail(),
                    codeowners_absent(),
                ),
            ),
        ];
        let stats = build_collection_statistics(&repos);
        assert_eq!(stats.total_repos, 1);
        assert_eq!(stats.public_repos, 1);
        assert_eq!(
            stats.archived_repos, 2,
            "archived_repos must reflect archived entries present in the input \
             (warm-start glitch: count was hardcoded to 0)"
        );
    }

    #[test]
    fn aggregate_metrics_includes_archived_public_repos_with_security_policy() {
        let repos = vec![
            make_repository_evidence(
                "pub-active-pass",
                Visibility::Public,
                false,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_conforming(),
                ),
            ),
            make_repository_evidence(
                "pub-archived-pass",
                Visibility::Public,
                true,
                make_checks(
                    policy_pass_setting(),
                    secret_enabled_observable(false),
                    dependabot_enabled(),
                    branch_pass(),
                    codeowners_conforming(),
                ),
            ),
            make_repository_evidence(
                "pub-archived-fail",
                Visibility::Public,
                true,
                make_checks(
                    policy_fail(),
                    secret_disabled(),
                    dependabot_disabled(),
                    branch_fail(),
                    codeowners_absent(),
                ),
            ),
        ];
        let metrics = aggregate_metrics(&repos);

        assert_eq!(metrics.policy_counts.via_setting, 2);
        assert_eq!(metrics.policy_counts.missing, 1);
        assert_eq!(metrics.security_policy_coverage.numerator, 2);
        assert_eq!(metrics.security_policy_coverage.denominator, 3);

        let active_only_branch = metrics.branch_protection_coverage;
        assert_eq!(active_only_branch.denominator, 1);
    }
}
