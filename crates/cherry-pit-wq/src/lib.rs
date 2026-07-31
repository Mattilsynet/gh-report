//! # cherry-pit-wq
//!
//! Domain-agnostic concurrency and resource-pacing primitives for
//! cherry-pit consumers: bounded deduplicated work queue, worker pool,
//! API-call budget gate, and a rate-limit observer. Runtime-neutral and
//! policy-free per CHE-0055 (G5) — adapter concerns (HTTP header
//! shapes, thresholds, pagination) belong to the calling crate.
//!
//! The flat re-export set below is the SemVer-public API (CHE-0030:R2,
//! CHE-0055:R10). [`DomainKey`], [`JobSource`], [`JobOutcome`] originate
//! in [`cherry_pit_core`], re-exported for single-crate import.
//!
//! `JobSpec<C>` carries `pub correlation: CorrelationContext`; the worker
//! pool propagates that chain end-to-end into the emitted
//! [`JobOutcome::Success`]/[`JobOutcome::Failure`] (CHE-0055:R4/R6). No
//! synthesis at the boundary — the producer chooses the chain.
//!
//! No constructor calls `tokio::runtime::Runtime::new()` or `Builder::*`
//! (CHE-0055:R5); the consumer's binary owns `#[tokio::main]` and signal
//! handling.
//!
//! ## Example
//!
//! Sketch of a worker-pool harness (call-shape that gh-report uses):
//!
//! ```no_run
//! use std::sync::Arc;
//! use cherry_pit_core::CorrelationContext;
//! use cherry_pit_wq::{
//!     JobOutcome, JobSource, JobSpec, Regulator, SystemClock, TokenBucketRegulator,
//!     WorkQueue, WorkerPoolConfig, run_worker_pool_regulated,
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
//! let token_bucket = Arc::new(TokenBucketRegulator::new(Arc::new(SystemClock), 1000, 100));
//! let regulators: Arc<[Arc<dyn Regulator>]> = Arc::from([token_bucket as Arc<dyn Regulator>]);
//! let (tx, _rx) = mpsc::channel::<JobOutcome<String>>(64);
//!
//! run_worker_pool_regulated(
//!     queue,
//!     executor,
//!     regulators,
//!     WorkerPoolConfig::default(),
//!     tokio_util::sync::CancellationToken::new(),
//!     tx,
//! )
//! .await;
//! # }
//! ```
//!
//! Governing ADR: CHE-0055.

#![forbid(unsafe_code)]

mod backoff;
mod budget;
mod rate_limit;
mod regulator;
mod token_bucket;
mod work_queue;
mod worker_pool;

pub use backoff::BackoffRegulator;
pub use budget::BudgetGate;
pub use cherry_pit_core::{DomainKey, JobOutcome, JobSource};
pub use rate_limit::{RateLimitObservation, RateLimitState};
pub use regulator::{Admission, BudgetRegulator, RateLimitRegulator, Regulator, SettleOutcome};
pub use token_bucket::{Clock, SystemClock, TokenBucketRegulator};
pub use work_queue::{
    BatchEnqueueResult, BatchTracker, EnqueueResult, JobSpec, WorkQueue, enqueue_batch,
};
pub use worker_pool::{
    JobExecutor, WorkerPoolConfig, run_worker_pool, run_worker_pool_regulated, shutdown_worker_pool,
};
