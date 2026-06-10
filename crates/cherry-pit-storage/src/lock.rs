//! Run lock management to prevent concurrent collection runs.
//!
//! A lock file is written to the working directory before a collection run
//! starts. The lock contains metadata (run ID, PID, creation time) so that
//! stale locks left by crashed processes can be identified and reclaimed.
//!
//! **Stale lock policy:** A lock is considered stale if its `created_at`
//! timestamp exceeds the configured TTL. The default TTL is 4 hours.
//! Manual recovery is available via `--force-unlock`.
//!
//! **Lock atomicity:** Lock creation uses `O_CREAT | O_EXCL` to prevent
//! TOCTOU races — only one process can successfully create the file.

use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use tracing::{info, warn};

use crate::error::PersistenceError;
use crate::fs::atomic_write_text;

/// Default stale-lock TTL: 4 hours.
pub const DEFAULT_LOCK_TTL: Duration = Duration::from_hours(4);

/// Default lock file name.
pub const DEFAULT_LOCK_FILENAME: &str = "collector.lock";

/// Metadata stored inside the lock file.
///
/// Serde DTO; forward/backward schema evolution is handled by serde's
/// field-presence semantics + `#[serde(default)]` rather than
/// `#[non_exhaustive]`. CHE-0021's `#[non_exhaustive]` rule is scoped
/// to public error types in cherry-pit-core, not infrastructure DTOs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockMetadata {
    /// Run ID that holds the lock.
    pub run_id: String,
    /// Process ID of the lock holder (diagnostic only).
    pub pid: u32,
    /// When the lock was created (UTC).
    pub created_at: Timestamp,
}

impl LockMetadata {
    /// Create lock metadata for the current process.
    #[must_use]
    pub fn current(run_id: &str) -> Self {
        Self {
            run_id: run_id.to_string(),
            pid: std::process::id(),
            created_at: Timestamp::now(),
        }
    }
}

/// RAII guard that releases the lock file when dropped.
///
/// The lock is released by deleting the lock file. If deletion fails,
/// a warning is logged but the error is swallowed to avoid masking
/// the original operation's result.
///
/// Fields are private; construction is via [`acquire`] only.
/// `#[non_exhaustive]` is not needed (CHE-0021 scope is error types
/// in cherry-pit-core, and the private fields already prevent
/// external literal construction).
#[derive(Debug)]
pub struct RunLock {
    path: PathBuf,
    metadata: LockMetadata,
}

impl RunLock {
    /// The lock metadata.
    #[must_use]
    pub fn metadata(&self) -> &LockMetadata {
        &self.metadata
    }

    /// The path to the lock file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Explicitly release the lock (delete the lock file).
    ///
    /// This is also called automatically on drop. Returns any I/O error
    /// from the deletion attempt.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError::Io`] if the lock file exists but cannot
    /// be deleted.
    pub fn release(self) -> Result<(), PersistenceError> {
        self.delete_lock_file()
    }

    /// Refresh the lock's `created_at` to now, so a long-running holder
    /// is not mistaken for a stale lock and reclaimed by another
    /// process. Callers (typically long-running daemons) invoke this
    /// well inside the configured stale-lock TTL.
    ///
    /// Rewrites the lock file atomically via [`atomic_write_text`], so
    /// concurrent readers never observe a partial-write state. The
    /// in-memory `metadata` is updated only on a successful write.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError::LockFailed`] if metadata
    /// serialization fails; otherwise propagates any error from the
    /// underlying atomic write as [`PersistenceError::AtomicWriteFailed`]
    /// or [`PersistenceError::Io`].
    pub fn renew(&mut self) -> Result<(), PersistenceError> {
        let refreshed = LockMetadata {
            run_id: self.metadata.run_id.clone(),
            pid: self.metadata.pid,
            created_at: Timestamp::now(),
        };
        let json =
            serde_json::to_string_pretty(&refreshed).map_err(|e| PersistenceError::LockFailed {
                reason: format!("failed to serialize lock metadata: {e}"),
            })?;
        atomic_write_text(&self.path, &json)?;
        self.metadata = refreshed;
        Ok(())
    }

    fn delete_lock_file(&self) -> Result<(), PersistenceError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(PersistenceError::Io(e)),
        }
    }
}

