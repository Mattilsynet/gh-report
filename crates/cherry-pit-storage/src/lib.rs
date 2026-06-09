//! # cherry-pit-storage
//!
//! Synchronous filesystem primitives for cherry-pit consumers. Provides
//! crash-safe atomic file writes, an RAII run-lock with TTL-based stale
//! detection, and canonical-JSON SHA-256 content signatures.
//!
//! Per [CHE-0053], this crate absorbs the `error`, `fs`, `lock`, and
//! `signature` modules from the donor `quics-memoization` crate under a
//! cherry-pit-named, leaf-utility surface. The full enumerated public
//! API surface is flat (CHE-0053:R3) over private modules.
//!
//! ## Crash-safety
//!
//! `atomic_write_bytes` and `atomic_write_text` use the temp-file +
//! fsync + rename + parent-dir-fsync pattern. File contents are
//! fsynced before rename, and the parent directory is fsynced after
//! rename so the rename itself is durable on filesystems whose
//! `fsync(dir)` propagates directory-entry changes (ext4, xfs, zfs;
//! see CHE-0032:R3). Weakening this guarantee — e.g. dropping the
//! parent-dir fsync — is a SemVer-major break (CHE-0053:R6).
//!
//! ## Synchronous-only
//!
//! No `async fn`, no tokio, no futures-util in the public surface
//! (CHE-0053:R4). Consumers needing async I/O wrap calls in
//! `tokio::task::spawn_blocking` themselves (CHE-0053:R7).
//!
//! ## Examples
//!
//! Atomic write + signature for a checkpoint snapshot:
//!
//! ```
//! use cherry_pit_storage::{atomic_write_bytes, build_snapshot_signature};
//! use tempfile::TempDir;
//!
//! let dir = TempDir::new().unwrap();
//! let path = dir.path().join("checkpoint.json");
//! let snapshot = serde_json::json!({"total": 42});
//! let sig = build_snapshot_signature(Some(&snapshot));
//! assert_eq!(sig.len(), 64); // SHA-256 hex
//! atomic_write_bytes(&path, snapshot.to_string().as_bytes()).unwrap();
//! assert!(path.exists());
//! ```
//!
//! Acquire a run lock with the default filename and TTL:
//!
//! ```
//! use cherry_pit_storage::{
//!     acquire, lock_path, DEFAULT_LOCK_FILENAME, DEFAULT_LOCK_TTL,
//! };
//! use tempfile::TempDir;
//!
//! let dir = TempDir::new().unwrap();
//! let lock = acquire(dir.path(), "run-1", DEFAULT_LOCK_TTL, false, DEFAULT_LOCK_FILENAME)
//!     .unwrap();
//! assert_eq!(lock.path(), lock_path(dir.path(), DEFAULT_LOCK_FILENAME));
//! // Drop releases the lock automatically; or call `lock.release()` explicitly.
//! ```
//!
//! Governing ADR: [CHE-0053].
//!
//! [CHE-0053]: https://github.com/acje/solon/blob/main/docs/adr/cherry/CHE-0053-cherry-pit-storage-design.md

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

mod error;
mod fs;
mod lock;
mod signature;

pub use error::PersistenceError;
pub use fs::{atomic_write_bytes, atomic_write_text};
pub use lock::{
    DEFAULT_LOCK_FILENAME, DEFAULT_LOCK_TTL, LockMetadata, RunLock, acquire, lock_path,
};
pub use signature::build_snapshot_signature;
