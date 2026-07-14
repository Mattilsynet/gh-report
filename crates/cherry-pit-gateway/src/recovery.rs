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
#[derive(Debug, Clone)]
pub struct StaleLockEvidence {
    /// Absolute or relative path to the `.lock` sentinel file, as
    /// computed from the `store_dir` argument.
    pub lock_path: PathBuf,
    /// Last-modified time reported by the filesystem.
    pub lock_mtime: SystemTime,
    /// Size in bytes (typically zero — the sentinel is content-free).
    pub lock_size: u64,
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
