//! v2 file-format Reader.
//!
//! Opens a file produced by [`crate::file::Writer`] and validates its
//! geometry inline at open time (GEN-0011 R2 — no `open_unchecked` path).
//! Header + footer + index checks run at [`Reader::open`]; per-message
//! payload reads via [`Reader::read_message`] / [`Reader::iter_messages`]
//! verify each body's xxh64 against the stored index entry (GEN-0011 #14,
//! GEN-0016 R1) before yielding bytes.
//!
//! Checks executed at [`Reader::open`]:
//!  * #13 magic at header offset 0 and footer offset 20 must be "PGNO".
//!  * Format version (u16 LE at header offset 4) must equal
//!    [`FORMAT_VERSION`] = 2.
//!  * #16 reserved regions must be all-zero: header bytes 33..40,
//!    footer bytes 16..20, and each index entry's `reserved:u32` field.
//!    Routed through [`FileError::InvalidReserved`].
//!  * #17 footer checksum: `xxh64(seed=0)` over footer[0..24] must
//!    match the LE u64 at footer[24..32].
//!  * #18 index well-formedness: each `offset` is ≥
//!    `messages_offset(schema_size)`; each `offset + size` is ≤
//!    `index_offset` (entries live in the pre-index message region);
//!    strictly monotonic on `offset`; non-overlapping (`entries[i].offset
//!    + size ≤ entries[i+1].offset`). Routed through
//!    [`FileError::InvalidIndex`].
//!  * #20 overflow: `message_count * INDEX_ENTRY_SIZE` must not overflow
//!    `u64`; `index_offset + index_bytes` must not overflow `u64`;
//!    per-entry `offset + size` must not overflow `u64`; and
//!    `message_count` must fit `usize` on this platform. Routed through
//!    [`FileError::IndexOverflow`] — distinct from #18 so callers can
//!    distinguish "this file is malformed" from "this file's index
//!    cannot be addressed on this host".
//!
//! Design notes:
//!  * `R: Read + Seek` — Reader is allowed to seek (package brief, SM-2
//!    success criterion #2). Writer remains streaming `Write`-only.
//!  * Index entries are parsed once into an in-memory `Vec` so SM-3 can
//!    iterate without re-seeking.
//!  * `FileError` stays separate from `DeError` (GEN-0026 R3). No
//!    message-payload errors are surfaced in SM-2.

use std::io::{Read, Seek, SeekFrom};

use crate::error::FileError;
use crate::format::{
    FILE_FOOTER_SIZE, FILE_HEADER_SIZE, FOOTER_CHECKSUM_OFFSET, FOOTER_INDEX_OFFSET,
    FOOTER_MAGIC_OFFSET, FOOTER_MESSAGE_COUNT_OFFSET, FOOTER_RESERVED_LEN, FOOTER_RESERVED_OFFSET,
    FORMAT_VERSION, HEADER_DICT_ID_OFFSET, HEADER_FLAGS_OFFSET, HEADER_MAGIC_OFFSET,
    HEADER_PAGE_CLASS_OFFSET, HEADER_RESERVED_LEN, HEADER_RESERVED_OFFSET, HEADER_SCHEMA_HASH_LEN,
    HEADER_SCHEMA_HASH_OFFSET, HEADER_SCHEMA_SIZE_OFFSET, HEADER_VERSION_OFFSET, INDEX_ENTRY_SIZE,
    MAGIC, messages_offset,
};
use xxhash_rust::xxh64::xxh64;

/// Parsed message index entry.
///
/// 24 wire bytes per entry: `offset:u64 LE`, `size:u32 LE`,
/// `reserved:u32 LE (=0)`, `checksum:u64 LE`. The `checksum` is xxh64
/// (seed=0) of the message body (GEN-0016 R1); verified by
/// [`Reader::read_message`].
#[derive(Debug, Clone, Copy)]
pub struct IndexEntry {
    /// Byte offset of the message body within the file.
    pub offset: u64,
    /// Message body length in bytes.
    pub size: u32,
    /// xxh64(seed=0) of the message body. Verified at read time.
    pub checksum: u64,
}

