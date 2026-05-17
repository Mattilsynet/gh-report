//! Genome-file Writer (emits current [`crate::format::FORMAT_VERSION`]).
//!
//! Emits a complete file in three phases from a single
//! [`Writer::finish`] call (no in-place mutation, no append after finish per
//! GEN-0009 R3):
//!
//!   1. Header (40 bytes) — written eagerly at construction's first
//!      `write_message` or at `finish` for 0-message files; magic + version +
//!      schema_hash + page_class + schema_size + reserved zeros per
//!      `crates/pardosa-genome/src/format.rs:18`.
//!   2. Message bodies — each `write_message` appends the body verbatim and
//!      buffers an in-memory index entry (offset, size, xxh64 seed=0 per
//!      GEN-0016 R1).
//!   3. Index block — `finish` flushes the buffered entries.
//!   4. Footer (32 bytes) — `finish` writes `index_offset`, `message_count`,
//!      reserved zeros, "PGNO" magic, and a footer-spanning xxh64 checksum
//!      over footer[0..24].
//!
//! Design notes:
//!  * `W: Write` only — no `Seek` requirement (per package G3 resolution: the
//!    index is buffered in memory so the footer can be authored last from
//!    known state). Aligns GEN-0008 R1 — no transport deps.
//!  * `schema_source` is optional (`Option<&str>`, GEN-0009 R2). When `Some`
//!    and non-empty, the UTF-8 bytes are emitted between the header and the
//!    first message body, followed by zero-pad to an 8-byte boundary; header
//!    `schema_size` records the unpadded byte count. `Some("")` writes no
//!    block (schema_size = 0) — observably indistinguishable from `None`.
//!  * `dict_id` is hard-zero (`format.rs:28`).
//!  * `page_class` is opaque to v0.1 (G5): caller sets it via
//!    [`Writer::with_page_class`]; Writer writes it verbatim.

use std::io::{self, Write};

use crate::error::FileError;
use crate::format::{
    FILE_FOOTER_SIZE, FILE_HEADER_SIZE, FOOTER_CHECKSUM_OFFSET, FOOTER_INDEX_OFFSET,
    FOOTER_MAGIC_OFFSET, FOOTER_MESSAGE_COUNT_OFFSET, FORMAT_VERSION, HEADER_MAGIC_OFFSET,
    HEADER_PAGE_CLASS_OFFSET, HEADER_SCHEMA_HASH_LEN, HEADER_SCHEMA_HASH_OFFSET,
    HEADER_SCHEMA_SIZE_OFFSET, HEADER_VERSION_OFFSET, INDEX_ENTRY_SIZE, MAGIC, messages_offset,
    pad_to_8,
};
use xxhash_rust::xxh64::xxh64;

/// In-memory record of one message's index entry. Materialised to wire bytes
/// at `finish` time so the Writer doesn't need a `Seek` sink.
#[derive(Debug, Clone, Copy)]
struct IndexEntry {
    offset: u64,
    size: u32,
    checksum: u64,
}

/// Genome-file writer (emits current [`crate::format::FORMAT_VERSION`]).
///
/// Use [`Writer::new`] to start, [`Writer::write_message`] zero or more times,
/// then [`Writer::finish`] exactly once. After `finish`, the Writer is
/// consumed — append-after-finish is not a supported mode (GEN-0009 R3).
pub struct Writer<'w, W: Write> {
    sink: &'w mut W,
    schema_hash: u128,
    page_class: u8,
    /// Optional UTF-8 schema source (GEN-0009 R2). When `Some` and non-empty,
    /// written between the header and the first message body, padded to an
    /// 8-byte boundary with zeros. Borrowed from the caller — the bytes are
    /// flushed during the first `write_message`/`finish` call.
    schema_source: Option<&'w str>,
    /// Tracks bytes written for index-entry offsets. Starts at
    /// `messages_offset(schema_size)` once the header + schema block are flushed.
    cursor: u64,
    /// `true` once the header has been written to `sink`.
    header_written: bool,
    /// Buffered index entries (G3: in-memory until `finish`).
    index: Vec<IndexEntry>,
}

