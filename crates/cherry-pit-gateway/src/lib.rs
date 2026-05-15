//! # cherry-pit-gateway
//!
//! Infrastructure implementations for cherry-pit port traits.
//!
//! This crate provides concrete implementations of the ports defined
//! in `cherry-pit-core`. For development and small deployments, the
//! [`MsgpackFileStore`] persists aggregate event streams as `MessagePack`
//! files on the local filesystem.
//!
//! ## Event store implementations
//!
//! - [`MsgpackFileStore`] — file-based, MessagePack-serialized, default
//!   directory `store/`
//!
//! ## Governing ADRs
//!
//! - [CHE-0006](../docs/adr/cherry/CHE-0006-single-writer-assumption.md) — single-writer assumption
//! - [CHE-0024](../docs/adr/cherry/CHE-0024-event-delivery-model.md) — event delivery model
//! - [CHE-0032](../docs/adr/cherry/CHE-0032-atomic-file-writes.md) — atomic file writes
//! - [CHE-0035](../docs/adr/cherry/CHE-0035-two-level-concurrency.md) — two-level concurrency
//! - [CHE-0036](../docs/adr/cherry/CHE-0036-file-per-stream-full-rewrite-storage.md) — file-per-stream full-rewrite storage
//! - [CHE-0038](../docs/adr/cherry/CHE-0038-testing-strategy.md) — testing strategy
//! - [CHE-0043](../docs/adr/cherry/CHE-0043-process-level-file-fencing.md) — process-level file fencing
//! - [CHE-0047](../docs/adr/cherry/CHE-0047-operational-recovery-runbooks.md) — operational recovery runbooks

#![forbid(unsafe_code)]

mod event_store;
mod recovery;

pub use event_store::MsgpackFileStore;
pub use recovery::{StaleLockEvidence, stale_lock_evidence};
