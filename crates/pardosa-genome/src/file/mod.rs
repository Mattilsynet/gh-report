//! File-level binary format: Writer (and, later, Reader) for the v2 wire
//! defined in [`crate::format`]. Sync `std::io`-only; no transport deps
//! (GEN-0008 R1). `FileError` stays separate from `DeError` (GEN-0026 R3).

mod writer;

pub use writer::Writer;