impl Drop for RunLock {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            warn!(
                path = %self.path.display(),
                error = %e,
                "failed to release lock file on drop"
            );
        }
    }
}

/// Maximum number of acquire attempts after stale-lock recovery.
///
/// Handles the race where two processes both see a stale lock, both call
/// `remove_file`, and one wins the `create_lock_exclusive` — the loser
/// retries the full acquire logic.
const MAX_ACQUIRE_ATTEMPTS: u32 = 3;

/// Force-remove an existing lock file, logging the previous holder.
fn force_remove_lock(lock_path: &Path) {
    match read_lock(lock_path) {
        Ok(existing) => {
            warn!(
                run_id = %existing.run_id,
                pid = existing.pid,
                created_at = %existing.created_at,
                "force-removing existing lock"
            );
        }
        Err(_) => {
            warn!(
                path = %lock_path.display(),
                "force-removing corrupt/unreadable lock file"
            );
        }
    }
    if let Err(e) = std::fs::remove_file(lock_path) {
        warn!(
            path = %lock_path.display(),
            error = %e,
            "failed to remove lock file during force-unlock"
        );
    }
}

/// Attempt to acquire a run lock.
///
/// If a lock file already exists, checks whether it is stale using
/// `stale_ttl`. A lock is stale when its `created_at` timestamp plus
/// `stale_ttl` is in the past.
///
/// When `force` is true, any existing lock is removed before acquiring,
/// regardless of stale/alive status. The previous lock's details are
/// logged at `warn` level.
///
/// Lock creation publishes the lock file via `link(2)` (a single atomic
/// step that refuses to clobber an existing path), eliminating any
/// partial-write window between creation and the metadata being durable.
///
/// # Errors
///
/// Returns `PersistenceError::LockFailed` if the lock cannot be acquired
/// because another process holds a non-stale lock. The lock primitive
/// (`create_lock_exclusive`) publishes via `link(2)` so partial-write
/// states are not observable to other processes — no TOCTOU window.
pub fn acquire(
    lock_dir: &Path,
    run_id: &str,
    stale_ttl: Duration,
    force: bool,
    lock_filename: &str,
) -> Result<RunLock, PersistenceError> {
    let lock_path = lock_dir.join(lock_filename);

    std::fs::create_dir_all(lock_dir).map_err(PersistenceError::Io)?;

    if force && lock_path.exists() {
        force_remove_lock(&lock_path);
    }

    let metadata = LockMetadata::current(run_id);

    for _attempt in 0..MAX_ACQUIRE_ATTEMPTS {
        match create_lock_exclusive(&lock_path, &metadata) {
            Ok(()) => {
                info!(
                    run_id = %metadata.run_id,
                    pid = metadata.pid,
                    "lock acquired"
                );
                return Ok(RunLock {
                    path: lock_path,
                    metadata,
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if let Ok(existing) = read_lock(&lock_path) {
                    if is_stale(&existing, stale_ttl) {
                        warn!(
                            run_id = %existing.run_id,
                            pid = existing.pid,
                            created_at = %existing.created_at,
                            "reclaiming stale lock"
                        );
                        if let Err(e) = std::fs::remove_file(&lock_path) {
                            warn!(
                                path = %lock_path.display(),
                                error = %e,
                                "failed to remove stale lock file"
                            );
                        }
                        continue;
                    }
                    return Err(PersistenceError::LockFailed {
                        reason: format!(
                            "lock held by run {} (pid {}, since {})",
                            existing.run_id, existing.pid, existing.created_at,
                        ),
                    });
                }
                match std::fs::metadata(&lock_path) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    _ => {
                        warn!(
                            path = %lock_path.display(),
                            "removing corrupt lock file"
                        );
                        if let Err(e) = std::fs::remove_file(&lock_path) {
                            warn!(
                                path = %lock_path.display(),
                                error = %e,
                                "failed to remove corrupt lock file"
                            );
                        }
                    }
                }
            }
            Err(e) => {
                return Err(PersistenceError::LockFailed {
                    reason: format!("failed to create lock file: {e}"),
                });
            }
        }
    }

    Err(PersistenceError::LockFailed {
        reason: "lock acquisition failed after max retries (concurrent stale-lock race)"
            .to_string(),
    })
}

/// Determine whether a lock is stale based on TTL.
///
/// A lock is stale when its `created_at` timestamp plus `ttl` is in the
/// past. This is a simple time-based check — no PID or host awareness.
fn is_stale(meta: &LockMetadata, ttl: Duration) -> bool {
    let ttl_jiff = SignedDuration::try_from(ttl).unwrap_or(SignedDuration::from_hours(4));
    Timestamp::now().duration_since(meta.created_at) > ttl_jiff
}

/// Create a lock file atomically by writing to a temp file in the same
/// directory and publishing via `link(2)` (`persist_noclobber`).
///
/// Returns `Ok(())` if the file was created and written successfully.
/// Returns `Err` with `ErrorKind::AlreadyExists` if the destination
/// already exists. The whole-file contents are durable and visible
/// atomically — readers never observe an empty or partial state
/// produced by this function.
fn create_lock_exclusive(path: &Path, metadata: &LockMetadata) -> Result<(), std::io::Error> {
    use std::io::Write;
    let json = serde_json::to_string_pretty(metadata)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "lock path has no parent directory",
        )
    })?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(json.as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist_noclobber(path)
        .map_err(|persist_err| persist_err.error)?;
    Ok(())
}

