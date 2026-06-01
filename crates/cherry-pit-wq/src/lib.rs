//! # cherry-pit-wq
//!
//! Domain-agnostic concurrency and resource-pacing primitives for
//! cherry-pit consumers: bounded deduplicated work queue, worker pool,
//! API-call budget gate, and a generic rate-limit observer. Per
//! CHE-0055 G5 the surface is runtime-neutral and policy-free —
//! source-specific concerns (HTTP header shapes, GitHub thresholds,
//! pagination) belong to the calling adapter crate.
//!
//! ## v0.1 surface
//!
//! The flat re-export set below is the SemVer-public API. Shared
//! work-execution types ([`DomainKey`], [`JobSource`], [`JobOutcome`])
//! originate in [`cherry_pit_core`] and are re-exported here for
//! ergonomic single-crate import; their canonical home is core.
//! Internal module structure is implementation detail per
//! CHE-0052:R3 / CHE-0030:R2.
//!
//! ## Correlation propagation (v0.1)
//!
//! `JobSpec<C>` carries `pub correlation: CorrelationContext` and the
//! worker pool propagates that chain end-to-end into the emitted
//! [`JobOutcome::Success`]/[`JobOutcome::Failure`] per CHE-0055 G5
//! (ratified 2026-05-12), which closes the CHE-0052 v0.2 deferral.
//! No synthesis at the worker boundary; the producer chooses the chain
//! (`CorrelationContext::none()` for user-initiated work,
//! `::correlated(uuid)` / `::new(corr, cause)` for policy-driven work).
//!
//! ## Runtime neutrality (CHE-0052:R5)
//!
//! No constructor here calls `tokio::runtime::Runtime::new()` or
//! `Builder::*`. The consumer's binary owns `#[tokio::main]` and
//! signal handling; this crate assumes an active tokio runtime context.
//!
//! ## Example
//!
//! Sketch of a worker-pool harness (call-shape that gh-report uses):
//!
//! ```no_run
//! use std::sync::Arc;
//! use std::time::Duration;
//! use cherry_pit_core::CorrelationContext;
//! use cherry_pit_wq::{
//!     BudgetGate, JobSource, JobSpec, RateLimitState, WorkQueue,
//!     WorkerPoolConfig, JobOutcome, run_worker_pool,
//! };
//! use tokio::sync::mpsc;
//!
//! # async fn demo<E>(executor: Arc<E>) -> ()
//! # where
//! #     E: cherry_pit_wq::JobExecutor<Context = String, Result = String>,
//! # {
//! let queue: Arc<WorkQueue<String>> = Arc::new(WorkQueue::new(100));
//! queue.enqueue(JobSpec::new(
//!     "repo-1".to_string(),
//!     "ctx".to_string(),
//!     JobSource::ScheduledBatch,
//!     CorrelationContext::none(),
//! ));
//!
//! let budget = Arc::new(BudgetGate::new(1000, Duration::from_secs(60)));
//! let rate_limit = Arc::new(RateLimitState::default());
//! let (tx, _rx) = mpsc::channel::<JobOutcome<String>>(64);
//!
//! run_worker_pool(
//!     queue,
//!     executor,
//!     budget,
//!     rate_limit,
//!     WorkerPoolConfig::default(),
//!     tx,
//! )
//! .await;
//! # }
//! ```
//!
//! Governing ADR: CHE-0055 (G5; supersedes CHE-0052 carve-out).

#![forbid(unsafe_code)]

mod budget;
mod rate_limit;
mod work_queue;
mod worker_pool;

pub use budget::BudgetGate;
pub use cherry_pit_core::{DomainKey, JobOutcome, JobSource};
pub use rate_limit::{RateLimitObservation, RateLimitState};
pub use work_queue::{
    BatchEnqueueResult, BatchTracker, EnqueueResult, JobSpec, WorkQueue, enqueue_batch,
};
pub use worker_pool::{JobExecutor, WorkerPoolConfig, run_worker_pool, shutdown_worker_pool};
