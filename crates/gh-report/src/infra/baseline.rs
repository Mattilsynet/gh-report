//! Baseline file mechanism for minimizing API calls across runs.
//!
//! A baseline persists per-repository evidence from the most recent successful
//! run. On subsequent runs, if a repository's `updated_at` timestamp matches
//! the baseline entry, the previous evidence is reused without re-evaluating
//! the repository via the GitHub API.
//!
//! # Staleness window
//!
//! `updated_at` is fetched at inventory time; a repository could change
//! between inventory and evaluation. For large organizations this window
//! can be minutes to hours. The `inventory_fetched_at` field on
//! [`AssessmentMetadata`](crate::domain::evidence::AssessmentMetadata)
//! makes this observable.
//!
//! # Storage
//!
//! The baseline is stored as `baseline.msgpack` (`MessagePack`) in `store_dir`
//! (persists across runs). Schema version is validated before reuse — on
//! mismatch the baseline is discarded.
//! Previous versions stored the baseline as `baseline.json`; that format
//! is no longer supported.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use tracing::{debug, info, warn};

use crate::config;
use crate::domain::checks::{
    BranchProtectionStatus, CodeownersStatus, DependabotStatus, SecretScanningStatus,
    SecurityPolicyStatus,
};
use crate::domain::evidence::RepositoryEvidence;
use crate::error::PersistenceError;
use cherry_pit_storage::atomic_write_bytes;

/// A single repository's baseline entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineEntry {
    /// `updated_at` timestamp from the GitHub API at the time this
    /// evidence was collected.
    pub updated_at: String,
    /// The full repository evidence from the previous run.
    pub evidence: RepositoryEvidence,
}

/// Persisted baseline: a map of inventory keys to evidence entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline {
    /// Schema version — must match [`config::EVIDENCE_SCHEMA_VERSION`]
    /// for the baseline to be considered valid.
    pub schema_version: String,
    /// Per-repository baseline entries keyed by inventory key.
    pub entries: HashMap<String, BaselineEntry>,
}

/// Maximum baseline file size in bytes (200 MB).
const MAX_BASELINE_FILE_BYTES: u64 = 200 * 1024 * 1024;

/// Return the canonical path for the baseline file (`MessagePack`).
#[must_use]
pub fn baseline_path(store_dir: &Path) -> PathBuf {
    store_dir.join("baseline.msgpack")
}

/// Load a baseline from `store_dir/baseline.msgpack`.
///
/// Returns `None` if no baseline exists, the file is corrupt, exceeds the
/// size limit, or has a schema version mismatch. All non-fatal failures
/// are logged as warnings.
pub fn load_baseline(store_dir: &Path) -> Option<Baseline> {
    let path = baseline_path(store_dir);

    if !path.exists() {
        debug!("no baseline file found");
        return None;
    }

    // Size guard.
    match std::fs::metadata(&path) {
        Ok(meta) if meta.len() > MAX_BASELINE_FILE_BYTES => {
            warn!(
                size = meta.len(),
                max = MAX_BASELINE_FILE_BYTES,
                "baseline file exceeds size limit — discarding"
            );
            return None;
        }
        Err(e) => {
            warn!(error = %e, "failed to stat baseline file — discarding");
            return None;
        }
        _ => {}
    }

    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(e) => {
            warn!(error = %e, "failed to read baseline file — discarding");
            return None;
        }
    };

    let baseline: Baseline = match rmp_serde::from_slice(&data) {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "failed to parse baseline file — discarding");
            return None;
        }
    };

    if baseline.schema_version != config::EVIDENCE_SCHEMA_VERSION {
        warn!(
            found = %baseline.schema_version,
            expected = config::EVIDENCE_SCHEMA_VERSION,
            "baseline schema version mismatch — discarding"
        );
        return None;
    }

    info!(entries = baseline.entries.len(), "baseline loaded");
    Some(baseline)
}

/// Save a baseline atomically to `store_dir/baseline.msgpack`.
///
/// # Errors
///
/// Returns [`PersistenceError`] if serialization or the atomic write fails.
pub fn save_baseline(store_dir: &Path, baseline: &Baseline) -> Result<(), PersistenceError> {
    let path = baseline_path(store_dir);
    let data =
        rmp_serde::to_vec_named(baseline).map_err(|e| PersistenceError::AtomicWriteFailed {
            reason: format!("failed to serialize baseline: {e}"),
        })?;
    atomic_write_bytes(&path, &data)?;
    info!(
        entries = baseline.entries.len(),
        bytes = data.len(),
        path = %path.display(),
        "baseline saved"
    );
    Ok(())
}

