//! # cherry-pit-gateway
//!
//! Infrastructure implementations for cherry-pit port traits.
//!
//! This crate provides concrete implementations of the ports defined
//! in `cherry-pit-core`. Event stores are consumed via `pardosa`
//! (`.pgno`, default backend); the crate's own file-based `MessagePack`
//! event store was retired per CHE-0100 (msgpack-removal-2).
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

mod recovery;

pub use recovery::{StaleLockEvidence, stale_lock_evidence};