impl<'w, W: Write> Writer<'w, W> {
    /// Construct a Writer with a 16-byte schema hash (xxh3-128 per GEN-0035).
    ///
    /// `page_class` defaults to `0`; override via [`Writer::with_page_class`].
    /// `schema_source` defaults to `None`; override via
    /// [`Writer::with_schema_source`] (GEN-0009 R2).
    pub fn new(sink: &'w mut W, schema_hash: u128) -> Self {
        Self {
            sink,
            schema_hash,
            page_class: 0,
            schema_source: None,
            cursor: 0,
            header_written: false,
            index: Vec::new(),
        }
    }

    /// Builder-style override of `page_class`. Semantics opaque to v0.1 (G5);
    /// the byte is written verbatim into the header.
    #[must_use]
    pub fn with_page_class(mut self, page_class: u8) -> Self {
        self.page_class = page_class;
        self
    }

    /// Builder-style attachment of an optional informational schema source
    /// (GEN-0009 R2). The byte length is recorded in the header's
    /// `schema_size` u32 LE; the bytes themselves are written verbatim after
    /// the header, then zero-padded to an 8-byte boundary so the first
    /// message body lands on an aligned offset (`messages_offset(size)`).
    ///
    /// Passing `""` is permitted — it sets `schema_size = 0` and writes no
    /// block (observably the same as never calling this rung). The Reader
    /// surface returns `None` in that case (no block to expose).
    #[must_use]
    pub fn with_schema_source(mut self, schema_source: &'w str) -> Self {
        self.schema_source = Some(schema_source);
        self
    }

    /// Append a single message body. The bytes are written verbatim and an
    /// index entry is recorded (offset = pre-write cursor, size = body length,
    /// checksum = xxh64(seed=0) of body per GEN-0016 R1).
    ///
    /// # Errors
    /// Returns [`FileError`] if the underlying sink write fails or if the
    /// message size exceeds `u32::MAX` (the wire-format limit for the index
    /// entry's `size` field).
    pub fn write_message(&mut self, body: &[u8]) -> Result<(), FileError> {
        if !self.header_written {
            self.write_header()?;
        }
        let size = u32::try_from(body.len()).map_err(|_| FileError::InvalidIndex)?;
        let offset = self.cursor;
        let checksum = xxh64(body, 0);
        self.sink.write_all(body).map_err(io_to_file)?;
        self.cursor = self
            .cursor
            .checked_add(u64::from(size))
            .ok_or(FileError::InvalidIndex)?;
        self.index.push(IndexEntry {
            offset,
            size,
            checksum,
        });
        Ok(())
    }

