//! IO-efficient `.pgno` append-writer side index manifest.
//!
//! [`AppendWriter`](crate::AppendWriter) accumulates per-message index
//! state in memory while bodies stream to the `.pgno` sink. The
//! manifest is the durable companion: a small, separately-fsynced
//! file of `(offset, size, checksum)` triples. On a crash without
//! [`AppendWriter::finish`](crate::AppendWriter::finish) the
//! footerless `.pgno` is not [`Reader::open`](crate::Reader::open)-
//! compatible; the manifest enables recovery without re-reading the
//! `.pgno` body to guess boundaries.
//!
//! # Layout
//!
//! ```text
//! [ManifestHeader: 32 bytes]
//!   magic [4]        = "PGIX"
//!   version [2]      = 2 (little-endian u16)
//!   reserved [2]     = 0
//!   schema_hash [16] = .pgno schema_hash (little-endian u128)
//!   page_class [1]   = .pgno page_class
//!   reserved [3]     = 0
//!   schema_size [4]  = .pgno schema_size (little-endian u32)
//! [Records: N × 24 bytes — ManifestRecord wire form]
//!   offset [8]   little-endian u64
//!   size [4]     little-endian u32
//!   reserved [4] = 0
//!   checksum [8] little-endian u64
//! [ManifestFooter: 60 bytes]
//!   message_count [8] little-endian u64
//!   data_end [8]      little-endian u64
//!   frontier [32]     rolling unkeyed BLAKE3 frontier
//!   checksum [8]      xxh64 over (header || records || frontier), little-endian
//!   magic [4]         = "PGIX"
//! ```
//!
//! Wire format frozen at version 2; pinned by
//! `tests/append_manifest.rs`. Bumps follow ADR-0006 semver discipline.
/// Manifest header / footer magic. Distinct from the `.pgno`
/// `PGNO` magic so a stray cross-file open surfaces immediately.
pub const MANIFEST_MAGIC: &[u8; 4] = b"PGIX";
/// Wire-format version of the manifest. Bumped only when the
/// record / footer layout changes.
pub const MANIFEST_VERSION: u16 = 2;
/// Fixed manifest header size.
pub const MANIFEST_HEADER_SIZE: usize = 32;
/// On-disk size of one [`ManifestRecord`].
pub const MANIFEST_RECORD_SIZE: usize = 24;
/// Fixed manifest footer size.
pub const MANIFEST_FOOTER_SIZE: usize = 60;
pub(super) const MANIFEST_HEADER_MAGIC_OFFSET: usize = 0;
pub(super) const MANIFEST_HEADER_VERSION_OFFSET: usize = 4;
pub(super) const MANIFEST_HEADER_RESERVED0_OFFSET: usize = 6;
pub(super) const MANIFEST_HEADER_SCHEMA_HASH_OFFSET: usize = 8;
pub(super) const MANIFEST_HEADER_PAGE_CLASS_OFFSET: usize = 24;
pub(super) const MANIFEST_HEADER_RESERVED1_OFFSET: usize = 25;
pub(super) const MANIFEST_HEADER_SCHEMA_SIZE_OFFSET: usize = 28;
const _: () = assert!(
    MANIFEST_HEADER_SCHEMA_SIZE_OFFSET + 4 == MANIFEST_HEADER_SIZE,
    "MANIFEST_HEADER_SIZE drifted from the header offset table",
);
pub(super) const MANIFEST_FOOTER_MESSAGE_COUNT_OFFSET: usize = 0;
pub(super) const MANIFEST_FOOTER_DATA_END_OFFSET: usize = 8;
pub(super) const MANIFEST_FOOTER_FRONTIER_OFFSET: usize = 16;
pub(super) const MANIFEST_FOOTER_CHECKSUM_OFFSET: usize = 48;
pub(super) const MANIFEST_FOOTER_MAGIC_OFFSET: usize = 56;
const _: () = assert!(
    MANIFEST_FOOTER_MAGIC_OFFSET + 4 == MANIFEST_FOOTER_SIZE,
    "MANIFEST_FOOTER_SIZE drifted from the footer offset table",
);
pub(crate) const GENESIS_FRONTIER: [u8; 32] = [0u8; 32];
pub(crate) fn roll_frontier(previous: [u8; 32], body: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&previous);
    hasher.update(body);
    hasher.finalize().into()
}
pub(crate) mod error;
pub(crate) mod record;
pub(crate) mod recovery;
pub(crate) mod snapshot;
pub(crate) mod writer;
pub use error::RecoveryError;
pub use record::ManifestRecord;
pub use recovery::{
    RecoveredPrefix, RecoveryOutcome, RecoveryReaderErrorKind, finalize_recovered_prefix,
    recover_footerless_prefix,
};
pub use snapshot::{ManifestSnapshot, parse_manifest, write_complete_manifest};
pub(crate) use writer::IndexManifestWriter;
#[cfg(test)]
const _: fn() = || {};
