//! `pardosa-eventstore` — persistent file-per-aggregate `EventStore`
//! backend.
//!
//! See [`PardosaLogEventStore`] for the entrypoint. Wire format is
//! length-prefixed xxh64-framed `pardosa_encoding::Encode` envelopes,
//! one file per aggregate, advisory `RunLock` on `<root>/.lock`.

#![forbid(unsafe_code)]

pub(crate) mod error;
pub(crate) mod frame;
pub(crate) mod store;

pub use error::OpenError;
pub use store::PardosaLogEventStore;
