//! Checkpoint/resume logic for long-running collection runs.
//!
//! A checkpoint captures per-repository results collected so far in a run.
//! If the process crashes or is interrupted, the next run can resume from
//! the checkpoint rather than re-collecting every repository.
//!
//! Checkpoint files are date-scoped (one per report date) and versioned.
//! A checkpoint is invalidated when:
//! - The schema version changes (semantics changed, results must be reprocessed).
//! - The secret scanning snapshot signature changes (org-level data changed).
//! - The date portion of the run timestamp differs (new day).
//!
//! Checkpoint files only contain checkpoint-resumed and freshly-evaluated
//! entries — baseline-reused entries are excluded because they are
//! recoverable from `store/baseline.msgpack` on restart.  After a
//! successful baseline save, [`remove_checkpoint`] deletes the checkpoint
//! file to prevent accumulation.
//!
//! **Signature scheme:** The secret scanning snapshot signature is a SHA-256
//! hash of the org-level alert summary (minus the `run_timestamp` field).
//! This ensures that if the upstream org-level data changed between runs on
//! the same day, per-repo results referencing the old data are discarded.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use tracing::{debug, warn};

use crate::config;
use crate::domain::evidence::RepositoryEvidence;
use crate::error::PersistenceError;
use cherry_pit_storage::atomic_write_bytes;

// Re-export signature utilities for backward compatibility.
pub use cherry_pit_storage::build_snapshot_signature;

// ── Data types ──────────────────────────────────────────────────────

/// On-disk checkpoint representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Evidence schema version at the time the checkpoint was written.
    pub schema_version: String,
    /// ISO-8601 run timestamp for this checkpoint.
    pub run_timestamp: String,
    /// SHA-256 signature of the secret scanning snapshot used.
    pub secret_scanning_snapshot_signature: String,
    /// Per-repository results keyed by `inventory_key`.
    ///
    /// Values are `Arc`-wrapped to allow cheap cloning during checkpoint
    /// snapshots — workers share references instead of deep-copying
    /// entire evidence structs.  serde handles `Arc<T>` transparently
    /// (serialises/deserialises as `T`), so the on-disk format is unchanged.
    pub results: HashMap<String, Arc<RepositoryEvidence>>,
}

// ── Construction ────────────────────────────────────────────────────

/// Create an empty checkpoint with the current schema version.
#[must_use]
pub fn empty_checkpoint(run_timestamp: &str) -> Checkpoint {
    Checkpoint {
        schema_version: config::EVIDENCE_SCHEMA_VERSION.to_string(),
        run_timestamp: run_timestamp.to_string(),
        secret_scanning_snapshot_signature: build_snapshot_signature(None),
        results: HashMap::new(),
    }
}

/// Magic header for binary (`MessagePack`) checkpoint files.
const CHECKPOINT_MAGIC: &[u8; 4] = b"CKPT";

// ── Loading ─────────────────────────────────────────────────────────

/// Load a checkpoint from disk.
///
/// Expects binary format: 4-byte magic header `b"CKPT"` followed by a
/// `MessagePack` body (written by [`save_checkpoint`]).
///
/// Returns an empty checkpoint if the file does not exist.
///
/// # Errors
///
/// Returns `PersistenceError` if the file exists but cannot be read or parsed,
/// or if the schema is unsupported.
pub fn load_checkpoint(path: &Path) -> Result<Checkpoint, PersistenceError> {
    if !path.exists() {
        return Ok(empty_checkpoint(""));
    }

    // Guard against corrupt or unexpectedly large checkpoint files.
    let metadata = std::fs::metadata(path).map_err(PersistenceError::Io)?;
    if metadata.len() > config::MAX_CHECKPOINT_FILE_BYTES {
        return Err(PersistenceError::LoadFailed {
            reason: format!(
                "checkpoint file too large: {} bytes (max: {})",
                metadata.len(),
                config::MAX_CHECKPOINT_FILE_BYTES
            ),
        });
    }

    let raw = std::fs::read(path).map_err(PersistenceError::Io)?;

    let checkpoint: Checkpoint = if raw.starts_with(CHECKPOINT_MAGIC) {
        // Binary (MessagePack) format.
        rmp_serde::from_slice(&raw[CHECKPOINT_MAGIC.len()..]).map_err(|e| {
            PersistenceError::LoadFailed {
                reason: format!("checkpoint binary data is corrupt: {e}"),
            }
        })?
    } else {
        return Err(PersistenceError::LoadFailed {
            reason: "checkpoint file missing CKPT magic header".to_string(),
        });
    };

    // Validate schema version.
    if checkpoint.schema_version != config::EVIDENCE_SCHEMA_VERSION {
        return Err(PersistenceError::LoadFailed {
            reason: format!(
                "checkpoint has unsupported schema version: {} (expected {})",
                checkpoint.schema_version,
                config::EVIDENCE_SCHEMA_VERSION
            ),
        });
    }

    // Validate snapshot signature field exists (non-empty).
    if checkpoint.secret_scanning_snapshot_signature.is_empty() {
        return Err(PersistenceError::LoadFailed {
            reason: "checkpoint is missing secret scanning snapshot signature".to_string(),
        });
    }

    Ok(checkpoint)
}