/// v2 genome-file reader.
///
/// Use [`Reader::open`] to validate and parse the file's geometry.
/// Accessors expose header metadata and the parsed index; message-body
/// access is the SM-3 surface.
#[derive(Debug)]
pub struct Reader<R: Read + Seek> {
    inner: R,
    schema_hash: u128,
    page_class: u8,
    schema_size: u32,
    message_count: u64,
    index: Vec<IndexEntry>,
}

impl<R: Read + Seek> Reader<R> {
    /// Open `source`, validating header + footer + index in one pass.
    ///
    /// # Errors
    /// Returns [`FileError`] on any structural defect. See module docs
    /// for the full list of checks.
    pub fn open(mut source: R) -> Result<Self, FileError> {
        // ── Header ──────────────────────────────────────────────────
        source.seek(SeekFrom::Start(0)).map_err(FileError::Io)?;
        let mut header = [0u8; FILE_HEADER_SIZE];
        source.read_exact(&mut header).map_err(FileError::Io)?;

        // #13 — header magic.
        if header[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 4] != MAGIC {
            return Err(FileError::InvalidMagic);
        }

        // Format version.
        let version = u16::from_le_bytes(
            header[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2]
                .try_into()
                .expect("slice len 2"),
        );
        if version != FORMAT_VERSION {
            return Err(FileError::UnsupportedVersion(version));
        }

        // Flags: SM-2 accepts only zero (no compression). Compressed
        // paths land later; signal them as unsupported so we don't
        // silently accept undefined wire.
        let flags = u16::from_le_bytes(
            header[HEADER_FLAGS_OFFSET..HEADER_FLAGS_OFFSET + 2]
                .try_into()
                .expect("slice len 2"),
        );
        if flags != 0 {
            // Low 3 bits encode compression_algo per format.rs:26; surface
            // any non-zero algo as unsupported in SM-2 scope.
            #[allow(clippy::cast_possible_truncation)]
            let algo = (flags & 0b111) as u8;
            return Err(FileError::UnsupportedCompression(algo));
        }

        // schema_hash u128 LE at offset 8.
        let schema_hash = u128::from_le_bytes(
            header[HEADER_SCHEMA_HASH_OFFSET..HEADER_SCHEMA_HASH_OFFSET + HEADER_SCHEMA_HASH_LEN]
                .try_into()
                .expect("slice len 16"),
        );

        // dict_id u32 LE: hard-zero in v2 (format.rs:28). Treat any
        // non-zero value as a wire violation — surface as InvalidReserved
        // since the field is effectively reserved for future versions.
        let dict_id = u32::from_le_bytes(
            header[HEADER_DICT_ID_OFFSET..HEADER_DICT_ID_OFFSET + 4]
                .try_into()
                .expect("slice len 4"),
        );
        if dict_id != 0 {
            return Err(FileError::InvalidReserved);
        }

        let page_class = header[HEADER_PAGE_CLASS_OFFSET];

        let schema_size = u32::from_le_bytes(
            header[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4]
                .try_into()
                .expect("slice len 4"),
        );

        // #16 header reserved zeros.
        if header[HEADER_RESERVED_OFFSET..HEADER_RESERVED_OFFSET + HEADER_RESERVED_LEN]
            .iter()
            .any(|&b| b != 0)
        {
            return Err(FileError::InvalidReserved);
        }

        // ── Footer ──────────────────────────────────────────────────
        let file_len = source.seek(SeekFrom::End(0)).map_err(FileError::Io)?;
        if file_len < (FILE_HEADER_SIZE + FILE_FOOTER_SIZE) as u64 {
            // File too short to contain both header and footer; surface
            // as InvalidIndex (geometry violation) rather than Io to
            // give callers a structural rather than transport diagnostic.
            return Err(FileError::InvalidIndex);
        }
        source
            .seek(SeekFrom::Start(file_len - FILE_FOOTER_SIZE as u64))
            .map_err(FileError::Io)?;
        let mut footer = [0u8; FILE_FOOTER_SIZE];
        source.read_exact(&mut footer).map_err(FileError::Io)?;

        // #13 footer magic.
        if footer[FOOTER_MAGIC_OFFSET..FOOTER_MAGIC_OFFSET + 4] != MAGIC {
            return Err(FileError::InvalidMagic);
        }

        // #16 footer reserved zeros.
        if footer[FOOTER_RESERVED_OFFSET..FOOTER_RESERVED_OFFSET + FOOTER_RESERVED_LEN]
            .iter()
            .any(|&b| b != 0)
        {
            return Err(FileError::InvalidReserved);
        }

        // #17 footer checksum: xxh64(seed=0) over footer[0..24].
        let claimed_cksum = u64::from_le_bytes(
            footer[FOOTER_CHECKSUM_OFFSET..FOOTER_CHECKSUM_OFFSET + 8]
                .try_into()
                .expect("slice len 8"),
        );
        let computed_cksum = xxh64(&footer[..FOOTER_CHECKSUM_OFFSET], 0);
        if computed_cksum != claimed_cksum {
            return Err(FileError::InvalidChecksum);
        }

        let index_offset = u64::from_le_bytes(
            footer[FOOTER_INDEX_OFFSET..FOOTER_INDEX_OFFSET + 8]
                .try_into()
                .expect("slice len 8"),
        );
        let message_count = u64::from_le_bytes(
            footer[FOOTER_MESSAGE_COUNT_OFFSET..FOOTER_MESSAGE_COUNT_OFFSET + 8]
                .try_into()
                .expect("slice len 8"),
        );

        // ── Index geometry ──────────────────────────────────────────
        // #20 overflow check: message_count * INDEX_ENTRY_SIZE must fit u64.
        let index_bytes = message_count
            .checked_mul(INDEX_ENTRY_SIZE as u64)
            .ok_or(FileError::IndexOverflow)?;

        // index_offset must point into the file, after the message region,
        // and the index region plus footer must fit before EOF.
        let msgs_start = messages_offset(schema_size) as u64;
        if index_offset < msgs_start {
            return Err(FileError::InvalidIndex);
        }
        let index_end = index_offset
            .checked_add(index_bytes)
            .ok_or(FileError::IndexOverflow)?;
        if index_end + (FILE_FOOTER_SIZE as u64) != file_len {
            // The footer must start exactly at index_end. A mismatch
            // means either the index spans into the footer or there's
            // padding/extra bytes we don't accept.
            return Err(FileError::InvalidIndex);
        }

        // Parse index entries. Pre-size in usize, which on 32-bit hosts
        // could itself overflow — treat that as #20 too.
        let capacity = usize::try_from(message_count).map_err(|_| FileError::IndexOverflow)?;
        let index: Vec<IndexEntry> = if capacity == 0 {
            Vec::new()
        } else {
            source
                .seek(SeekFrom::Start(index_offset))
                .map_err(FileError::Io)?;
            let mut v = Vec::with_capacity(capacity);
            let mut entry_buf = [0u8; INDEX_ENTRY_SIZE];
            for _ in 0..capacity {
                source.read_exact(&mut entry_buf).map_err(FileError::Io)?;
                let offset = u64::from_le_bytes(entry_buf[0..8].try_into().expect("8"));
                let size = u32::from_le_bytes(entry_buf[8..12].try_into().expect("4"));
                let reserved = u32::from_le_bytes(entry_buf[12..16].try_into().expect("4"));
                let checksum = u64::from_le_bytes(entry_buf[16..24].try_into().expect("8"));
                // Per-entry reserved-zero is a #16-class violation, same
                // as header/footer reserved regions — route accordingly.
                if reserved != 0 {
                    return Err(FileError::InvalidReserved);
                }
                v.push(IndexEntry {
                    offset,
                    size,
                    checksum,
                });
            }
            v
        };

        // #18 per-entry geometry + monotonic + non-overlapping.
        let mut prev_end: u64 = msgs_start;
        for entry in &index {
            if entry.offset < msgs_start {
                return Err(FileError::InvalidIndex);
            }
            if entry.offset < prev_end {
                // Either non-monotonic (offset < previous entry's offset)
                // or overlapping with the previous entry's body.
                return Err(FileError::InvalidIndex);
            }
            let end = entry
                .offset
                .checked_add(u64::from(entry.size))
                .ok_or(FileError::IndexOverflow)?;
            if end > index_offset {
                // Message body extends past the start of the index block.
                return Err(FileError::InvalidIndex);
            }
            prev_end = end;
        }

        Ok(Self {
            inner: source,
            schema_hash,
            page_class,
            schema_size,
            message_count,
            index,
        })
    }

