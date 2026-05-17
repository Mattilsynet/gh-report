//! File-level binary format: Writer and Reader for the wire layout defined
//! in [`crate::format`] (current [`crate::format::FORMAT_VERSION`]).
//! Sync `std::io`-only; no transport deps (GEN-0008 R1).
//! `FileError` stays separate from `DeError` (GEN-0026 R3).

mod reader;
mod writer;

pub use reader::{IndexEntry, MessageIter, Reader};
pub use writer::Writer;
