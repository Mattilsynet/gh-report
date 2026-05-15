//! Infrastructure: baseline persistence, checkpointing, locking, filesystem,
//! signal handling, logging, and input validation.

pub mod baseline;
pub mod checkpoint;
pub mod cloud_logging;
pub mod lock;
pub mod logging;
pub mod server;
pub mod signal;
pub mod validate;
