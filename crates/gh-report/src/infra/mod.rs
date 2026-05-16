//! Infrastructure: baseline persistence, checkpointing, locking, filesystem,
//! signal handling, logging, and signal/server primitives.
//!
//! Input validation (`sanitize_path_segment`) lives in `cherry_pit_web`
//! since SM1 `sm1-sanitize-path-1779000001`; the prior `infra::validate`
//! module was deleted to collapse the cross-crate duplicate.

pub mod baseline;
pub mod checkpoint;
pub mod cloud_logging;
pub mod lock;
pub mod logging;
pub mod server;
pub mod signal;