/// Dump a baseline file to stdout as pretty-printed JSON.
///
/// Used by the `--dump-baseline` CLI flag for inspecting `MessagePack` baselines.
///
/// # Errors
///
/// Returns an error message if the file cannot be read, parsed, or serialized.
pub fn dump_baseline(store_dir: &Path) -> Result<String, String> {
    let path = baseline_path(store_dir);
    if !path.exists() {
        return Err(format!("baseline file not found: {}", path.display()));
    }
    let data =
        std::fs::read(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let baseline: Baseline = rmp_serde::from_slice(&data)
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
    serde_json::to_string_pretty(&baseline)
        .map_err(|e| format!("failed to serialize baseline as JSON: {e}"))
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

/// Build a new baseline from a completed collection run.
///
/// Only includes entries for repositories that were either:
/// - Freshly evaluated in the current run, OR
/// - Reused from a previous baseline (and thus validated in the current run).
///
/// Excludes entries where all 5 checks are `Unknown` (total collection
/// failures from rate-limit halts or panics) to prevent permanent caching
/// of unresolved results.
///
/// Does NOT blindly persist stale baseline entries that were neither
/// re-evaluated nor reused.
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
    use std::sync::Arc;

    fn make_evidence_with_updated_at(name: &str, updated_at: Option<&str>) -> RepositoryEvidence {
        let mut ev = test_fixtures::all_passing_evidence(name);
        Arc::make_mut(&mut ev.repository).updated_at = updated_at.map(String::from);
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

    // ── load / save round-trip ─────────────────────────────────────

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::TempDir::new().unwrap();
        let ev = make_evidence_with_updated_at("repo-rt", Some("2026-04-09T12:00:00Z"));
        let baseline = build_baseline(&[ev]);

        save_baseline(dir.path(), &baseline).unwrap();
        let loaded = load_baseline(dir.path()).unwrap();

        assert_eq!(loaded.schema_version, baseline.schema_version);
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(
            loaded.entries["id-repo-rt"].updated_at,
            "2026-04-09T12:00:00Z"
        );
    }

    #[test]
    fn load_missing_baseline_returns_none() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(load_baseline(dir.path()).is_none());
    }

    #[test]
    fn load_corrupt_baseline_returns_none() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = baseline_path(dir.path());
        std::fs::write(&path, "not valid msgpack").unwrap();
        assert!(load_baseline(dir.path()).is_none());
    }

    #[test]
    fn load_wrong_schema_version_returns_none() {
        let dir = tempfile::TempDir::new().unwrap();
        let baseline = Baseline {
            schema_version: "0.0".to_string(),
            entries: HashMap::new(),
        };
        let data = rmp_serde::to_vec(&baseline).unwrap();
        std::fs::write(baseline_path(dir.path()), data).unwrap();
        assert!(load_baseline(dir.path()).is_none());
    }

    // ── baseline_path ──────────────────────────────────────────────

    #[test]
    fn baseline_path_is_in_store_dir() {
        let path = baseline_path(Path::new("/tmp/store"));
        assert_eq!(path, PathBuf::from("/tmp/store/baseline.msgpack"));
    }

    // ── dump_baseline ──────────────────────────────────────────────

    #[test]
    fn dump_baseline_outputs_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let ev = make_evidence_with_updated_at("repo-dump", Some("2026-04-09T12:00:00Z"));
        let baseline = build_baseline(&[ev]);
        save_baseline(dir.path(), &baseline).unwrap();

        let json_str = dump_baseline(dir.path()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert!(parsed["entries"].is_object());
    }

    #[test]
    fn dump_baseline_missing_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = dump_baseline(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    // ── baseline + checkpoint composition ──────────────────────────

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
        Arc::make_mut(&mut ev.repository).updated_at = Some("2026-04-09T12:00:00Z".to_string());
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
        Arc::make_mut(&mut ev.repository).updated_at = Some("2026-04-09T12:00:00Z".to_string());
        ev.checks.branch_protection.status = BranchProtectionStatus::Unknown;

        let baseline = build_baseline(&[ev]);
        assert_eq!(baseline.entries.len(), 1);
    }
}
