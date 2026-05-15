//! Re-export of [`cherry_pit_wq`] work-queue surface.
//!
//! All types and functions are provided by the `cherry-pit-wq` crate.
//! This module preserves the original import paths for downstream code.

pub use cherry_pit_wq::{
    BatchEnqueueResult, BatchTracker, DomainKey, EnqueueResult, JobSource, JobSpec, WorkQueue,
    enqueue_batch,
};