/// Write lock metadata to a file using atomic temp+rename.
///
/// Used by test fixtures to set up lock file state. Production lock
/// creation uses `create_lock_exclusive` for TOCTOU safety.
#[cfg(test)]
fn write_lock(path: &Path, metadata: &LockMetadata) -> Result<(), PersistenceError> {
    let json =
        serde_json::to_string_pretty(metadata).map_err(|e| PersistenceError::LockFailed {
            reason: format!("failed to serialize lock metadata: {e}"),
        })?;
    atomic_write_text(path, &json).map_err(|e| PersistenceError::LockFailed {
        reason: format!("failed to write lock file: {e}"),
    })
}

/// Maximum lock file size in bytes (1 MB).
///
/// Lock files are small (~200 bytes of JSON). A file exceeding this limit
/// is corrupt or adversarially crafted and should not be loaded into memory.
const MAX_LOCK_FILE_BYTES: u64 = 1_048_576;

fn read_lock(path: &Path) -> Result<LockMetadata, PersistenceError> {
    let metadata = std::fs::metadata(path).map_err(PersistenceError::Io)?;
    if metadata.len() > MAX_LOCK_FILE_BYTES {
        return Err(PersistenceError::LockFailed {
            reason: format!(
                "lock file too large: {} bytes (max {MAX_LOCK_FILE_BYTES})",
                metadata.len(),
            ),
        });
    }
    let content = std::fs::read_to_string(path).map_err(PersistenceError::Io)?;
    serde_json::from_str(&content).map_err(|e| PersistenceError::LockFailed {
        reason: format!("lock file is corrupt: {e}"),
    })
}

