//! Baseline algorithms for evidence reuse and `--dump-baseline` JSON shape.
//!
//! δ.3c-ii retired on-disk persistence (`baseline.msgpack`). The baseline
//! is now reconstructed from the projection (in-memory; rebuilt at boot via
//! event-log replay per CHE-0051:R5 + CHE-0048:R2). This module keeps:
//!
//! - `Baseline` / `BaselineEntry` — JSON shape for `--dump-baseline` output
//!   (byte-equivalent to the pre-δ.3c-ii dump).
//! - `build_baseline` — pure builder over `&[RepositoryEvidence]`; reused
//!   by `--dump-baseline` (replay → project → build → JSON) and by tests.
//! - `should_reuse` — staleness comparator (called by `reuse_from_baseline`
//!   in `app/collect.rs`).
//! - `is_total_failure` — total-failure filter used by `build_baseline`.
//!
//! # Staleness window
//!
//! `updated_at` is fetched at inventory time; a repository could change
//! between inventory and evaluation. For large organizations this window
//! can be minutes to hours. The `inventory_fetched_at` field on
//! [`AssessmentMetadata`](crate::domain::evidence::AssessmentMetadata)
//! makes this observable.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use tracing::debug;

use crate::config;
use crate::domain::checks::{
    BranchProtectionStatus, CodeownersStatus, DependabotStatus, SecretScanningStatus,
    SecurityPolicyStatus,
};
use crate::domain::evidence::RepositoryEvidence;

/// A single repository's baseline entry.
///
/// Retained as the JSON shape for `--dump-baseline` output. Field order
/// and names define the byte-equivalent dump contract; do not reorder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineEntry {
    /// `updated_at` timestamp from the GitHub API at the time this
    /// evidence was collected.
    pub updated_at: String,
    /// The full repository evidence from the previous run.
    pub evidence: RepositoryEvidence,
}

/// Baseline JSON shape: a map of inventory keys to evidence entries.
///
/// δ.3c-ii: no longer persisted to disk. Constructed in-memory by
/// [`build_baseline`] from the projection (via event-log replay) for
/// the `--dump-baseline` CLI flag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline {
    /// Schema version — stamped from [`config::EVIDENCE_SCHEMA_VERSION`].
    pub schema_version: String,
    /// Per-repository baseline entries keyed by inventory key.
    pub entries: HashMap<String, BaselineEntry>,
}

/// Determine whether a baseline entry can be reused for a given repository.
///
/// Reuse is safe when:
/// - The baseline entry has a non-empty `updated_at` value.
/// - The current repository has a non-empty `updated_at` value.
/// - Both values are identical (repository has not changed since the baseline).
#[must_use]
pub fn should_reuse(baseline_updated_at: &str, current_updated_at: Option<&str>) -> bool {
    if baseline_updated_at.is_empty() {
        return false;
    }
    match current_updated_at {
        Some(current) if !current.is_empty() => baseline_updated_at == current,
        _ => false,
    }
}

/// Check whether a repository evidence entry represents a total collection
/// failure (all 5 checks are `Unknown`).
///
/// Rate-limit-halted repos produce `failure_evidence` where every check
/// status is `Unknown`. Caching these entries would persist `Unknown`
/// results forever (until `updated_at` changes). Excluding them from
/// the baseline forces re-evaluation on the next run.
///
/// Permission-denied repos typically have only 1-2 `Unknown` checks and
/// are correctly cached.
fn is_total_failure(evidence: &RepositoryEvidence) -> bool {
    let c = &evidence.checks;
    c.security_policy.status == SecurityPolicyStatus::Unknown
        && c.secret_scanning.status == SecretScanningStatus::Unknown
        && c.dependabot_security_updates.status == DependabotStatus::Unknown
        && c.branch_protection.status == BranchProtectionStatus::Unknown
        && c.codeowners.status == CodeownersStatus::Unknown
}

