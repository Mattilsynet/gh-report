//! Error types for [`PardosaLogEventStore`](crate::PardosaLogEventStore).
//!
//! [`OpenError`] carries boot-time failures (lock acquisition, log
//! scan, corrupt envelope on recovery). Runtime failures from
//! `EventStore::load` / `create` / `append` surface as
//! [`cherry_pit_core::StoreError`] per the trait contract.

use std::path::PathBuf;

use cherry_pit_storage::PersistenceError;
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OpenError {
    #[error("create root directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("acquire run lock on {path}: {source}")]
    Lock {
        path: PathBuf,
        #[source]
        source: PersistenceError,
    },

    #[error("open unified log {path}: {source}")]
    OpenLog {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("read unified log {path}: {source}")]
    ReadLog {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("decode envelope in {path} at frame {frame_index}: invalid encoding")]
    DecodeEnvelope { path: PathBuf, frame_index: usize },

    #[error("truncate unified log {path} to recover torn tail: {source}")]
    Truncate {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