    /// Number of messages in the file (footer's `message_count` field).
    #[must_use]
    pub fn message_count(&self) -> u64 {
        self.message_count
    }

    /// File's schema hash (xxh3-128 per GEN-0035; widened from v1 per
    /// `format.rs:14`).
    #[must_use]
    pub fn schema_hash(&self) -> u128 {
        self.schema_hash
    }

    /// `page_class` byte from the header. Semantics opaque to v0.1 (G5);
    /// callers interpret.
    #[must_use]
    pub fn page_class(&self) -> u8 {
        self.page_class
    }

    /// Embedded schema source. SM-2 always returns `None`; SM-4 parses
    /// the block and exposes the UTF-8 string.
    #[must_use]
    #[allow(clippy::unused_self)]
    pub fn schema_source(&self) -> Option<&str> {
        None
    }

    /// Parsed index entries. Useful to SM-3 for per-message iteration.
    #[must_use]
    pub fn index(&self) -> &[IndexEntry] {
        &self.index
    }

    /// Raw `schema_size` from the header. SM-2 always 0 (no schema block);
    /// SM-4 reads non-zero. Exposed so [`Reader::read_message`] can locate
    /// message bodies via the index entries (the helper isn't called
    /// directly here — entries store absolute offsets — but it pins the
    /// invariant that bodies live at `messages_offset(schema_size) ..
    /// index_offset`).
    #[must_use]
    pub fn schema_size(&self) -> u32 {
        self.schema_size
    }

