//! `.pgno` container writer.
//!
//! # Lifecycle
//!
//! * [`Writer::write_message`] — append-only; lazy header on
//!   first call. No durability fence.
//! * [`Writer::sync_data`] — fences durability so far; writer
//!   stays usable; no footer (prefix files unopenable).
//! * [`Writer::finish`] — consumes `self`; writes index +
//!   footer + checksum. Only openable path. Caller calls
//!   `sync_data` for durability.
//!
//! `Drop` is not a durability boundary. ADR-0010.
//!
//! # Integrity
//!
//! `xxh64` (accidental, not MAC). ADR-0006 §D4.
use crate::config::PageClass;
use crate::error::FileError;
use crate::format::{
    EPOCH_LEN_PREFIX_SIZE, FILE_FOOTER_SIZE, FILE_HEADER_SIZE, FOOTER_CHECKSUM_OFFSET,
    FOOTER_INDEX_OFFSET, FOOTER_MAGIC_OFFSET, FOOTER_MESSAGE_COUNT_OFFSET, FORMAT_VERSION,
    HEADER_EPOCH_PRESENT_FLAG, HEADER_FLAGS_OFFSET, HEADER_MAGIC_OFFSET, HEADER_PAGE_CLASS_OFFSET,
    HEADER_SCHEMA_HASH_LEN, HEADER_SCHEMA_HASH_OFFSET, HEADER_SCHEMA_SIZE_OFFSET,
    HEADER_VERSION_OFFSET, INDEX_ENTRY_SIZE, MAGIC, messages_offset_with_epoch, pad_to_8,
};
use crate::options::{Compression, WriterOptions};
use crate::syncable::Syncable;
use std::io;
use xxhash_rust::xxh64::xxh64;
#[derive(Debug, Clone, Copy)]
struct IndexEntry {
    offset: u64,
    size: u32,
    checksum: u64,
}
/// Single-pass `.pgno` container writer.
///
/// Holds an exclusive borrow of the [`Syncable`] sink. See the
/// crate-level module docs for the lifecycle contract.
///
/// `page_class` is opaque (`u8` round-trips through
/// [`Reader::page_class`](crate::Reader::page_class)).
/// Compression is signalled by the low 3 bits of header
/// `flags` (`ALGO_NONE=0`, `ALGO_ZSTD=1`); selected via
/// [`WriterOptions::compression`](crate::WriterOptions).
/// ADR-0006 §5–§6.
pub struct Writer<'w, W: Syncable> {
    sink: &'w mut W,
    schema_hash: u128,
    page_class: u8,
    schema_source: Option<&'w str>,
    epoch: Option<&'w [u8]>,
    cursor: u64,
    header_written: bool,
    index: Vec<IndexEntry>,
    options: WriterOptions,
}
impl<'w, W: Syncable> Writer<'w, W> {
    pub fn new(sink: &'w mut W, schema_hash: u128) -> Self {
        Self::with_options(sink, schema_hash, WriterOptions::default())
    }
    /// Construct a `Writer` with explicit options.
    ///
    /// `WriterOptions::default()` matches `Writer::new` exactly and produces
    /// byte-identical output to the pre-Z1 writer.
    pub fn with_options(sink: &'w mut W, schema_hash: u128, options: WriterOptions) -> Self {
        Self {
            sink,
            schema_hash,
            page_class: 0,
            schema_source: None,
            epoch: None,
            cursor: 0,
            header_written: false,
            index: Vec::new(),
            options,
        }
    }
    /// Set the on-disk `page_class` byte directly.
    ///
    /// Accepts any `u8`; `page_class` is opaque from the substrate's
    /// perspective (ADR-0006 §5–§6) and the byte round-trips through
    /// [`Reader::page_class`](crate::Reader::page_class) unchanged.
    /// Prefer [`with_page_class_typed`](Self::with_page_class_typed)
    /// for callers that know they are emitting one of the four
    /// substrate-defined [`PageClass`] discriminants; the raw setter
    /// remains the canonical entry point for any future or
    /// adopter-defined extension byte.
    #[must_use]
    pub fn with_page_class(mut self, page_class: u8) -> Self {
        self.page_class = page_class;
        self
    }
    /// Set the on-disk `page_class` byte from a typed [`PageClass`]
    /// discriminant. Byte-identical to
    /// `with_page_class(class as u8)` — sibling, not replacement.
    ///
    /// This is the recommended path for callers that intend one of
    /// the four substrate-defined classes; the raw `u8` ingress
    /// remains for the opaque-byte extension axis (ADR-0006 §5–§6).
    #[must_use]
    pub fn with_page_class_typed(mut self, page_class: PageClass) -> Self {
        self.page_class = page_class as u8;
        self
    }
    #[must_use]
    pub fn with_schema_source(mut self, schema_source: &'w str) -> Self {
        self.schema_source = Some(schema_source);
        self
    }
    /// Set the opaque `adopter_epoch` token (PGN-0021).
    ///
    /// `Some(&[])` (present, zero-length) is on-disk distinct from
    /// `None` (absent) — the presence bit in header `flags` carries
    /// the discriminant, never the length (PGN-0021 R3). pardosa
    /// never interprets these bytes; they are stored and later
    /// byte-compared only.
    #[must_use]
    pub fn with_epoch(mut self, epoch: &'w [u8]) -> Self {
        self.epoch = Some(epoch);
        self
    }
    /// Append a single message body and its xxh64 checksum to the file, updating the
    /// in-memory index.
    ///
    /// # Errors
    /// Returns `FileError::InvalidIndex` if `body.len()` exceeds `u32::MAX` or if the
    /// running cursor would overflow `u64`; any `FileError::Io` from the underlying sink.
    pub fn write_message(&mut self, body: &[u8]) -> Result<(), FileError> {
        if !self.header_written {
            self.write_header()?;
        }
        let stored: std::borrow::Cow<'_, [u8]> = match self.options.compression {
            Compression::None => std::borrow::Cow::Borrowed(body),
            #[cfg(feature = "zstd")]
            Compression::Zstd9 => {
                std::borrow::Cow::Owned(zstd::bulk::compress(body, 9).map_err(io_to_file)?)
            }
            #[cfg(feature = "zstd")]
            Compression::Zstd19 => {
                std::borrow::Cow::Owned(zstd::bulk::compress(body, 19).map_err(io_to_file)?)
            }
        };
        let size = u32::try_from(stored.len()).map_err(|_| FileError::InvalidIndex)?;
        let offset = self.cursor;
        let checksum = xxh64(&stored, 0);
        self.sink.write_all(&stored).map_err(io_to_file)?;
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
    /// Flush trailing index, footer, and checksum; finalise.
    ///
    /// # Durability
    ///
    /// Produces an openable file but does not fence durability.
    /// `Writer` is consumed; call [`Syncable::sync_data`] on the
    /// underlying handle after `finish`, or use
    /// [`Journal::sync_data`](../../pardosa/journal/struct.Journal.html#method.sync_data),
    /// which composes finish + sync + truncate. The composition is
    /// not crash-atomic file replacement.
    ///
    /// # Errors
    /// [`FileError::InvalidIndex`] if message count or index byte
    /// size overflows `u64`; [`FileError::Io`] from the sink.
    pub fn finish(mut self) -> Result<(), FileError> {
        if !self.header_written {
            self.write_header()?;
        }
        let index_offset = self.cursor;
        let message_count = u64::try_from(self.index.len()).map_err(|_| FileError::InvalidIndex)?;
        let _index_bytes = message_count
            .checked_mul(INDEX_ENTRY_SIZE as u64)
            .ok_or(FileError::InvalidIndex)?;
        for entry in &self.index {
            let mut buf = [0u8; INDEX_ENTRY_SIZE];
            buf[0..8].copy_from_slice(&entry.offset.to_le_bytes());
            buf[8..12].copy_from_slice(&entry.size.to_le_bytes());
            buf[16..24].copy_from_slice(&entry.checksum.to_le_bytes());
            self.sink.write_all(&buf).map_err(io_to_file)?;
        }
        let mut footer = [0u8; FILE_FOOTER_SIZE];
        footer[FOOTER_INDEX_OFFSET..FOOTER_INDEX_OFFSET + 8]
            .copy_from_slice(&index_offset.to_le_bytes());
        footer[FOOTER_MESSAGE_COUNT_OFFSET..FOOTER_MESSAGE_COUNT_OFFSET + 8]
            .copy_from_slice(&message_count.to_le_bytes());
        footer[FOOTER_MAGIC_OFFSET..FOOTER_MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
        let cksum = xxh64(&footer[..FOOTER_CHECKSUM_OFFSET], 0);
        footer[FOOTER_CHECKSUM_OFFSET..FOOTER_CHECKSUM_OFFSET + 8]
            .copy_from_slice(&cksum.to_le_bytes());
        self.sink.write_all(&footer).map_err(io_to_file)?;
        Ok(())
    }
    /// Forward `Syncable::sync_data` to the sink. Persists
    /// every byte written so far (header on first call). The
    /// Writer remains open; further `write_message` permitted.
    ///
    /// Substrate-pure: returns `io::Error` directly rather
    /// than wrapping into `FileError` — durability failure is
    /// structurally distinct from framing failure.
    ///
    /// # Errors
    /// Forwards `io::Error` from `<W as Syncable>::sync_data`.
    /// `io::Error::InvalidInput` if lazy header write produces
    /// a non-Io `FileError` (structurally unreachable).
    pub fn sync_data(&mut self) -> io::Result<()> {
        if !self.header_written {
            self.write_header().map_err(file_to_io)?;
        }
        <W as Syncable>::sync_data(self.sink)
    }
    fn write_header(&mut self) -> Result<(), FileError> {
        let schema_bytes: &[u8] = self.schema_source.map_or(&[], str::as_bytes);
        let schema_size = u32::try_from(schema_bytes.len()).map_err(|_| FileError::InvalidIndex)?;
        let epoch_len = match self.epoch {
            Some(bytes) => Some(u32::try_from(bytes.len()).map_err(|_| FileError::InvalidIndex)?),
            None => None,
        };
        let mut buf = [0u8; FILE_HEADER_SIZE];
        buf[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
        buf[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2]
            .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        let mut flags: u16 = match self.options.compression {
            Compression::None => 0,
            #[cfg(feature = "zstd")]
            Compression::Zstd9 | Compression::Zstd19 => u16::from(crate::format::ALGO_ZSTD),
        };
        if self.epoch.is_some() {
            flags |= HEADER_EPOCH_PRESENT_FLAG;
        }
        buf[HEADER_FLAGS_OFFSET..HEADER_FLAGS_OFFSET + 2].copy_from_slice(&flags.to_le_bytes());
        buf[HEADER_SCHEMA_HASH_OFFSET..HEADER_SCHEMA_HASH_OFFSET + HEADER_SCHEMA_HASH_LEN]
            .copy_from_slice(&self.schema_hash.to_le_bytes());
        buf[HEADER_PAGE_CLASS_OFFSET] = self.page_class;
        buf[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4]
            .copy_from_slice(&schema_size.to_le_bytes());
        self.sink.write_all(&buf).map_err(io_to_file)?;
        if !schema_bytes.is_empty() {
            self.sink.write_all(schema_bytes).map_err(io_to_file)?;
            let pad_len = pad_to_8(schema_bytes.len()) - schema_bytes.len();
            if pad_len > 0 {
                let zeros = [0u8; 7];
                self.sink.write_all(&zeros[..pad_len]).map_err(io_to_file)?;
            }
        }
        if let Some(bytes) = self.epoch {
            let len_prefix = u32::try_from(bytes.len())
                .map_err(|_| FileError::InvalidIndex)?
                .to_le_bytes();
            self.sink.write_all(&len_prefix).map_err(io_to_file)?;
            self.sink.write_all(bytes).map_err(io_to_file)?;
            let written = EPOCH_LEN_PREFIX_SIZE + bytes.len();
            let pad_len = pad_to_8(written) - written;
            if pad_len > 0 {
                let zeros = [0u8; 7];
                self.sink.write_all(&zeros[..pad_len]).map_err(io_to_file)?;
            }
        }
        self.cursor = messages_offset_with_epoch(schema_size, epoch_len) as u64;
        self.header_written = true;
        Ok(())
    }
}
fn io_to_file(err: io::Error) -> FileError {
    FileError::Io(err)
}
fn file_to_io(err: FileError) -> io::Error {
    match err {
        FileError::Io(e) => e,
        other => io::Error::new(io::ErrorKind::InvalidInput, format!("{other}")),
    }
}
