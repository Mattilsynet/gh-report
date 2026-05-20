//! Error types for [`PardosaLogEventStore`](crate::PardosaLogEventStore).
//!
//! - [`OpenError`] — boot-time failures (lock acquisition, dir scan,
//!   corrupt envelope on recovery). Surfaced to the caller of
//!   [`PardosaLogEventStore::open`](crate::PardosaLogEventStore::open).
//! - Runtime failures from `EventStore::load` / `create` / `append`
//!   surface as [`cherry_pit_core::StoreError`] per the trait contract.

use std::path::PathBuf;

use cherry_pit_storage::PersistenceError;
use thiserror::Error;

/// Failures from [`PardosaLogEventStore::open`](crate::PardosaLogEventStore::open).
///
/// Boot-time recovery surfaces here. Each variant carries enough context
/// for an operator to act: the path under fault, or the underlying I/O
/// error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OpenError {
    /// `tokio::fs::create_dir_all` (or the implicit dir-create inside
    /// `RunLock::acquire`) failed.
    #[error("create root directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// `RunLock::acquire` failed — another live process holds the lock,
    /// or the lock file is unreadable garbage.
    #[error("acquire run lock on {path}: {source}")]
    Lock {
        path: PathBuf,
        #[source]
        source: PersistenceError,
    },

    /// The root directory scan failed (`read_dir` or per-entry I/O).
    #[error("scan root directory {path}: {source}")]
    Scan {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// An entry in the root directory does not match `<digits>.log` and
    /// is not the lock file. Fail loudly per the brief: silently
    /// skipping unknown files would hide configuration mistakes.
    #[error("unexpected file in event-store root: {path}")]
    UnknownFile { path: PathBuf },

    /// A per-aggregate log file failed to open during recovery.
    #[error("open aggregate log {path}: {source}")]
    OpenLog {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A per-aggregate log file failed to read during recovery (frame
    /// scan I/O error — distinct from a torn tail, which is recovered
    /// silently).
    #[error("read aggregate log {path}: {source}")]
    ReadLog {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A recovered envelope failed `pardosa_encoding::from_bytes` — the
    /// frame's xxh64 passed but the body is not a valid encoded
    /// envelope of the expected type. Genuinely corrupt data, not a
    /// torn tail. Caller intervention required.
    #[error("decode envelope in {path} at frame {frame_index}: invalid encoding")]
    DecodeEnvelope { path: PathBuf, frame_index: usize },

    /// Truncating a torn tail back to the last valid frame boundary
    /// failed.
    #[error("truncate aggregate log {path} to recover torn tail: {source}")]
    Truncate {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
