//! File-level binary format: Writer and Reader for the v2 wire defined in
//! [`crate::format`]. Sync `std::io`-only; no transport deps (GEN-0008 R1).
//! `FileError` stays separate from `DeError` (GEN-0026 R3).

mod reader;
mod writer;

pub use reader::{IndexEntry, MessageIter, Reader};
pub use writer::Writer;