    /// Finalise the file: flush header (if not yet written), index block, and
    /// footer. Consumes `self`.
    ///
    /// # Errors
    /// Returns [`FileError`] on any sink-write failure.
    pub fn finish(mut self) -> Result<(), FileError> {
        if !self.header_written {
            self.write_header()?;
        }
        // index_offset is the cursor position *before* the index block —
        // either right after the header (0-msg) or right after the last
        // message body (≥1-msg).
        let index_offset = self.cursor;

        // Per GEN-0011 #20 vibe (overflow check): message_count * INDEX_ENTRY_SIZE
        // must fit in u64. Vec::len() is usize, and `usize as u64 * 24` cannot
        // overflow u64 on any realistic platform, but be explicit.
        let message_count = u64::try_from(self.index.len()).map_err(|_| FileError::InvalidIndex)?;
        let _index_bytes = message_count
            .checked_mul(INDEX_ENTRY_SIZE as u64)
            .ok_or(FileError::InvalidIndex)?;

        // Write index block.
        for entry in &self.index {
            let mut buf = [0u8; INDEX_ENTRY_SIZE];
            buf[0..8].copy_from_slice(&entry.offset.to_le_bytes());
            buf[8..12].copy_from_slice(&entry.size.to_le_bytes());
            // buf[12..16] is `reserved` u32 LE = 0 (already zero-initialised).
            buf[16..24].copy_from_slice(&entry.checksum.to_le_bytes());
            self.sink.write_all(&buf).map_err(io_to_file)?;
        }

        // Build footer in a 32-byte buffer, then compute checksum over
        // bytes [0..24) per GEN-0016 R1.
        let mut footer = [0u8; FILE_FOOTER_SIZE];
        footer[FOOTER_INDEX_OFFSET..FOOTER_INDEX_OFFSET + 8]
            .copy_from_slice(&index_offset.to_le_bytes());
        footer[FOOTER_MESSAGE_COUNT_OFFSET..FOOTER_MESSAGE_COUNT_OFFSET + 8]
            .copy_from_slice(&message_count.to_le_bytes());
        // footer[FOOTER_RESERVED_OFFSET..+FOOTER_RESERVED_LEN] stays zero from init.
        footer[FOOTER_MAGIC_OFFSET..FOOTER_MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
        let cksum = xxh64(&footer[..FOOTER_CHECKSUM_OFFSET], 0);
        footer[FOOTER_CHECKSUM_OFFSET..FOOTER_CHECKSUM_OFFSET + 8]
            .copy_from_slice(&cksum.to_le_bytes());
        self.sink.write_all(&footer).map_err(io_to_file)?;

        Ok(())
    }

    /// Emit the 40-byte header followed by the optional schema-source
    /// block (UTF-8 + zero-pad to 8-byte boundary per GEN-0009 R2 and
    /// `format.rs:33-35`). Idempotent guard via `header_written`.
    fn write_header(&mut self) -> Result<(), FileError> {
        // schema_size is the unpadded UTF-8 byte length; pad-to-8 is added
        // after the block, not into the count.
        let schema_bytes: &[u8] = self.schema_source.map_or(&[], str::as_bytes);
        let schema_size = u32::try_from(schema_bytes.len()).map_err(|_| FileError::InvalidIndex)?;

        let mut buf = [0u8; FILE_HEADER_SIZE];
        // magic
        buf[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
        // format_version u16 LE
        buf[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2]
            .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        // flags u16 LE at HEADER_FLAGS_OFFSET stays zero (no compression in v0.1).
        // schema_hash u128 LE (16 bytes per HEADER_SCHEMA_HASH_LEN / GEN-0035)
        buf[HEADER_SCHEMA_HASH_OFFSET..HEADER_SCHEMA_HASH_OFFSET + HEADER_SCHEMA_HASH_LEN]
            .copy_from_slice(&self.schema_hash.to_le_bytes());
        // dict_id u32 LE at HEADER_DICT_ID_OFFSET stays zero (hard-zero, format.rs:28).
        // page_class u8
        buf[HEADER_PAGE_CLASS_OFFSET] = self.page_class;
        // schema_size u32 LE — unpadded byte count of the schema source block.
        buf[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4]
            .copy_from_slice(&schema_size.to_le_bytes());
        // reserved bytes already zero.

        self.sink.write_all(&buf).map_err(io_to_file)?;

        if !schema_bytes.is_empty() {
            // Write the block, then zero-pad to the next 8-byte boundary.
            // `pad_to_8(n) - n` is in 0..=7 — cheap stack-allocated buffer.
            self.sink.write_all(schema_bytes).map_err(io_to_file)?;
            let pad_len = pad_to_8(schema_bytes.len()) - schema_bytes.len();
            if pad_len > 0 {
                // Fixed 7-byte zero buffer covers the worst case (size % 8 == 1).
                let zeros = [0u8; 7];
                self.sink.write_all(&zeros[..pad_len]).map_err(io_to_file)?;
            }
        }

        self.cursor = messages_offset(schema_size) as u64;
        self.header_written = true;
        Ok(())
    }
}

/// Map a sink-side `io::Error` into a `FileError`. Stays separate from
/// `DeError` (GEN-0026 R3); the `Io` variant carries the underlying error
/// verbatim for the caller to inspect.
fn io_to_file(err: io::Error) -> FileError {
    FileError::Io(err)
}