/// Save a checkpoint atomically in binary (`MessagePack`) format.
///
/// Writes a 4-byte magic header `b"CKPT"` followed by the `MessagePack` body.
/// This is ~5-10× faster and ~2-3× smaller than JSON, reducing lock hold
/// time in the debounce task.
///
/// # Errors
///
/// Returns `PersistenceError` if the atomic write fails.
pub fn save_checkpoint(path: &Path, checkpoint: &Checkpoint) -> Result<(), PersistenceError> {
    let msgpack =
        rmp_serde::to_vec_named(checkpoint).map_err(|e| PersistenceError::AtomicWriteFailed {
            reason: format!("failed to serialize checkpoint: {e}"),
        })?;

    let mut buf = Vec::with_capacity(CHECKPOINT_MAGIC.len() + msgpack.len());
    buf.extend_from_slice(CHECKPOINT_MAGIC);
    buf.extend_from_slice(&msgpack);

    atomic_write_bytes(path, &buf)
}

// ── Validation ──────────────────────────────────────────────────────

/// Validate that a `RepositoryEvidence` has all expected check fields
/// with valid status values.
#[must_use]
pub fn result_has_expected_checks(evidence: &RepositoryEvidence) -> bool {
    use crate::domain::checks::{CodeownersStatus, SecurityPolicyEvidence, SecurityPolicyStatus};

    let checks = &evidence.checks;

    // Validate CODEOWNERS consistency:
    // - Conforming must have path == CONFORMING_CODEOWNERS_PATH
    // - NonConforming must have path == NON_CONFORMING_CODEOWNERS_PATH
    // - Absent/Unknown must have path == None
    match checks.codeowners.status {
        CodeownersStatus::Conforming => {
            if checks.codeowners.path.as_deref() != Some(config::CONFORMING_CODEOWNERS_PATH) {
                return false;
            }
        }
        CodeownersStatus::NonConforming => {
            if checks.codeowners.path.as_deref() != Some(config::NON_CONFORMING_CODEOWNERS_PATH) {
                return false;
            }
        }
        CodeownersStatus::Absent | CodeownersStatus::Unknown => {
            if checks.codeowners.path.is_some() {
                return false;
            }
        }
    }

    // Validate security policy consistency:
    // - Pass must have evidence Setting or File
    // - Fail must have evidence None
    match checks.security_policy.status {
        SecurityPolicyStatus::Pass => {
            if !matches!(
                checks.security_policy.evidence,
                SecurityPolicyEvidence::Setting | SecurityPolicyEvidence::File
            ) {
                return false;
            }
        }
        SecurityPolicyStatus::Fail => {
            if checks.security_policy.evidence != SecurityPolicyEvidence::Absent {
                return false;
            }
        }
        SecurityPolicyStatus::Unknown => {}
        SecurityPolicyStatus::NotApplicable => {
            if checks.security_policy.evidence != SecurityPolicyEvidence::NotApplicable {
                return false;
            }
        }
    }

    // All timestamps must be non-empty.
    if checks.security_policy.timestamp.is_empty()
        || checks.secret_scanning.timestamp.is_empty()
        || checks.dependabot_security_updates.timestamp.is_empty()
        || checks.branch_protection.timestamp.is_empty()
        || checks.codeowners.timestamp.is_empty()
    {
        return false;
    }

    true
}

