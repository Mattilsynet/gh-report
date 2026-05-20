//! `pardosa-eventstore` — unified-log persistent `EventStore` backend.
//!
//! All aggregates share a single append-only log file `<root>/log`,
//! holding one writer file descriptor regardless of aggregate count.
//! Wire format is length-prefixed xxh64-framed
//! `pardosa_encoding::Encode`-encoded envelopes; advisory `RunLock`
//! on `<root>/.lock`.

#![forbid(unsafe_code)]

pub(crate) mod error;
pub(crate) mod frame;
pub(crate) mod store;

pub use error::OpenError;
pub use store::PardosaLogEventStore;
