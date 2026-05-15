//! Run lock management to prevent concurrent collection runs.
//!
//! Re-exports from `cherry-pit-storage`. The wrapper functions pass
//! [`DEFAULT_LOCK_FILENAME`] automatically for backward compatibility.

pub use cherry_pit_storage::{DEFAULT_LOCK_FILENAME, DEFAULT_LOCK_TTL, LockMetadata, RunLock};

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::PersistenceError;

/// Attempt to acquire a run lock using the default lock filename.
///
/// Delegates to [`cherry_pit_storage::acquire`] with
/// [`DEFAULT_LOCK_FILENAME`].
///
/// # Errors
///
/// Returns `PersistenceError::LockFailed` if the lock cannot be acquired.
pub fn acquire(
    lock_dir: &Path,
    run_id: &str,
    stale_ttl: Duration,
    force: bool,
) -> Result<RunLock, PersistenceError> {
    cherry_pit_storage::acquire(lock_dir, run_id, stale_ttl, force, DEFAULT_LOCK_FILENAME)
}

/// Return the lock file path using the default lock filename.
#[must_use]
pub fn lock_path(lock_dir: &Path) -> PathBuf {
    cherry_pit_storage::lock_path(lock_dir, DEFAULT_LOCK_FILENAME)
}