/// Re-stamp all check timestamps in a `RepositoryEvidence` to a new run timestamp.
///
/// This is used when resuming a checkpoint from a prior run on the same day
/// but with a different run timestamp (e.g., a retry).
pub fn restamp_evidence(evidence: &mut RepositoryEvidence, run_timestamp: &str) {
    evidence.checks.security_policy.timestamp = run_timestamp.to_string();
    evidence.checks.secret_scanning.timestamp = run_timestamp.to_string();
    evidence.checks.dependabot_security_updates.timestamp = run_timestamp.to_string();
    evidence.checks.branch_protection.timestamp = run_timestamp.to_string();
    evidence.checks.codeowners.timestamp = run_timestamp.to_string();
}

// ── Resume logic ────────────────────────────────────────────────────

/// Outcome of attempting to resume from a checkpoint.
#[derive(Debug)]
pub struct ResumeResult {
    /// Results that were successfully resumed from the checkpoint.
    pub completed: HashMap<String, Arc<RepositoryEvidence>>,
    /// Whether the checkpoint was rotated (moved to `.corrupt`) due to
    /// schema mismatch, parse error, or other invalidation.
    pub rotated: bool,
}

/// Attempt to resume a checkpoint.
///
/// Checkpoint resume decision logic:
///
/// 1. If `resume` is false, start fresh.
/// 2. Load the checkpoint; on parse/schema error, rotate it and start fresh.
/// 3. If the checkpoint date differs from the current run date, start fresh.
/// 4. If the snapshot signature differs, start fresh (org data changed).
/// 5. Filter results to only those whose `inventory_key` is in `current_keys`.
/// 6. Filter results to only those passing `result_has_expected_checks`.
/// 7. Re-stamp timestamps if the run timestamp changed but the date is the same.
///
/// # Errors
///
/// Returns `PersistenceError` only for I/O failures during rotation.
pub fn try_resume<S: ::std::hash::BuildHasher>(
    checkpoint_path: &Path,
    run_timestamp: &str,
    snapshot_signature: &str,
    current_keys: &std::collections::HashSet<String, S>,
    resume: bool,
) -> Result<ResumeResult, PersistenceError> {
    if !resume {
        debug!("checkpoint resume disabled");
        return Ok(ResumeResult {
            completed: HashMap::new(),
            rotated: false,
        });
    }

    let Ok(checkpoint) = load_checkpoint(checkpoint_path) else {
        let rotated = rotate_corrupt_checkpoint(checkpoint_path)?;
        return Ok(ResumeResult {
            completed: HashMap::new(),
            rotated,
        });
    };

    // Date mismatch → start fresh (new day).
    let checkpoint_date = &checkpoint.run_timestamp.get(..10).unwrap_or("");
    let current_date = &run_timestamp.get(..10).unwrap_or("");
    if !checkpoint_date.is_empty() && checkpoint_date != current_date {
        debug!(
            checkpoint_date = %checkpoint_date,
            current_date = %current_date,
            "checkpoint date mismatch, starting fresh"
        );
        return Ok(ResumeResult {
            completed: HashMap::new(),
            rotated: false,
        });
    }

    // Snapshot signature mismatch → start fresh (org data changed).
    if checkpoint.secret_scanning_snapshot_signature != snapshot_signature {
        debug!("checkpoint snapshot signature mismatch, starting fresh");
        return Ok(ResumeResult {
            completed: HashMap::new(),
            rotated: false,
        });
    }

    // Determine whether results need re-stamping.
    let needs_restamp =
        !checkpoint.run_timestamp.is_empty() && checkpoint.run_timestamp != run_timestamp;

    // Filter results: only current inventory keys with valid checks.
    let loaded = checkpoint.results.len();
    let mut completed: HashMap<String, Arc<RepositoryEvidence>> = checkpoint
        .results
        .into_iter()
        .filter(|(key, value)| current_keys.contains(key) && result_has_expected_checks(value))
        .collect();

    debug!(
        loaded,
        accepted = completed.len(),
        "checkpoint entries filtered"
    );

    // Re-stamp if needed.  `Arc::make_mut` clones only if the refcount > 1,
    // which it won't be here (we own the only handle after `into_iter`), so
    // this is effectively zero-cost.
    if needs_restamp {
        for evidence in completed.values_mut() {
            restamp_evidence(Arc::make_mut(evidence), run_timestamp);
        }
    }

    Ok(ResumeResult {
        completed,
        rotated: false,
    })
}