/// Build a baseline from a slice of evidence (typically the projection's
/// `repositories.values()` at `--dump-baseline` time).
///
/// Only includes entries for repositories that have a non-empty
/// `updated_at`. Excludes entries where all 5 checks are `Unknown`
/// (total collection failures) to keep the dump informative.
#[must_use]
pub fn build_baseline(repositories: &[RepositoryEvidence]) -> Baseline {
    let mut entries = HashMap::new();

    for repo_evidence in repositories {
        if is_total_failure(repo_evidence) {
            debug!(
                repo = %repo_evidence.repository.name,
                "skipping total-failure entry from baseline"
            );
            continue;
        }

        if let Some(ref updated_at) = repo_evidence.repository.updated_at
            && !updated_at.is_empty()
        {
            entries.insert(
                repo_evidence.repository.inventory_key.clone(),
                BaselineEntry {
                    updated_at: updated_at.clone(),
                    evidence: repo_evidence.clone(),
                },
            );
        }
    }

    Baseline {
        schema_version: config::EVIDENCE_SCHEMA_VERSION.to_string(),
        entries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures;

    fn make_evidence_with_updated_at(name: &str, updated_at: Option<&str>) -> RepositoryEvidence {
        let mut ev = test_fixtures::all_passing_evidence(name);
        ev.repository.updated_at = updated_at.map(String::from);
        ev
    }

    // ── should_reuse tests ─────────────────────────────────────────

    #[test]
    fn reuse_when_updated_at_matches() {
        assert!(should_reuse(
            "2026-04-09T12:00:00Z",
            Some("2026-04-09T12:00:00Z")
        ));
    }

    #[test]
    fn no_reuse_when_updated_at_differs() {
        assert!(!should_reuse(
            "2026-04-09T12:00:00Z",
            Some("2026-04-10T12:00:00Z")
        ));
    }

    #[test]
    fn no_reuse_when_current_is_none() {
        assert!(!should_reuse("2026-04-09T12:00:00Z", None));
    }

    #[test]
    fn no_reuse_when_current_is_empty() {
        assert!(!should_reuse("2026-04-09T12:00:00Z", Some("")));
    }

    #[test]
    fn no_reuse_when_baseline_is_empty() {
        assert!(!should_reuse("", Some("2026-04-09T12:00:00Z")));
    }

    // ── build_baseline tests ───────────────────────────────────────

    #[test]
    fn build_baseline_includes_repos_with_updated_at() {
        let ev = make_evidence_with_updated_at("repo-a", Some("2026-04-09T12:00:00Z"));
        let baseline = build_baseline(&[ev]);
        assert_eq!(baseline.entries.len(), 1);
        assert!(baseline.entries.contains_key("id-repo-a"));
        assert_eq!(
            baseline.entries["id-repo-a"].updated_at,
            "2026-04-09T12:00:00Z"
        );
    }

    #[test]
    fn build_baseline_excludes_repos_without_updated_at() {
        let ev = make_evidence_with_updated_at("repo-b", None);
        let baseline = build_baseline(&[ev]);
        assert!(baseline.entries.is_empty());
    }

    #[test]
    fn build_baseline_excludes_repos_with_empty_updated_at() {
        let ev = make_evidence_with_updated_at("repo-c", Some(""));
        let baseline = build_baseline(&[ev]);
        assert!(baseline.entries.is_empty());
    }

    #[test]
    fn build_baseline_has_current_schema_version() {
        let baseline = build_baseline(&[]);
        assert_eq!(baseline.schema_version, config::EVIDENCE_SCHEMA_VERSION);
    }

    #[test]
    fn baseline_only_includes_evaluated_repos() {
        let ev1 = make_evidence_with_updated_at("evaluated", Some("2026-04-09T00:00:00Z"));
        let ev2 = make_evidence_with_updated_at("also-evaluated", Some("2026-04-09T01:00:00Z"));
        let baseline = build_baseline(&[ev1, ev2]);
        assert_eq!(baseline.entries.len(), 2);

        // A repo not in the list is not in the baseline.
        assert!(!baseline.entries.contains_key("id-missing"));
    }

    #[test]
    fn build_baseline_excludes_total_failure() {
        // Create a total-failure evidence (all 5 checks Unknown).
        let mut ev = test_fixtures::all_passing_evidence("halted-repo");
        ev.repository.updated_at = Some("2026-04-09T12:00:00Z".to_string());
        ev.checks.security_policy.status = SecurityPolicyStatus::Unknown;
        ev.checks.secret_scanning.status = SecretScanningStatus::Unknown;
        ev.checks.dependabot_security_updates.status = DependabotStatus::Unknown;
        ev.checks.branch_protection.status = BranchProtectionStatus::Unknown;
        ev.checks.codeowners.status = CodeownersStatus::Unknown;

        let baseline = build_baseline(&[ev]);
        assert!(
            baseline.entries.is_empty(),
            "total-failure entries must be excluded from baseline"
        );
    }

    #[test]
    fn is_total_failure_false_when_policy_not_applicable() {
        // NotApplicable policy + 4 Unknown is NOT a total failure.
        // NotApplicable != Unknown, so the conjunction fails.
        let mut ev = test_fixtures::all_passing_evidence("na-repo");
        ev.checks.security_policy.status = SecurityPolicyStatus::NotApplicable;
        ev.checks.secret_scanning.status = SecretScanningStatus::Unknown;
        ev.checks.dependabot_security_updates.status = DependabotStatus::Unknown;
        ev.checks.branch_protection.status = BranchProtectionStatus::Unknown;
        ev.checks.codeowners.status = CodeownersStatus::Unknown;

        assert!(
            !is_total_failure(&ev),
            "NotApplicable policy + 4 Unknown should NOT be total failure"
        );
    }

    #[test]
    fn build_baseline_keeps_partial_failure() {
        // Only 1 check is Unknown — should still be cached.
        let mut ev = test_fixtures::all_passing_evidence("partial-repo");
        ev.repository.updated_at = Some("2026-04-09T12:00:00Z".to_string());
        ev.checks.branch_protection.status = BranchProtectionStatus::Unknown;

        let baseline = build_baseline(&[ev]);
        assert_eq!(baseline.entries.len(), 1);
    }
}
