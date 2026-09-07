//! Run lock management to prevent concurrent collection runs.
//!
//! A lock file is written to the working directory before a collection run
//! starts. The lock contains metadata (run ID, PID, hostname, creation time)
//! so that stale locks left by crashed processes can be identified and
//! reclaimed.
//!
//! **Stale lock policy:** A lock is considered stale if its holder is dead on
//! the current host, or if its `created_at` timestamp exceeds the configured
//! TTL. The default TTL is 15 minutes.
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

/// Default stale-lock TTL: 15 minutes.
pub const DEFAULT_LOCK_TTL: Duration = Duration::from_mins(15);

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
    /// Hostname of the lock holder.
    #[serde(default)]
    pub hostname: String,
    /// When the lock was created (UTC).
    pub created_at: Timestamp,
}

impl LockMetadata {
    /// Create lock metadata for the current process.
    #[must_use]
    pub fn current(run_id: &str) -> Self {
        Self::current_with_clock(run_id, Timestamp::now)
    }

    fn current_with_clock(run_id: &str, now: impl Fn() -> Timestamp) -> Self {
        Self {
            run_id: run_id.to_string(),
            pid: std::process::id(),
            hostname: current_hostname(),
            created_at: now(),
        }
    }
}

fn current_hostname() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
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
        self.renew_with_clock(Timestamp::now)
    }

    fn renew_with_clock(&mut self, now: impl Fn() -> Timestamp) -> Result<(), PersistenceError> {
        let refreshed = LockMetadata {
            run_id: self.metadata.run_id.clone(),
            pid: self.metadata.pid,
            hostname: self.metadata.hostname.clone(),
            created_at: now(),
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
/// Lock creation is atomic via [`create_lock_exclusive`] (no TOCTOU
/// window), per SEC-0006:R1 and the stale-lock recovery procedure
/// mandated by COM-0025:R6.
///
/// # Errors
///
/// Returns `PersistenceError::LockFailed` if the lock cannot be acquired
/// because another process holds a non-stale lock.
pub fn acquire(
    lock_dir: &Path,
    run_id: &str,
    stale_ttl: Duration,
    force: bool,
    lock_filename: &str,
) -> Result<RunLock, PersistenceError> {
    acquire_with_clock(
        lock_dir,
        run_id,
        stale_ttl,
        force,
        lock_filename,
        Timestamp::now,
    )
}

fn acquire_with_clock(
    lock_dir: &Path,
    run_id: &str,
    stale_ttl: Duration,
    force: bool,
    lock_filename: &str,
    now: impl Fn() -> Timestamp,
) -> Result<RunLock, PersistenceError> {
    let lock_path = lock_dir.join(lock_filename);

    std::fs::create_dir_all(lock_dir).map_err(PersistenceError::Io)?;

    if force && lock_path.exists() {
        force_remove_lock(&lock_path);
    }

    let metadata = LockMetadata::current_with_clock(run_id, &now);

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
                    if let Some(reclaim_reason) = reclaim_reason(&existing, stale_ttl, &now) {
                        let reason = reclaim_reason.as_str();
                        warn!(
                            run_id = %existing.run_id,
                            pid = existing.pid,
                            host = %existing.hostname,
                            created_at = %existing.created_at,
                            reason = reason,
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

#[cfg(test)]
fn is_stale(meta: &LockMetadata, ttl: Duration) -> bool {
    reclaim_reason(meta, ttl, Timestamp::now).is_some()
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum ReclaimReason {
    TtlExpired,
    DeadHolderSameHost,
}

impl ReclaimReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::TtlExpired => "ttl-expired",
            Self::DeadHolderSameHost => "dead-holder-same-host",
        }
    }
}

fn reclaim_reason(
    meta: &LockMetadata,
    ttl: Duration,
    now: impl Fn() -> Timestamp,
) -> Option<ReclaimReason> {
    if same_host_dead_holder(meta) {
        return Some(ReclaimReason::DeadHolderSameHost);
    }
    if ttl_expired(meta, ttl, now) {
        return Some(ReclaimReason::TtlExpired);
    }
    None
}

fn same_host_dead_holder(meta: &LockMetadata) -> bool {
    !meta.hostname.is_empty() && meta.hostname == current_hostname() && !pid_is_alive(meta.pid)
}

fn ttl_expired(meta: &LockMetadata, ttl: Duration, now: impl Fn() -> Timestamp) -> bool {
    let ttl_jiff = SignedDuration::try_from(ttl).unwrap_or_else(|_| {
        SignedDuration::try_from(DEFAULT_LOCK_TTL).unwrap_or(SignedDuration::from_mins(15))
    });
    now().duration_since(meta.created_at) > ttl_jiff
}

#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    let Ok(raw_pid) = i32::try_from(pid) else {
        return true;
    };
    let Some(pid) = rustix::process::Pid::from_raw(raw_pid) else {
        return true;
    };
    !matches!(
        rustix::process::test_kill_process(pid),
        Err(err) if err == rustix::io::Errno::SRCH
    )
}

#[cfg(not(unix))]
fn pid_is_alive(_pid: u32) -> bool {
    true
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
        assert!(!meta.hostname.is_empty());
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
            hostname: LockMetadata::current("host-probe").hostname,
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
            hostname: LockMetadata::current("host-probe").hostname,
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
            hostname: LockMetadata::current("host-probe").hostname,
            created_at: Timestamp::now(),
        };
        assert!(!is_stale(&meta, DEFAULT_LOCK_TTL));
    }

    #[test]
    fn is_stale_old_lock_is_stale() {
        let meta = LockMetadata {
            run_id: "run".to_string(),
            pid: 999_999_999,
            hostname: LockMetadata::current("host-probe").hostname,
            created_at: Timestamp::now() - SignedDuration::from_hours(5),
        };
        assert!(is_stale(&meta, DEFAULT_LOCK_TTL));
    }

    #[test]
    fn cross_host_lock_at_default_ttl_boundary_is_not_stale() {
        let meta = LockMetadata {
            run_id: "run".to_string(),
            pid: 1,
            hostname: "foreign-host.example.com".to_string(),
            created_at: Timestamp::now() - SignedDuration::from_mins(14),
        };
        assert!(!is_stale(&meta, DEFAULT_LOCK_TTL));
    }

    #[test]
    fn default_ttl_is_fifteen_minutes() {
        assert_eq!(DEFAULT_LOCK_TTL, Duration::from_mins(15));
    }

    #[test]
    fn is_stale_custom_short_ttl() {
        let meta = LockMetadata {
            run_id: "run".to_string(),
            pid: 1,
            hostname: LockMetadata::current("host-probe").hostname,
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
            hostname: LockMetadata::current("host-probe").hostname,
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
        assert_eq!(loaded.hostname, meta.hostname);
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
        assert_eq!(meta.hostname, "old-host.example.com");
    }

    #[test]
    fn old_format_lock_without_hostname_is_ttl_only() {
        let dir = TempDir::new().unwrap();
        let lock_file = dir.path().join(DEFAULT_LOCK_FILENAME);
        let json = format!(
            r#"{{
            "run_id": "old-run",
            "pid": 999999999,
            "created_at": "{}"
        }}"#,
            Timestamp::now()
        );
        std::fs::write(&lock_file, json).unwrap();

        let meta = read_lock(&lock_file).unwrap();
        assert!(meta.hostname.is_empty());

        let result = acquire(
            dir.path(),
            "new-run",
            DEFAULT_LOCK_TTL,
            false,
            DEFAULT_LOCK_FILENAME,
        );
        assert!(
            result.is_err(),
            "missing hostname must prevent same-host dead-pid auto-steal"
        );
    }

    #[test]
    fn same_host_dead_pid_lock_is_reclaimed_immediately() {
        let dir = TempDir::new().unwrap();
        let meta = LockMetadata {
            run_id: "dead-run".to_string(),
            pid: 999_999_999,
            hostname: LockMetadata::current("host-probe").hostname,
            created_at: Timestamp::now(),
        };
        let lock_file = dir.path().join(DEFAULT_LOCK_FILENAME);
        write_lock(&lock_file, &meta).unwrap();

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
    fn same_host_alive_pid_lock_is_not_reclaimed() {
        let dir = TempDir::new().unwrap();
        let meta = LockMetadata {
            run_id: "alive-run".to_string(),
            pid: std::process::id(),
            hostname: LockMetadata::current("host-probe").hostname,
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
    fn different_host_dead_pid_lock_reclaims_only_after_ttl() {
        let dir = TempDir::new().unwrap();
        let lock_file = dir.path().join(DEFAULT_LOCK_FILENAME);
        let fresh_meta = LockMetadata {
            run_id: "foreign-run".to_string(),
            pid: 999_999_999,
            hostname: "foreign-host.example.com".to_string(),
            created_at: Timestamp::now(),
        };
        write_lock(&lock_file, &fresh_meta).unwrap();

        let fresh_result = acquire(
            dir.path(),
            "new-run",
            DEFAULT_LOCK_TTL,
            false,
            DEFAULT_LOCK_FILENAME,
        );
        assert!(fresh_result.is_err());

        let aged_meta = LockMetadata {
            created_at: Timestamp::now() - SignedDuration::from_mins(16),
            ..fresh_meta
        };
        write_lock(&lock_file, &aged_meta).unwrap();

        let lock = acquire(
            dir.path(),
            "ttl-run",
            DEFAULT_LOCK_TTL,
            false,
            DEFAULT_LOCK_FILENAME,
        )
        .unwrap();

        assert_eq!(lock.metadata().run_id, "ttl-run");
    }

    #[test]
    fn auto_steal_emits_warn_with_holder_evidence() {
        let dir = TempDir::new().unwrap();
        let meta = LockMetadata {
            run_id: "dead-run".to_string(),
            pid: 999_999_999,
            hostname: LockMetadata::current("host-probe").hostname,
            created_at: Timestamp::now(),
        };
        let lock_file = dir.path().join(DEFAULT_LOCK_FILENAME);
        write_lock(&lock_file, &meta).unwrap();

        let events = capture_lock_events(|| {
            acquire(
                dir.path(),
                "new-run",
                DEFAULT_LOCK_TTL,
                false,
                DEFAULT_LOCK_FILENAME,
            )
            .unwrap();
        });

        assert!(events.contains("level=WARN"), "got: {events}");
        assert!(events.contains("run_id=dead-run"), "got: {events}");
        assert!(events.contains("pid=999999999"), "got: {events}");
        assert!(events.contains("host="), "got: {events}");
        assert!(
            events.contains("reason=\"dead-holder-same-host\""),
            "got: {events}"
        );
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
        let original_hostname = lock.metadata.hostname.clone();

        std::thread::sleep(Duration::from_millis(20));
        lock.renew().unwrap();

        assert_eq!(lock.metadata.run_id, "long-running");
        assert_eq!(lock.metadata.pid, original_pid);
        assert_eq!(lock.metadata.hostname, original_hostname);
        assert!(
            lock.metadata.created_at > original_created_at,
            "renew() must advance created_at"
        );

        let on_disk = read_lock(lock.path()).unwrap();
        assert_eq!(on_disk.run_id, "long-running");
        assert_eq!(on_disk.created_at, lock.metadata.created_at);
        assert_eq!(on_disk.hostname, lock.metadata.hostname);
    }

    #[test]
    fn renewed_lock_is_not_reclaimed_by_other_acquire() {
        let dir = TempDir::new().unwrap();
        let start: Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
        let mut lock = acquire_with_clock(
            dir.path(),
            "long-running",
            Duration::from_secs(1),
            false,
            DEFAULT_LOCK_FILENAME,
            || start,
        )
        .unwrap();
        assert_eq!(
            lock.metadata.created_at, start,
            "acquire must use injected time"
        );
        let original = lock.metadata.clone();
        let renewed_at = start + SignedDuration::from_millis(500);
        lock.renew_with_clock(|| renewed_at).unwrap();
        let expected = LockMetadata {
            created_at: renewed_at,
            ..original
        };
        assert_eq!(lock.metadata, expected);
        assert_eq!(read_lock(lock.path()).unwrap(), expected);

        for elapsed_ms in [1200, 1500] {
            let result = acquire_with_clock(
                dir.path(),
                "other",
                Duration::from_secs(1),
                false,
                DEFAULT_LOCK_FILENAME,
                || start + SignedDuration::from_millis(elapsed_ms),
            );
            assert!(
                matches!(result, Err(PersistenceError::LockFailed { .. })),
                "renewed lock must remain held before and exactly at TTL"
            );
            assert_eq!(read_lock(lock.path()).unwrap(), expected);
        }

        let after_ttl = renewed_at + SignedDuration::from_secs(1) + SignedDuration::from_nanos(1);
        let replacement = acquire_with_clock(
            dir.path(),
            "other",
            Duration::from_secs(1),
            false,
            DEFAULT_LOCK_FILENAME,
            || after_ttl,
        )
        .expect("renewed lock must expire strictly after TTL");
        assert_eq!(replacement.metadata.run_id, "other");
        assert_eq!(replacement.metadata.created_at, after_ttl);
        assert_eq!(read_lock(replacement.path()).unwrap(), replacement.metadata);
    }

    #[test]
    fn capture_survives_wrong_thread_first_registration() {
        for order in [
            RegistrationOrder::CompetitorFirst,
            RegistrationOrder::CaptureFirst,
        ] {
            let mut command = capture_child_command("lock::tests::capture_registration_child");
            command.env(REGISTRATION_ORDER, order.as_str());
            let mut child = command.spawn().unwrap();
            let outcome = wait_for_capture_child(&mut child, CAPTURE_CHILD_TIMEOUT).unwrap();
            assert!(
                matches!(outcome, CaptureChildOutcome::Exited(status) if status.success()),
                "{order:?}: {outcome:?}; retained child output max=0 bytes"
            );
            eprintln!("verified-order={}", order.as_str());
        }
    }

    const REGISTRATION_ORDER: &str = "CHERRY_PIT_CAPTURE_REGISTRATION_ORDER";
    const CAPTURE_CHILD_TIMEOUT: Duration = Duration::from_secs(10);

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum RegistrationOrder {
        CompetitorFirst,
        CaptureFirst,
    }

    impl RegistrationOrder {
        fn parse(value: &str) -> Result<Self, &'static str> {
            match value {
                "competitor-first" => Ok(Self::CompetitorFirst),
                "capture-first" => Ok(Self::CaptureFirst),
                _ => Err("unknown capture registration order"),
            }
        }

        fn as_str(self) -> &'static str {
            match self {
                Self::CompetitorFirst => "competitor-first",
                Self::CaptureFirst => "capture-first",
            }
        }
    }

    fn capture_child_command(target: &str) -> std::process::Command {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", target, "--ignored", "--nocapture"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        command
    }

    #[derive(Debug)]
    enum CaptureChildOutcome {
        Exited(std::process::ExitStatus),
        TimedOut(std::process::ExitStatus),
    }

    fn wait_for_capture_child(
        child: &mut std::process::Child,
        timeout: Duration,
    ) -> std::io::Result<CaptureChildOutcome> {
        let started = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Ok(CaptureChildOutcome::Exited(status)),
                Ok(None) if started.elapsed() >= timeout => {
                    return kill_and_reap_capture_child(child).map(CaptureChildOutcome::TimedOut);
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(error) => {
                    kill_and_reap_capture_child(child)?;
                    return Err(error);
                }
            }
        }
    }

    fn kill_and_reap_capture_child(
        child: &mut std::process::Child,
    ) -> std::io::Result<std::process::ExitStatus> {
        let killed = match child.kill() {
            Ok(()) => Ok(()),
            Err(error) => match child.try_wait() {
                Ok(Some(_)) => Ok(()),
                Ok(None) => Err(error),
                Err(wait_error) => Err(wait_error),
            },
        };
        let reaped = child.wait();
        killed?;
        reaped
    }

    #[test]
    fn capture_child_timeout_kills_and_reaps() {
        let mut child = capture_child_command("lock::tests::capture_timeout_child")
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        let outcome = wait_for_capture_child(&mut child, Duration::from_millis(50)).unwrap();
        assert!(
            matches!(outcome, CaptureChildOutcome::TimedOut(status) if !status.success()),
            "must kill and reap timed-out child: {outcome:?}"
        );
        assert!(child.try_wait().unwrap().is_some());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn capture_timeout_child_exits_without_parent_kill() {
        let mut child = capture_child_command("lock::tests::capture_timeout_child")
            .spawn()
            .unwrap();
        let outcome = wait_for_capture_child(&mut child, Duration::from_secs(3)).unwrap();
        assert!(
            matches!(outcome, CaptureChildOutcome::Exited(status) if status.success()),
            "fixture must self-terminate: {outcome:?}"
        );
    }

    #[test]
    #[ignore = "dedicated subprocess deadline fixture"]
    fn capture_timeout_child() {
        std::thread::sleep(Duration::from_secs(2));
    }

    #[test]
    fn capture_registration_order_rejects_unknown() {
        assert_eq!(
            RegistrationOrder::parse("unknown"),
            Err("unknown capture registration order")
        );
    }

    #[test]
    #[ignore = "parent executes both registration orders in fresh processes"]
    fn capture_registration_child() {
        let order = RegistrationOrder::parse(&std::env::var(REGISTRATION_ORDER).unwrap()).unwrap();

        let events = capture_lock_events(|| {
            if order == RegistrationOrder::CaptureFirst {
                reclaim_dead_holder("captured-first");
            }
            std::thread::spawn(|| reclaim_dead_holder("uncaptured-competitor"))
                .join()
                .unwrap();
            let max_level = tracing::metadata::LevelFilter::current();
            eprintln!("order={order:?} max_level={max_level}");
            assert!(max_level >= tracing::metadata::LevelFilter::WARN);
            reclaim_dead_holder("captured-after");
        });
        let expected_host = format!("host={} ", LockMetadata::current("host-probe").hostname);
        assert!(
            events.lines().any(|event| [
                "run_id=captured-after",
                "level=WARN",
                "pid=999999999",
                &expected_host,
                "reason=\"dead-holder-same-host\""
            ]
            .iter()
            .all(|field| event.contains(field))),
            "got: {events}"
        );
        assert!(!events.contains("uncaptured-competitor"), "got: {events}");
        if order == RegistrationOrder::CaptureFirst {
            assert!(events.contains("run_id=captured-first"), "got: {events}");
        }
    }

    fn reclaim_dead_holder(run_id: &str) {
        let dir = TempDir::new().unwrap();
        let meta = LockMetadata {
            run_id: run_id.to_owned(),
            pid: 999_999_999,
            hostname: LockMetadata::current("host-probe").hostname,
            created_at: Timestamp::now(),
        };
        write_lock(&dir.path().join(DEFAULT_LOCK_FILENAME), &meta).unwrap();
        let lock = acquire(
            dir.path(),
            "replacement",
            DEFAULT_LOCK_TTL,
            false,
            DEFAULT_LOCK_FILENAME,
        )
        .unwrap();
        assert_eq!(lock.metadata.run_id, "replacement");
    }

    fn capture_lock_events(f: impl FnOnce()) -> String {
        use std::fmt::Write;
        use std::sync::{Arc, Mutex};
        use tracing::Event;
        use tracing::Level;
        use tracing::Metadata;
        use tracing::Subscriber;
        use tracing::field::{Field, Visit};
        use tracing::span;
        use tracing::subscriber::Interest;

        #[derive(Clone, Default)]
        struct CaptureSubscriber {
            events: Arc<Mutex<String>>,
        }

        impl Subscriber for CaptureSubscriber {
            fn enabled(&self, metadata: &Metadata<'_>) -> bool {
                metadata.level() <= &Level::WARN
            }

            fn new_span(&self, _span: &span::Attributes<'_>) -> span::Id {
                span::Id::from_u64(1)
            }

            fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}

            fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}

            fn event(&self, event: &Event<'_>) {
                let mut visitor = EventVisitor::default();
                event.record(&mut visitor);
                let mut events = self.events.lock().unwrap();
                writeln!(
                    events,
                    "level={} {}",
                    event.metadata().level(),
                    visitor.fields
                )
                .unwrap();
            }

            fn enter(&self, _span: &span::Id) {}

            fn exit(&self, _span: &span::Id) {}

            fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
                if self.enabled(metadata) {
                    Interest::always()
                } else {
                    Interest::never()
                }
            }

            fn max_level_hint(&self) -> Option<tracing::metadata::LevelFilter> {
                Some(tracing::metadata::LevelFilter::WARN)
            }
        }

        #[derive(Default)]
        struct EventVisitor {
            fields: String,
        }

        impl Visit for EventVisitor {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                write!(self.fields, "{}={value:?} ", field.name()).unwrap();
            }
        }

        let subscriber = CaptureSubscriber::default();
        let events = Arc::clone(&subscriber.events);
        let registration_dispatch = tracing::Dispatch::new(subscriber.clone());
        tracing::subscriber::with_default(subscriber, f);
        drop(registration_dispatch);
        events.lock().unwrap().clone()
    }
}
