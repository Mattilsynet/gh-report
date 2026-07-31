//! Operator-side recovery helpers.
//!
//! Free-standing helpers that operators (or the agent layer) can call when
//! a [`cherry_pit_core::StoreError::StoreLocked`] is observed. These
//! helpers are deliberately additive on the public surface: they do not
//! widen any error type and they only inspect the filesystem.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Filesystem-metadata evidence of a stale lock sentinel, returned by
/// [`stale_lock_evidence`] when `{store_dir}/.lock` exists.
///
/// Record this evidence in the incident record *before* deleting the
/// lock file, so postmortem can correlate the artefact with the
/// `StoreLocked` error that triggered the runbook (CHE-0047:R5).
///
/// **Bound.** `flock(2)` does not portably expose the holder PID, so
/// evidence is restricted to metadata reproducible via `stat` / `ls -la`.
/// Capturing it in-process fixes the value *at the moment of the error*,
/// before clock drift or unrelated mutation perturbs it.
///
/// Cited from CHE-0043:R1 (lock-acquisition mechanism producing this
/// artefact) and CHE-0047:R5 (the runbook this helper supports).
///
/// Fields are private; construction is via [`stale_lock_evidence`] only.
/// Metadata is fixed at the moment the `StoreLocked` error was observed
/// and cannot be mutated or fabricated after the fact (SEC-0002:R2).
#[derive(Debug, Clone)]
#[expect(
    clippy::struct_field_names,
    reason = "lock_* prefix matches the accessor names (lock_path/lock_mtime/lock_size); renaming would desync struct field names from their public accessor methods"
)]
pub struct StaleLockEvidence {
    lock_path: PathBuf,
    lock_mtime: SystemTime,
    lock_size: u64,
}

impl StaleLockEvidence {
    /// Absolute or relative path to the `.lock` sentinel file, as
    /// computed from the `store_dir` argument.
    #[must_use]
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// Last-modified time reported by the filesystem.
    #[must_use]
    pub fn lock_mtime(&self) -> SystemTime {
        self.lock_mtime
    }

    /// Size in bytes (typically zero — the sentinel is content-free).
    #[must_use]
    pub fn lock_size(&self) -> u64 {
        self.lock_size
    }
}

/// Read filesystem metadata for `{store_dir}/.lock`.
///
/// Returns `Ok(Some(_))` when the sentinel file is present, `Ok(None)`
/// when it is absent (including when `store_dir` itself does not exist),
/// and `Err(_)` for any other I/O error (permission denied, etc.).
///
/// Per CHE-0047:R5, callers receiving
/// [`cherry_pit_core::StoreError::StoreLocked`] should invoke
/// this helper to capture incident-record evidence before any operator
/// action that mutates the lock file.
///
/// **Bound.** This helper deliberately does *not* attempt to identify
/// the lock holder: `flock(2)` does not portably expose holder PID.
/// See [`StaleLockEvidence`] for the full rationale.
///
/// Cited from CHE-0043:R1 (lock mechanism) and CHE-0047:R5 (runbook).
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] when `stat` on the lock
/// path fails for any reason other than "not found".
pub fn stale_lock_evidence(store_dir: &Path) -> std::io::Result<Option<StaleLockEvidence>> {
    let lock_path = store_dir.join(".lock");
    match std::fs::metadata(&lock_path) {
        Ok(m) => Ok(Some(StaleLockEvidence {
            lock_mtime: m.modified()?,
            lock_size: m.len(),
            lock_path,
        })),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessors_round_trip_lock_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        let lock_path = dir.path().join(".lock");
        std::fs::write(&lock_path, b"held").expect("write lock file");
        let expected_metadata = std::fs::metadata(&lock_path).expect("stat lock file");

        let evidence = stale_lock_evidence(dir.path())
            .expect("stat store_dir")
            .expect("lock file present");

        assert_eq!(evidence.lock_path(), lock_path);
        assert_eq!(
            evidence.lock_mtime(),
            expected_metadata.modified().expect("modified time")
        );
        assert_eq!(evidence.lock_size(), expected_metadata.len());
    }
}