#[must_use]
pub fn lock_path(lock_dir: &Path, lock_filename: &str) -> PathBuf {
    lock_dir.join(lock_filename)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn lock_metadata_current_populates_fields() {
        let meta = LockMetadata::current("test-run-123");
        assert_eq!(meta.run_id, "test-run-123");
        assert_eq!(meta.pid, std::process::id());
    }

    #[test]
    fn acquire_creates_lock_file() {
        let dir = TempDir::new().unwrap();
        let lock = acquire(
            dir.path(),
            "run-1",
            DEFAULT_LOCK_TTL,
            false,
            DEFAULT_LOCK_FILENAME,
        )
        .unwrap();

        assert!(lock.path().exists());
        assert_eq!(lock.metadata().run_id, "run-1");

        let meta = read_lock(lock.path()).unwrap();
        assert_eq!(meta.run_id, "run-1");
        assert_eq!(meta.pid, std::process::id());
    }

    #[test]
    fn acquire_fails_when_lock_held() {
        let dir = TempDir::new().unwrap();
        let _lock = acquire(
            dir.path(),
            "run-1",
            DEFAULT_LOCK_TTL,
            false,
            DEFAULT_LOCK_FILENAME,
        )
        .unwrap();

        let result = acquire(
            dir.path(),
            "run-2",
            DEFAULT_LOCK_TTL,
            false,
            DEFAULT_LOCK_FILENAME,
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("collection lock failed:"), "got: {msg}");
    }

    #[test]
    fn lock_released_on_drop() {
        let dir = TempDir::new().unwrap();
        let lock_file_path;
        {
            let lock = acquire(
                dir.path(),
                "run-1",
                DEFAULT_LOCK_TTL,
                false,
                DEFAULT_LOCK_FILENAME,
            )
            .unwrap();
            lock_file_path = lock.path().to_path_buf();
            assert!(lock_file_path.exists());
        }
        assert!(!lock_file_path.exists());
    }

    #[test]
    fn lock_released_explicitly() {
        let dir = TempDir::new().unwrap();
        let lock = acquire(
            dir.path(),
            "run-1",
            DEFAULT_LOCK_TTL,
            false,
            DEFAULT_LOCK_FILENAME,
        )
        .unwrap();
        let path = lock.path().to_path_buf();
        assert!(path.exists());

        lock.release().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn stale_lock_is_reclaimed() {
        let dir = TempDir::new().unwrap();

        let stale_meta = LockMetadata {
            run_id: "old-run".to_string(),
            pid: 999_999_999,
            created_at: Timestamp::now() - SignedDuration::from_hours(5),
        };
        let lock_file = dir.path().join(DEFAULT_LOCK_FILENAME);
        write_lock(&lock_file, &stale_meta).unwrap();

        let lock = acquire(
            dir.path(),
            "new-run",
            DEFAULT_LOCK_TTL,
            false,
            DEFAULT_LOCK_FILENAME,
        )
        .unwrap();
        assert_eq!(lock.metadata().run_id, "new-run");
    }

    #[test]
    fn fresh_lock_is_not_reclaimed() {
        let dir = TempDir::new().unwrap();

        let meta = LockMetadata {
            run_id: "active-run".to_string(),
            pid: std::process::id(),
            created_at: Timestamp::now(),
        };
        let lock_file = dir.path().join(DEFAULT_LOCK_FILENAME);
        write_lock(&lock_file, &meta).unwrap();

        let result = acquire(
            dir.path(),
            "new-run",
            DEFAULT_LOCK_TTL,
            false,
            DEFAULT_LOCK_FILENAME,
        );
        assert!(result.is_err());
    }

    #[test]
    fn corrupt_lock_file_is_replaced() {
        let dir = TempDir::new().unwrap();
        let lock_file = dir.path().join(DEFAULT_LOCK_FILENAME);
        std::fs::write(&lock_file, "not-json").unwrap();

        let lock = acquire(
            dir.path(),
            "new-run",
            DEFAULT_LOCK_TTL,
            false,
            DEFAULT_LOCK_FILENAME,
        )
        .unwrap();
        assert_eq!(lock.metadata().run_id, "new-run");
    }

    #[test]
    fn acquire_after_release_succeeds() {
        let dir = TempDir::new().unwrap();
        let lock = acquire(
            dir.path(),
            "run-1",
            DEFAULT_LOCK_TTL,
            false,
            DEFAULT_LOCK_FILENAME,
        )
        .unwrap();
        lock.release().unwrap();

        let lock2 = acquire(
            dir.path(),
            "run-2",
            DEFAULT_LOCK_TTL,
            false,
            DEFAULT_LOCK_FILENAME,
        )
        .unwrap();
        assert_eq!(lock2.metadata().run_id, "run-2");
    }

    #[test]
    fn lock_path_returns_expected() {
        let path = lock_path(Path::new("/tmp/work"), DEFAULT_LOCK_FILENAME);
        assert_eq!(path, PathBuf::from("/tmp/work/collector.lock"));
    }

    #[test]
    fn lock_path_custom_filename() {
        let path = lock_path(Path::new("/tmp/work"), "my-service.lock");
        assert_eq!(path, PathBuf::from("/tmp/work/my-service.lock"));
    }

    #[test]
    fn acquire_with_custom_filename() {
        let dir = TempDir::new().unwrap();
        let lock = acquire(dir.path(), "run-1", DEFAULT_LOCK_TTL, false, "custom.lock").unwrap();
        assert!(lock.path().ends_with("custom.lock"));
    }

    #[test]
    fn is_stale_fresh_lock_is_not_stale() {
        let meta = LockMetadata {
            run_id: "run".to_string(),
            pid: std::process::id(),
            created_at: Timestamp::now(),
        };
        assert!(!is_stale(&meta, DEFAULT_LOCK_TTL));
    }

    #[test]
    fn is_stale_old_lock_is_stale() {
        let meta = LockMetadata {
            run_id: "run".to_string(),
            pid: 999_999_999,
            created_at: Timestamp::now() - SignedDuration::from_hours(5),
        };
        assert!(is_stale(&meta, DEFAULT_LOCK_TTL));
    }

    #[test]
    fn is_stale_lock_at_ttl_boundary_is_not_stale() {
        let meta = LockMetadata {
            run_id: "run".to_string(),
            pid: 1,
            created_at: Timestamp::now() - SignedDuration::from_mins(239),
        };
        assert!(!is_stale(&meta, DEFAULT_LOCK_TTL));
    }

    #[test]
    fn is_stale_custom_short_ttl() {
        let meta = LockMetadata {
            run_id: "run".to_string(),
            pid: 1,
            created_at: Timestamp::now() - SignedDuration::from_secs(61),
        };
        assert!(is_stale(&meta, Duration::from_mins(1)));
    }

    #[test]
    fn concurrent_acquire_exactly_one_wins() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let dir = TempDir::new().unwrap();
        let dir_path = dir.path().to_path_buf();
        let num_threads = 10;
        let barrier = Arc::new(Barrier::new(num_threads));

        let handles: Vec<_> = (0..num_threads)
            .map(|i| {
                let dir = dir_path.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    acquire(
                        &dir,
                        &format!("run-{i}"),
                        DEFAULT_LOCK_TTL,
                        false,
                        DEFAULT_LOCK_FILENAME,
                    )
                })
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let successes: Vec<_> = results.iter().filter(|r| r.is_ok()).collect();
        assert_eq!(
            successes.len(),
            1,
            "exactly one thread should acquire the lock, got {}",
            successes.len()
        );
    }

    #[test]
    fn acquire_with_force_reclaims_fresh_lock() {
        let dir = TempDir::new().unwrap();

        let meta = LockMetadata {
            run_id: "active-run".to_string(),
            pid: std::process::id(),
            created_at: Timestamp::now(),
        };
        let lock_file = dir.path().join(DEFAULT_LOCK_FILENAME);
        write_lock(&lock_file, &meta).unwrap();

        let lock = acquire(
            dir.path(),
            "forced-run",
            DEFAULT_LOCK_TTL,
            true,
            DEFAULT_LOCK_FILENAME,
        )
        .unwrap();
        assert_eq!(lock.metadata().run_id, "forced-run");
    }

    #[test]
    fn acquire_with_force_handles_corrupt_lock() {
        let dir = TempDir::new().unwrap();
        let lock_file = dir.path().join(DEFAULT_LOCK_FILENAME);
        std::fs::write(&lock_file, "not-json-garbage").unwrap();

        let lock = acquire(
            dir.path(),
            "forced-run",
            DEFAULT_LOCK_TTL,
            true,
            DEFAULT_LOCK_FILENAME,
        )
        .unwrap();
        assert_eq!(lock.metadata().run_id, "forced-run");
    }

    #[test]
    fn acquire_with_force_no_existing_lock() {
        let dir = TempDir::new().unwrap();
        let lock = acquire(
            dir.path(),
            "run-1",
            DEFAULT_LOCK_TTL,
            true,
            DEFAULT_LOCK_FILENAME,
        )
        .unwrap();
        assert_eq!(lock.metadata().run_id, "run-1");
    }

    #[tokio::test]
    async fn lock_released_via_arc_mutex_take() {
        use std::sync::Arc;

        let dir = TempDir::new().unwrap();
        let lock = acquire(
            dir.path(),
            "run-1",
            DEFAULT_LOCK_TTL,
            false,
            DEFAULT_LOCK_FILENAME,
        )
        .unwrap();
        let lock_path = lock.path().to_path_buf();
        assert!(lock_path.exists());

        let handle: Arc<tokio::sync::Mutex<Option<RunLock>>> =
            Arc::new(tokio::sync::Mutex::new(Some(lock)));

        {
            let mut guard = handle.lock().await;
            let taken = guard.take().unwrap();
            taken.release().unwrap();
        }

        assert!(
            !lock_path.exists(),
            "lock file should be deleted after take+release"
        );
    }

    #[test]
    fn read_lock_rejects_oversized_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("oversized.lock");
        let f = std::fs::File::create(&path).unwrap();
        f.set_len(MAX_LOCK_FILE_BYTES + 1).unwrap();
        drop(f);

        let err = read_lock(&path).unwrap_err();
        match &err {
            PersistenceError::LockFailed { reason } => {
                assert!(
                    reason.contains("too large"),
                    "expected 'too large' in reason: {reason}"
                );
            }
            other => panic!("expected LockFailed, got: {other:?}"),
        }
    }

    #[test]
    fn read_write_lock_round_trip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.lock");

        let meta = LockMetadata::current("test-run");
        write_lock(&path, &meta).unwrap();
        let loaded = read_lock(&path).unwrap();

        assert_eq!(loaded.run_id, meta.run_id);
        assert_eq!(loaded.pid, meta.pid);
    }

    #[test]
    fn release_on_already_deleted_lock_file_succeeds() {
        let dir = TempDir::new().unwrap();
        let lock = acquire(
            dir.path(),
            "run-1",
            DEFAULT_LOCK_TTL,
            false,
            DEFAULT_LOCK_FILENAME,
        )
        .unwrap();
        let path = lock.path().to_path_buf();

        std::fs::remove_file(&path).unwrap();
        assert!(!path.exists());

        lock.release().unwrap();
    }

    #[test]
    fn old_format_lock_with_hostname_is_readable() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("old.lock");
        let json = r#"{
            "run_id": "old-run",
            "pid": 12345,
            "hostname": "old-host.example.com",
            "created_at": "2026-01-01T00:00:00Z"
        }"#;
        std::fs::write(&path, json).unwrap();

        let meta = read_lock(&path).unwrap();
        assert_eq!(meta.run_id, "old-run");
        assert_eq!(meta.pid, 12345);
    }

    #[test]
    fn externally_created_empty_lock_file_is_replaced() {
        let dir = TempDir::new().unwrap();
        let lock_file = dir.path().join(DEFAULT_LOCK_FILENAME);

        std::fs::write(&lock_file, "").unwrap();

        let result = acquire(
            dir.path(),
            "new-run",
            DEFAULT_LOCK_TTL,
            false,
            DEFAULT_LOCK_FILENAME,
        );
        let lock = result.expect("acquire should succeed by replacing empty lock");
        assert_eq!(lock.metadata.run_id, "new-run");
    }

    #[test]
    fn force_remove_lock_on_missing_file_does_not_panic() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("nonexistent.lock");

        force_remove_lock(&missing);
    }

    #[test]
    fn renew_updates_created_at_and_keeps_run_id_and_pid() {
        let dir = TempDir::new().unwrap();
        let mut lock = acquire(
            dir.path(),
            "long-running",
            DEFAULT_LOCK_TTL,
            false,
            DEFAULT_LOCK_FILENAME,
        )
        .unwrap();
        let original_created_at = lock.metadata.created_at;
        let original_pid = lock.metadata.pid;

        std::thread::sleep(Duration::from_millis(20));
        lock.renew().unwrap();

        assert_eq!(lock.metadata.run_id, "long-running");
        assert_eq!(lock.metadata.pid, original_pid);
        assert!(
            lock.metadata.created_at > original_created_at,
            "renew() must advance created_at"
        );

        let on_disk = read_lock(lock.path()).unwrap();
        assert_eq!(on_disk.run_id, "long-running");
        assert_eq!(on_disk.created_at, lock.metadata.created_at);
    }

    #[test]
    fn renewed_lock_is_not_reclaimed_by_other_acquire() {
        let dir = TempDir::new().unwrap();
        let mut lock = acquire(
            dir.path(),
            "long-running",
            Duration::from_secs(1),
            false,
            DEFAULT_LOCK_FILENAME,
        )
        .unwrap();

        std::thread::sleep(Duration::from_millis(500));
        lock.renew().unwrap();
        std::thread::sleep(Duration::from_millis(700));

        let result = acquire(
            dir.path(),
            "other",
            Duration::from_secs(1),
            false,
            DEFAULT_LOCK_FILENAME,
        );
        assert!(
            result.is_err(),
            "renewed lock should still be live (post-renew elapsed < TTL)"
        );
    }
}
