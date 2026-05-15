//! Operator-side recovery helpers.
//!
//! Free-standing helpers that operators (or the agent layer) can call when
//! a [`cherry_pit_core::StoreError::StoreLocked`] is observed. These
//! helpers are deliberately additive on the public surface: they do not
//! widen any error type and they only inspect the filesystem.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Filesystem-metadata evidence of a stale lock sentinel.
///
/// Returned by [`stale_lock_evidence`] when `{store_dir}/.lock` exists.
/// Per CHE-0047:R5 (operational recovery runbooks), operators should
/// record this evidence in the incident record *before* deleting the
/// lock file, so the eventual postmortem can correlate the lock
/// artefact with the `StoreLocked` error that triggered the runbook.
///
/// **Bound.** `flock(2)` does not portably expose the holder process's
/// PID, so evidence is restricted to filesystem metadata that any
/// operator could also reproduce via `stat` / `ls -la`. The value of
/// capturing it in-process is to record it *at the moment of the
/// `StoreLocked` error*, before clock drift or unrelated filesystem
/// mutation can perturb the artefact.
///
/// Cited from:
/// - CHE-0043:R1 — the lock-acquisition mechanism that produces this
///   artefact (`{store_dir}/.lock` advisory `flock`).
/// - CHE-0047:R5 — the operational runbook this helper supports.
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
