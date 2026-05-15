//! Re-export of [`cherry_pit_wq`] worker-pool surface.
//!
//! All types and functions are provided by the `cherry-pit-wq` crate.
//! This module preserves the original import paths for downstream code.

pub use cherry_pit_wq::{
    JobExecutor, JobOutcome, WorkerPoolConfig, run_worker_pool, shutdown_worker_pool,
};
