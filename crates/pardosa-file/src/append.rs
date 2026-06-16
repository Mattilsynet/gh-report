//! `.pgno` container append-shaped writer.
//!
//! [`AppendWriter`] is the IO-efficient sibling of
//! [`Writer`](crate::Writer): accumulates per-message
//! [`IndexEntry`](crate::IndexEntry) state in memory; sink
//! receives lazy header + bodies. [`AppendWriter::finish`] emits
//! index + footer + checksum in one trailing pass, byte-identical
//! to [`Writer::finish`](crate::Writer::finish) (ADR-0006).
//!
//! [`AppendWriter::sync_data`] fences durability without footer;
//! prefix is not [`Reader::open`](crate::Reader::open)-compatible
//! and recoverable only through the explicit recovery API.
//! `Drop` is not a durability boundary (ADR-0010).
use crate::config::PageClass;
use crate::error::FileError;
use crate::format::{
    FILE_FOOTER_SIZE, FILE_HEADER_SIZE, FOOTER_CHECKSUM_OFFSET, FOOTER_INDEX_OFFSET,
    FOOTER_MAGIC_OFFSET, FOOTER_MESSAGE_COUNT_OFFSET, FORMAT_VERSION, HEADER_FLAGS_OFFSET,
    HEADER_MAGIC_OFFSET, HEADER_PAGE_CLASS_OFFSET, HEADER_SCHEMA_HASH_LEN,
    HEADER_SCHEMA_HASH_OFFSET, HEADER_SCHEMA_SIZE_OFFSET, HEADER_VERSION_OFFSET, INDEX_ENTRY_SIZE,
    MAGIC, messages_offset, pad_to_8,
};
use crate::manifest::{IndexManifestWriter, ManifestRecord, RecoveredPrefix};
use crate::options::{Compression, WriterOptions};
use crate::syncable::Syncable;
use std::io::{self, Seek};
use xxhash_rust::xxh64::xxh64;
#[derive(Debug, Clone, Copy)]
pub(crate) struct StoredEntry {
    pub(crate) offset: u64,
    pub(crate) size: u32,
    pub(crate) checksum: u64,
}
/// Append-shaped `.pgno` container writer.
///
/// Holds an exclusive borrow of the [`Syncable`] sink. Accumulates
/// the per-message index in memory; the sink receives only the
/// lazy header and bodies until [`Self::finish`]. See module docs.
///
/// When an index manifest is attached via [`Self::with_manifest`],
/// the second [`Syncable`] sink receives an
/// [`IndexManifestWriter`](crate::manifest) update on each
/// [`Self::sync_data`] — enabling footerless prefix recovery
/// without rescanning the `.pgno` body.
pub struct AppendWriter<'w, W: Syncable, M: Syncable + Seek = std::io::Cursor<Vec<u8>>> {
    sink: &'w mut W,
    schema_hash: u128,
    page_class: u8,
    schema_size: u32,
    schema_source: Option<&'w str>,
    cursor: u64,
    header_written: bool,
    index: Vec<StoredEntry>,
    options: WriterOptions,
    manifest: Option<IndexManifestWriter<'w, M>>,
}
impl<'w, W: Syncable> AppendWriter<'w, W> {
    /// Construct an append writer over `sink` with the default
    /// [`WriterOptions`] (no compression). The header is written
    /// lazily on the first [`Self::append_message`] call.
    pub fn new(sink: &'w mut W, schema_hash: u128) -> Self {
        Self::with_options(sink, schema_hash, WriterOptions::default())
    }
    /// Construct an append writer with explicit [`WriterOptions`].
    ///
    /// Equivalent to [`Self::new`] when `options` is
    /// `WriterOptions::default()`.
    pub fn with_options(sink: &'w mut W, schema_hash: u128, options: WriterOptions) -> Self {
        Self {
            sink,
            schema_hash,
            page_class: 0,
            schema_size: 0,
            schema_source: None,
            cursor: 0,
            header_written: false,
            index: Vec::new(),
            options,
            manifest: None,
        }
    }
}
impl<'w, W: Syncable> AppendWriter<'w, W> {
    /// Set the on-disk `page_class` byte (raw `u8` ingress; see
    /// [`Writer::with_page_class`](crate::Writer::with_page_class)).
    #[must_use]
    pub fn with_page_class(mut self, page_class: u8) -> Self {
        self.page_class = page_class;
        self
    }
    /// Set the on-disk `page_class` byte from a typed
    /// [`PageClass`] discriminant (sibling of
    /// [`Self::with_page_class`]).
    #[must_use]
    pub fn with_page_class_typed(mut self, page_class: PageClass) -> Self {
        self.page_class = page_class as u8;
        self
    }
    /// Embed `schema_source` in the header on first write.
    #[must_use]
    pub fn with_schema_source(mut self, schema_source: &'w str) -> Self {
        self.schema_source = Some(schema_source);
        self
    }
    /// Resume appending after a recovered or parsed footerless prefix.
    ///
    /// The caller must position the sink at `prefix.data_end` or allow
    /// the next append to do so before writing. Existing message-index
    /// records are retained so [`Self::finish`] and manifest recovery
    /// include the prior durable prefix plus any newly appended suffix.
    #[must_use]
    pub fn resume_from_recovered_prefix(sink: &'w mut W, prefix: &RecoveredPrefix) -> Self {
        Self {
            sink,
            schema_hash: prefix.schema_hash,
            page_class: prefix.page_class,
            schema_size: prefix.schema_size,
            schema_source: None,
            cursor: prefix.data_end,
            header_written: true,
            index: prefix
                .records
                .iter()
                .map(|record| StoredEntry {
                    offset: record.offset,
                    size: record.size,
                    checksum: record.checksum,
                })
                .collect(),
            options: WriterOptions::default(),
            manifest: None,
        }
    }
    /// Attach an index-manifest sink. On every [`Self::sync_data`]
    /// call, an [`IndexManifestWriter`](crate::manifest)-shaped
    /// update is written to `manifest_sink` before the call
    /// returns; the update is O(delta records) bytes plus a
    /// 28-byte footer. Without a manifest, `sync_data` is a pure
    /// `.pgno` durability fence and footerless-prefix recovery is
    /// not possible.
    ///
    /// Returns a writer with the manifest-sink generic parameter
    /// pinned to `MNew`. Cannot be chained back to a writer with a
    /// different manifest type.
    #[must_use]
    pub fn with_manifest<MNew: Syncable + Seek>(
        self,
        manifest_sink: &'w mut MNew,
    ) -> AppendWriter<'w, W, MNew> {
        self.with_manifest_synced_records(manifest_sink, 0, false)
    }
    /// Attach an index-manifest sink that already contains a synced
    /// prefix of this writer's records.
    ///
    /// `synced_records` is the count already durable in the manifest.
    /// When `header_synced` is `true`, the manifest header is also
    /// assumed durable and the next [`Self::sync_data`] writes only
    /// records after `synced_records` plus the footer. This is the
    /// resume path used by file-mode append sessions that reopen after
    /// a recovered prefix.
    #[must_use]
    pub fn with_manifest_synced_records<MNew: Syncable + Seek>(
        self,
        manifest_sink: &'w mut MNew,
        synced_records: usize,
        header_synced: bool,
    ) -> AppendWriter<'w, W, MNew> {
        let schema_size = if self.header_written {
            self.schema_size
        } else {
            self.schema_source
                .map_or(0, |s| u32::try_from(s.len()).unwrap_or(u32::MAX))
        };
        let records = self
            .index
            .iter()
            .map(|entry| ManifestRecord {
                offset: entry.offset,
                size: entry.size,
                checksum: entry.checksum,
            })
            .collect();
        AppendWriter {
            sink: self.sink,
            schema_hash: self.schema_hash,
            page_class: self.page_class,
            schema_size,
            schema_source: self.schema_source,
            cursor: self.cursor,
            header_written: self.header_written,
            index: self.index,
            options: self.options,
            manifest: Some(IndexManifestWriter::new_with_records(
                manifest_sink,
                self.schema_hash,
                self.page_class,
                schema_size,
                records,
                synced_records,
                header_synced,
            )),
        }
    }
}
impl<W: Syncable, M: Syncable + Seek> AppendWriter<'_, W, M> {
    /// Append a single message body, updating the in-memory index.
    ///
    /// Writes the lazy header on the first call. Returns the
    /// post-append byte offset within the container (the future
    /// `index_offset` if [`Self::finish`] were called immediately
    /// after).
    ///
    /// # Errors
    /// [`FileError::InvalidIndex`] when `body.len()` exceeds
    /// `u32::MAX` or the running cursor would overflow `u64`;
    /// [`FileError::Io`] from the sink.
    pub fn append_message(&mut self, body: &[u8]) -> Result<u64, FileError> {
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
        self.index.push(StoredEntry {
            offset,
            size,
            checksum,
        });
        if let Some(m) = self.manifest.as_mut() {
            m.record(ManifestRecord {
                offset,
                size,
                checksum,
            });
        }
        Ok(self.cursor)
    }
    /// Number of messages appended so far.
    #[must_use]
    pub fn message_count(&self) -> u64 {
        self.index.len() as u64
    }
    /// Byte offset immediately past the last appended body; the
    /// future `index_offset` if [`Self::finish`] were called now.
    /// Returns `0` before the lazy header is written.
    #[must_use]
    pub fn data_end_offset(&self) -> u64 {
        self.cursor
    }
    /// Total on-disk manifest length **as of the last successful
    /// [`Self::sync_data`] call**, when a manifest is attached.
    /// Returns `None` if no manifest is attached or
    /// [`Self::sync_data`] has not yet been called.
    ///
    /// Computed without touching the manifest sink — exposes the
    /// expected post-sync length so test harnesses can observe
    /// IO-efficiency invariants (delta-only writes) without
    /// duplicating the manifest layout arithmetic.
    #[must_use]
    pub fn manifest_persisted_len(&self) -> Option<u64> {
        self.manifest
            .as_ref()
            .map(IndexManifestWriter::persisted_len)
    }
    /// Snapshot the append session's current recoverable prefix.
    #[must_use]
    pub fn recovered_prefix(&self) -> RecoveredPrefix {
        RecoveredPrefix {
            schema_hash: self.schema_hash,
            page_class: self.page_class,
            schema_size: self.schema_size,
            schema_source: self.schema_source.map(str::to_owned),
            records: self
                .index
                .iter()
                .map(|entry| ManifestRecord {
                    offset: entry.offset,
                    size: entry.size,
                    checksum: entry.checksum,
                })
                .collect(),
            data_end: self.cursor,
        }
    }
    /// Fence durability of bytes written so far. Writes the lazy
    /// header on the first call. Does NOT write a footer — the
    /// prefix is not [`Reader::open`](crate::Reader::open)-compatible.
    ///
    /// When a manifest is attached, the manifest is updated and
    /// synced **before** this call returns. Order: (1) `.pgno`
    /// sink `sync_data`; (2) manifest sink `sync_data`. The
    /// manifest never claims more data than the `.pgno` carries.
    ///
    /// # Errors
    ///
    /// Forwards [`io::Error`] from
    /// [`Syncable::sync_data`](crate::Syncable::sync_data).
    pub fn sync_data(&mut self) -> io::Result<()> {
        if !self.header_written {
            self.write_header().map_err(file_to_io)?;
        }
        <W as Syncable>::sync_data(self.sink)?;
        if let Some(m) = self.manifest.as_mut() {
            m.sync_data(self.cursor)?;
        }
        Ok(())
    }
    /// Write the index, footer, and footer checksum. Consumes
    /// `self`; produces a [`Reader::open`](crate::Reader::open)-compatible
    /// file byte-identical to [`Writer::finish`](crate::Writer::finish)
    /// for the same payload sequence.
    ///
    /// Does not fence durability; call
    /// [`Syncable::sync_data`](crate::Syncable::sync_data) on the
    /// underlying handle afterwards. The manifest is **not**
    /// touched: a completed `.pgno` is fully self-describing and
    /// the manifest is redundant after `finish`.
    ///
    /// # Errors
    /// [`FileError::InvalidIndex`] when the message count or index
    /// byte size overflows `u64`; [`FileError::Io`] from the sink.
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
    fn write_header(&mut self) -> Result<(), FileError> {
        let schema_bytes: &[u8] = self.schema_source.map_or(&[], str::as_bytes);
        let schema_size = u32::try_from(schema_bytes.len()).map_err(|_| FileError::InvalidIndex)?;
        self.schema_size = schema_size;
        let mut buf = [0u8; FILE_HEADER_SIZE];
        buf[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
        buf[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2]
            .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        let flags: u16 = match self.options.compression {
            Compression::None => 0,
            #[cfg(feature = "zstd")]
            Compression::Zstd9 | Compression::Zstd19 => u16::from(crate::format::ALGO_ZSTD),
        };
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
        self.cursor = messages_offset(schema_size) as u64;
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