/// Rotate a corrupt checkpoint by renaming it to `{filename}.corrupt`.
///
/// Returns `true` if a file was actually rotated.
fn rotate_corrupt_checkpoint(path: &Path) -> Result<bool, PersistenceError> {
    if !path.exists() {
        return Ok(false);
    }

    let corrupt_name = format!(
        "{}.corrupt",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("checkpoint")
    );
    let corrupt_path = path.with_file_name(corrupt_name);
    std::fs::rename(path, &corrupt_path).map_err(PersistenceError::Io)?;
    warn!(
        original = %path.display(),
        rotated = %corrupt_path.display(),
        "rotated unusable checkpoint"
    );
    Ok(true)
}

/// Remove a checkpoint file after a successful run.
///
/// Best-effort cleanup: a missing file is treated as success (debug log),
/// and permission or I/O errors are logged at `warn` level without failing
/// the run.  The checkpoint is only redundant once the baseline has been
/// saved, so callers should invoke this *after* a successful baseline write.
pub fn remove_checkpoint(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {
            debug!(path = %path.display(), "checkpoint removed after successful run");
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            debug!(path = %path.display(), "no checkpoint file to remove");
        }
        Err(e) => {
            warn!(
                path = %path.display(),
                error = %e,
                "failed to remove checkpoint file"
            );
        }
    }
}