    /// Read the body of message `idx`, verifying its xxh64 against the
    /// stored index entry before returning the bytes (GEN-0011 #14,
    /// GEN-0016 R1 — xxh64 seed=0, full 64 bits, no truncation).
    ///
    /// # Errors
    /// Returns [`FileError::InvalidIndex`] when `idx >= message_count`.
    /// Returns [`FileError::ChecksumMismatch`] when the body's xxh64
    /// does not match the index entry. Returns [`FileError::Io`] on a
    /// read-side I/O failure.
    pub fn read_message(&mut self, idx: usize) -> Result<Vec<u8>, FileError> {
        let entry = self
            .index
            .get(idx)
            .copied()
            .ok_or(FileError::InvalidIndex)?;
        // size:u32 → usize is widening on every supported host.
        let size = entry.size as usize;
        let mut buf = vec![0u8; size];
        self.inner
            .seek(SeekFrom::Start(entry.offset))
            .map_err(FileError::Io)?;
        self.inner.read_exact(&mut buf).map_err(FileError::Io)?;
        let computed = xxh64(&buf, 0);
        if computed != entry.checksum {
            // Index payload of ChecksumMismatch is the message index in
            // storage order. `idx` fits u64 because it was bounded above
            // by `self.index.len()` which came from `message_count: u64`.
            return Err(FileError::ChecksumMismatch(idx as u64));
        }
        Ok(buf)
    }

    /// Iterate every message in storage order, verifying each body's
    /// xxh64 before yielding. Sugar over [`Reader::read_message`].
    ///
    /// The iterator yields `Result<Vec<u8>, FileError>` so callers can
    /// see *which* message failed. A `ChecksumMismatch(i)` does not
    /// poison subsequent iterations — the caller decides whether to
    /// keep reading or abort.
    pub fn iter_messages(&mut self) -> MessageIter<'_, R> {
        MessageIter {
            reader: self,
            next: 0,
        }
    }

    /// Consume the Reader, returning the underlying source.
    pub fn into_inner(self) -> R {
        self.inner
    }
}

/// Iterator over message bodies in storage order, verifying each body's
/// xxh64 before yielding. Returned by [`Reader::iter_messages`].
#[derive(Debug)]
pub struct MessageIter<'r, R: Read + Seek> {
    reader: &'r mut Reader<R>,
    next: usize,
}

impl<R: Read + Seek> Iterator for MessageIter<'_, R> {
    type Item = Result<Vec<u8>, FileError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.reader.index.len() {
            return None;
        }
        let item = self.reader.read_message(self.next);
        self.next += 1;
        Some(item)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.reader.index.len().saturating_sub(self.next);
        (remaining, Some(remaining))
    }
}

impl<R: Read + Seek> ExactSizeIterator for MessageIter<'_, R> {}