/// Compute the checkpoint file path for a given date and working directory.
#[must_use]
pub fn checkpoint_path(store_dir: &Path, report_date: &str) -> PathBuf {
    store_dir.join(format!("checkpoint-{report_date}.ckpt"))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::checks::CodeownersStatus;
    use crate::test_fixtures;
    use tempfile::TempDir;

    // all_passing_evidence is imported from test_fixtures.

    // ── empty_checkpoint ────────────────────────────────────────────

    #[test]
    fn empty_checkpoint_has_current_schema() {
        let cp = empty_checkpoint("2026-04-09T12:00:00+00:00");
        assert_eq!(cp.schema_version, config::EVIDENCE_SCHEMA_VERSION);
        assert!(cp.results.is_empty());
        assert!(!cp.secret_scanning_snapshot_signature.is_empty());
    }

    // ── result_has_expected_checks ──────────────────────────────────

    #[test]
    fn valid_evidence_passes_check() {
        assert!(result_has_expected_checks(
            &test_fixtures::all_passing_evidence("repo1")
        ));
    }

    #[test]
    fn conforming_codeowners_wrong_path_fails() {
        let mut ev = test_fixtures::all_passing_evidence("repo1");
        ev.checks.codeowners.status = CodeownersStatus::Conforming;
        ev.checks.codeowners.path = Some("CODEOWNERS".to_string()); // wrong
        assert!(!result_has_expected_checks(&ev));
    }

    #[test]
    fn non_conforming_codeowners_wrong_path_fails() {
        let mut ev = test_fixtures::all_passing_evidence("repo1");
        ev.checks.codeowners.status = CodeownersStatus::NonConforming;
        ev.checks.codeowners.path = Some(".github/CODEOWNERS".to_string()); // wrong
        assert!(!result_has_expected_checks(&ev));
    }

    #[test]
    fn absent_codeowners_with_path_fails() {
        let mut ev = test_fixtures::all_passing_evidence("repo1");
        ev.checks.codeowners.status = CodeownersStatus::Absent;
        ev.checks.codeowners.path = Some("CODEOWNERS".to_string()); // should be None
        assert!(!result_has_expected_checks(&ev));
    }

    #[test]
    fn empty_timestamp_fails() {
        let mut ev = test_fixtures::all_passing_evidence("repo1");
        ev.checks.security_policy.timestamp = String::new();
        assert!(!result_has_expected_checks(&ev));
    }

    // ── restamp_evidence ────────────────────────────────────────────

    #[test]
    fn restamp_updates_all_timestamps() {
        let mut ev = test_fixtures::all_passing_evidence("repo1");
        restamp_evidence(&mut ev, "2026-04-10T06:00:00+00:00");
        assert_eq!(
            ev.checks.security_policy.timestamp,
            "2026-04-10T06:00:00+00:00"
        );
        assert_eq!(
            ev.checks.secret_scanning.timestamp,
            "2026-04-10T06:00:00+00:00"
        );
        assert_eq!(
            ev.checks.dependabot_security_updates.timestamp,
            "2026-04-10T06:00:00+00:00"
        );
        assert_eq!(
            ev.checks.branch_protection.timestamp,
            "2026-04-10T06:00:00+00:00"
        );
        assert_eq!(ev.checks.codeowners.timestamp, "2026-04-10T06:00:00+00:00");
    }

    // ── save / load round-trip ──────────────────────────────────────

    #[test]
    fn save_and_load_round_trip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("checkpoint.ckpt");

        let mut cp = empty_checkpoint("2026-04-09T12:00:00+00:00");
        cp.results.insert(
            "id-repo1".to_string(),
            Arc::new(test_fixtures::all_passing_evidence("repo1")),
        );

        save_checkpoint(&path, &cp).unwrap();
        let loaded = load_checkpoint(&path).unwrap();

        assert_eq!(loaded.schema_version, cp.schema_version);
        assert_eq!(loaded.run_timestamp, cp.run_timestamp);
        assert_eq!(
            loaded.secret_scanning_snapshot_signature,
            cp.secret_scanning_snapshot_signature
        );
        assert_eq!(loaded.results.len(), 1);
        assert!(loaded.results.contains_key("id-repo1"));
    }

    #[test]
    fn load_nonexistent_returns_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.json");
        let cp = load_checkpoint(&path).unwrap();
        assert!(cp.results.is_empty());
        assert_eq!(cp.run_timestamp, "");
    }

    #[test]
    fn load_rejects_non_binary_data() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("checkpoint.ckpt");
        std::fs::write(&path, "{not-binary").unwrap();
        let err = load_checkpoint(&path).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("magic header"),
            "expected 'magic header' in error: {msg}"
        );
    }

    #[test]
    fn load_wrong_schema_fails() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("checkpoint.ckpt");

        // Build a checkpoint with a wrong schema version and write in binary format.
        let mut cp = empty_checkpoint("ts");
        cp.schema_version = "1.0".to_string();
        // Must also give it a non-empty signature to pass that check.
        cp.secret_scanning_snapshot_signature = "sig".to_string();

        let msgpack = rmp_serde::to_vec_named(&cp).unwrap();
        let mut buf = Vec::with_capacity(CHECKPOINT_MAGIC.len() + msgpack.len());
        buf.extend_from_slice(CHECKPOINT_MAGIC);
        buf.extend_from_slice(&msgpack);
        std::fs::write(&path, &buf).unwrap();

        assert!(load_checkpoint(&path).is_err());
    }

    #[test]
    fn load_rejects_oversized_checkpoint_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("big-checkpoint.ckpt");
        // Create a file that exceeds MAX_CHECKPOINT_FILE_BYTES.
        // We only need the metadata to report the right size, so write a
        // sparse-ish file just over the limit.
        let size = config::MAX_CHECKPOINT_FILE_BYTES + 1;
        let f = std::fs::File::create(&path).unwrap();
        f.set_len(size).unwrap();
        drop(f);

        let err = load_checkpoint(&path).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("too large"),
            "expected 'too large' in error: {msg}"
        );
    }

    // ── try_resume ──────────────────────────────────────────────────

    #[test]
    fn resume_false_returns_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("checkpoint.ckpt");

        // Write a valid checkpoint to prove it's ignored.
        let cp = empty_checkpoint("2026-04-09T12:00:00+00:00");
        save_checkpoint(&path, &cp).unwrap();

        let result = try_resume(
            &path,
            "2026-04-09T12:00:00+00:00",
            &cp.secret_scanning_snapshot_signature,
            &std::collections::HashSet::new(),
            false,
        )
        .unwrap();

        assert!(result.completed.is_empty());
        assert!(!result.rotated);
    }

    #[test]
    fn resume_filters_to_current_keys() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("checkpoint.ckpt");

        let sig = build_snapshot_signature(None);
        let mut cp = empty_checkpoint("2026-04-09T12:00:00+00:00");
        cp.secret_scanning_snapshot_signature = sig.clone();
        cp.results.insert(
            "id-repo1".to_string(),
            Arc::new(test_fixtures::all_passing_evidence("repo1")),
        );
        cp.results.insert(
            "id-repo2".to_string(),
            Arc::new(test_fixtures::all_passing_evidence("repo2")),
        );
        save_checkpoint(&path, &cp).unwrap();

        // Only repo1 is in current inventory.
        let mut current = std::collections::HashSet::new();
        current.insert("id-repo1".to_string());

        let result = try_resume(&path, "2026-04-09T12:00:00+00:00", &sig, &current, true).unwrap();

        assert_eq!(result.completed.len(), 1);
        assert!(result.completed.contains_key("id-repo1"));
    }

    #[test]
    fn resume_rejects_different_date() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("checkpoint.ckpt");

        let sig = build_snapshot_signature(None);
        let mut cp = empty_checkpoint("2026-04-08T12:00:00+00:00");
        cp.secret_scanning_snapshot_signature = sig.clone();
        cp.results.insert(
            "id-repo1".to_string(),
            Arc::new(test_fixtures::all_passing_evidence("repo1")),
        );
        save_checkpoint(&path, &cp).unwrap();

        let mut current = std::collections::HashSet::new();
        current.insert("id-repo1".to_string());

        let result = try_resume(
            &path,
            "2026-04-09T12:00:00+00:00", // different date
            &sig,
            &current,
            true,
        )
        .unwrap();

        assert!(result.completed.is_empty());
    }

    #[test]
    fn resume_rejects_different_snapshot_signature() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("checkpoint.ckpt");

        let sig = build_snapshot_signature(None);
        let mut cp = empty_checkpoint("2026-04-09T12:00:00+00:00");
        cp.secret_scanning_snapshot_signature = sig;
        cp.results.insert(
            "id-repo1".to_string(),
            Arc::new(test_fixtures::all_passing_evidence("repo1")),
        );
        save_checkpoint(&path, &cp).unwrap();

        let mut current = std::collections::HashSet::new();
        current.insert("id-repo1".to_string());

        let result = try_resume(
            &path,
            "2026-04-09T12:00:00+00:00",
            "different-signature", // changed
            &current,
            true,
        )
        .unwrap();

        assert!(result.completed.is_empty());
    }

    #[test]
    fn resume_restamps_on_timestamp_change() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("checkpoint.ckpt");

        let sig = build_snapshot_signature(None);
        let mut cp = empty_checkpoint("2026-04-09T10:00:00+00:00");
        cp.secret_scanning_snapshot_signature = sig.clone();
        cp.results.insert(
            "id-repo1".to_string(),
            Arc::new(test_fixtures::all_passing_evidence("repo1")),
        );
        save_checkpoint(&path, &cp).unwrap();

        let mut current = std::collections::HashSet::new();
        current.insert("id-repo1".to_string());

        let new_ts = "2026-04-09T14:00:00+00:00";
        let result = try_resume(&path, new_ts, &sig, &current, true).unwrap();

        assert_eq!(result.completed.len(), 1);
        let ev = result.completed.get("id-repo1").unwrap();
        assert_eq!(ev.checks.security_policy.timestamp, new_ts);
        assert_eq!(ev.checks.codeowners.timestamp, new_ts);
    }

    #[test]
    fn resume_corrupt_file_rotates() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("checkpoint.ckpt");
        std::fs::write(&path, "{not-binary").unwrap();

        let result = try_resume(
            &path,
            "2026-04-09T12:00:00+00:00",
            "sig",
            &std::collections::HashSet::new(),
            true,
        )
        .unwrap();

        assert!(result.completed.is_empty());
        assert!(result.rotated);
        assert!(!path.exists());
        assert!(dir.path().join("checkpoint.ckpt.corrupt").exists());
    }

    #[test]
    fn resume_filters_invalid_checks() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("checkpoint.ckpt");

        let sig = build_snapshot_signature(None);
        let mut cp = empty_checkpoint("2026-04-09T12:00:00+00:00");
        cp.secret_scanning_snapshot_signature = sig.clone();

        // Valid evidence
        cp.results.insert(
            "id-good".to_string(),
            Arc::new(test_fixtures::all_passing_evidence("good")),
        );

        // Invalid evidence (empty timestamp)
        let mut bad = test_fixtures::all_passing_evidence("bad");
        bad.checks.security_policy.timestamp = String::new();
        cp.results.insert("id-bad".to_string(), Arc::new(bad));

        save_checkpoint(&path, &cp).unwrap();

        let mut current = std::collections::HashSet::new();
        current.insert("id-good".to_string());
        current.insert("id-bad".to_string());

        let result = try_resume(&path, "2026-04-09T12:00:00+00:00", &sig, &current, true).unwrap();

        assert_eq!(result.completed.len(), 1);
        assert!(result.completed.contains_key("id-good"));
    }

    // ── checkpoint_path ─────────────────────────────────────────────

    #[test]
    fn checkpoint_path_includes_date() {
        let path = checkpoint_path(Path::new("/tmp"), "2026-04-09");
        assert_eq!(path, PathBuf::from("/tmp/checkpoint-2026-04-09.ckpt"));
    }

    // ── Arc round-trip ──────────────────────────────────────────────

    #[test]
    fn arc_wrapped_evidence_round_trips_through_serde() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("checkpoint.ckpt");

        let mut cp = empty_checkpoint("2026-04-09T12:00:00+00:00");
        let ev = Arc::new(test_fixtures::all_passing_evidence("arc-repo"));
        cp.results
            .insert("id-arc-repo".to_string(), Arc::clone(&ev));

        save_checkpoint(&path, &cp).unwrap();
        let loaded = load_checkpoint(&path).unwrap();

        assert_eq!(loaded.results.len(), 1);
        let loaded_ev = loaded.results.get("id-arc-repo").unwrap();
        // Verify the evidence data is identical after round-trip.
        assert_eq!(loaded_ev.repository.name, "arc-repo");
        assert_eq!(
            loaded_ev.checks.security_policy.timestamp,
            ev.checks.security_policy.timestamp
        );
        // The loaded Arc has refcount 1 (fresh deserialization).
        assert_eq!(Arc::strong_count(loaded_ev), 1);
    }

    // ── remove_checkpoint ───────────────────────────────────────────

    #[test]
    fn remove_checkpoint_deletes_existing_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("checkpoint.ckpt");
        std::fs::write(&path, "{}").unwrap();
        assert!(path.exists());

        remove_checkpoint(&path);
        assert!(!path.exists());
    }

    #[test]
    fn remove_checkpoint_handles_missing_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.json");
        assert!(!path.exists());

        // Should not panic or error.
        remove_checkpoint(&path);
    }

    // ── Binary format tests ─────────────────────────────────────────

    #[test]
    fn save_writes_magic_header() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("checkpoint.bin");

        let cp = empty_checkpoint("2026-04-09T12:00:00+00:00");
        save_checkpoint(&path, &cp).unwrap();

        let raw = std::fs::read(&path).unwrap();
        assert_eq!(
            &raw[..4],
            b"CKPT",
            "checkpoint must start with CKPT magic header"
        );
        assert!(
            raw.len() > 4,
            "checkpoint must contain data after magic header"
        );
    }
}
