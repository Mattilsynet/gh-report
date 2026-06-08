//! `.pgno` container reader.
//!
//! # Integrity
//!
//! Per-message + footer `xxh64`: accidental-corruption
//! detector (bit-rot, truncated writes, transport flips,
//! buggy scribbles) — **not** tamper-resistant.
//! Non-cryptographic; a malicious producer trivially forges a
//! matching checksum. ADR-0006 §D4.
//!
//! Adopters with adversarial threat models layer a
//! cryptographic MAC (BLAKE3-keyed / HMAC) externally. The
//! in-tree frontier chain (ADR-0004) makes per-event tampering
//! detectable end-to-end but assumes a trusted writer.
use crate::config::PageClass;
use crate::error::FileError;
use crate::format::{
    FILE_FOOTER_SIZE, FILE_HEADER_SIZE, FOOTER_CHECKSUM_OFFSET, FOOTER_INDEX_OFFSET,
    FOOTER_MAGIC_OFFSET, FOOTER_MESSAGE_COUNT_OFFSET, FOOTER_RESERVED_LEN, FOOTER_RESERVED_OFFSET,
    FORMAT_VERSION, HEADER_DICT_ID_OFFSET, HEADER_FLAGS_OFFSET, HEADER_MAGIC_OFFSET,
    HEADER_PAGE_CLASS_OFFSET, HEADER_RESERVED_LEN, HEADER_RESERVED_OFFSET, HEADER_SCHEMA_HASH_LEN,
    HEADER_SCHEMA_HASH_OFFSET, HEADER_SCHEMA_SIZE_OFFSET, HEADER_VERSION_OFFSET, INDEX_ENTRY_SIZE,
    MAGIC, messages_offset, pad_to_8,
};
use crate::options::ReaderOptions;
use std::io::{Read, Seek, SeekFrom};
use xxhash_rust::xxh64::xxh64;
/// One entry of the `.pgno` per-message offset index.
/// Returned by [`Reader::index`] as `&[IndexEntry]`.
///
/// # Field stability
///
/// Fields are currently `pub` for pre-1.0 compatibility but
/// **prefer the accessors** ([`offset`](Self::offset),
/// [`size`](Self::size), [`checksum`](Self::checksum)) —
/// per ADR-0009 fields go private at the next major bump.
///
/// `#[non_exhaustive]` (added this 0.x cycle): blocks external
/// destructuring and construction. ADR-0006 §3 pins the
/// wire-format to these three fields.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct IndexEntry {
    pub offset: u64,
    pub size: u32,
    pub checksum: u64,
}
impl IndexEntry {
    /// Byte offset of this message body within the `.pgno` container,
    /// measured from byte 0 of the file. Always `>= messages_offset(
    /// schema_size)` and strictly ascending across the index.
    #[must_use]
    pub fn offset(&self) -> u64 {
        self.offset
    }
    /// Stored body size in bytes (compressed bytes if the container
    /// uses `ALGO_ZSTD`; raw bytes otherwise). Bounded by `u32::MAX`
    /// at write time.
    #[must_use]
    pub fn size(&self) -> u32 {
        self.size
    }
    /// `xxh64` checksum over the **stored** body bytes (see
    /// the [`Reader`](crate::Reader) integrity-model docs for the
    /// threat-model scope — accidental-corruption detection only,
    /// not tamper resistance).
    #[must_use]
    pub fn checksum(&self) -> u64 {
        self.checksum
    }
}
#[derive(Debug)]
pub struct Reader<R: Read + Seek> {
    inner: R,
    schema_hash: u128,
    page_class: u8,
    schema_size: u32,
    schema_source: Option<String>,
    message_count: u64,
    index: Vec<IndexEntry>,
    #[cfg_attr(
        not(feature = "zstd"),
        allow(dead_code, reason = "consumed only by the zstd decompression path")
    )]
    compression_algo: u8,
    #[cfg_attr(
        not(feature = "zstd"),
        allow(dead_code, reason = "consumed only by the zstd decompression path")
    )]
    options: ReaderOptions,
}
impl<R: Read + Seek> Reader<R> {
    /// Open a pardosa-file from `source`, parsing header,
    /// footer, and index.
    ///
    /// # Errors
    /// `FileError::Io` for I/O failures; `InvalidMagic`,
    /// `UnsupportedVersion`, `UnsupportedCompression`,
    /// `InvalidReserved`, `InvalidSchemaSource`,
    /// `InvalidChecksum`, `InvalidIndex`, or `IndexOverflow`
    /// for structural rejections.
    pub fn open(source: R) -> Result<Self, FileError> {
        Self::open_with_options(source, ReaderOptions::default())
    }
    /// Open a pardosa-file with explicit reader options.
    ///
    /// Equivalent to [`Reader::open`] but lets callers override the
    /// decompressed-payload cap and other tunables.
    ///
    /// # Errors
    /// Same conditions as [`Reader::open`].
    ///
    /// # Panics
    /// Panics on internal invariant violation: fixed-length slice
    /// conversions sourced from previously-bounded buffers
    /// (header, footer, index entries). These slices are read into
    /// fixed-size byte arrays whose lengths match the destination
    /// integer width by construction, so the conversions are
    /// infallible in practice; the [`expect`](Result::expect) calls
    /// document that contract.
    #[expect(
        clippy::too_many_lines,
        reason = "header → schema → footer → index is a single top-to-bottom decode \
                  sequence; arbitrary mid-sequence extraction would obscure the file-format \
                  layout that the function mirrors line-for-line."
    )]
    pub fn open_with_options(mut source: R, options: ReaderOptions) -> Result<Self, FileError> {
        source.seek(SeekFrom::Start(0)).map_err(FileError::Io)?;
        let mut header = [0u8; FILE_HEADER_SIZE];
        source.read_exact(&mut header).map_err(FileError::Io)?;
        if header[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 4] != MAGIC {
            return Err(FileError::InvalidMagic);
        }
        let version = u16::from_le_bytes(
            header[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2]
                .try_into()
                .expect("slice len 2"),
        );
        if version != FORMAT_VERSION {
            return Err(FileError::UnsupportedVersion(version));
        }
        let flags = u16::from_le_bytes(
            header[HEADER_FLAGS_OFFSET..HEADER_FLAGS_OFFSET + 2]
                .try_into()
                .expect("slice len 2"),
        );
        if flags & !0b111 != 0 {
            return Err(FileError::UnsupportedCompression((flags & 0b111) as u8));
        }
        let compression_algo = (flags & 0b111) as u8;
        match compression_algo {
            crate::format::ALGO_NONE => {}
            #[cfg(feature = "zstd")]
            crate::format::ALGO_ZSTD => {}
            #[cfg(not(feature = "zstd"))]
            crate::format::ALGO_ZSTD => return Err(FileError::CompressionNotAvailable),
            other => return Err(FileError::UnsupportedCompression(other)),
        }
        let schema_hash = u128::from_le_bytes(
            header[HEADER_SCHEMA_HASH_OFFSET..HEADER_SCHEMA_HASH_OFFSET + HEADER_SCHEMA_HASH_LEN]
                .try_into()
                .expect("slice len 16"),
        );
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
        if header[HEADER_RESERVED_OFFSET..HEADER_RESERVED_OFFSET + HEADER_RESERVED_LEN]
            .iter()
            .any(|&b| b != 0)
        {
            return Err(FileError::InvalidReserved);
        }
        let schema_source = if schema_size == 0 {
            None
        } else {
            if schema_size > options.max_schema_source_bytes {
                return Err(FileError::SchemaSourceTooLarge {
                    claimed: schema_size,
                    limit: options.max_schema_source_bytes,
                });
            }
            let size = usize::try_from(schema_size).map_err(|_| FileError::IndexOverflow)?;
            let mut buf = vec![0u8; size];
            source.read_exact(&mut buf).map_err(FileError::Io)?;
            let s = String::from_utf8(buf).map_err(|_| FileError::InvalidSchemaSource)?;
            let pad_len = pad_to_8(size) - size;
            if pad_len > 0 {
                let mut pad = [0u8; 7];
                source
                    .read_exact(&mut pad[..pad_len])
                    .map_err(FileError::Io)?;
                if pad[..pad_len].iter().any(|&b| b != 0) {
                    return Err(FileError::InvalidReserved);
                }
            }
            Some(s)
        };
        let file_len = source.seek(SeekFrom::End(0)).map_err(FileError::Io)?;
        if file_len < (FILE_HEADER_SIZE + FILE_FOOTER_SIZE) as u64 {
            return Err(FileError::InvalidIndex);
        }
        source
            .seek(SeekFrom::Start(file_len - FILE_FOOTER_SIZE as u64))
            .map_err(FileError::Io)?;
        let mut footer = [0u8; FILE_FOOTER_SIZE];
        source.read_exact(&mut footer).map_err(FileError::Io)?;
        if footer[FOOTER_MAGIC_OFFSET..FOOTER_MAGIC_OFFSET + 4] != MAGIC {
            return Err(FileError::InvalidMagic);
        }
        if footer[FOOTER_RESERVED_OFFSET..FOOTER_RESERVED_OFFSET + FOOTER_RESERVED_LEN]
            .iter()
            .any(|&b| b != 0)
        {
            return Err(FileError::InvalidReserved);
        }
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
        if message_count > options.max_message_count {
            return Err(FileError::IndexTooLarge {
                claimed: message_count,
                limit: options.max_message_count,
            });
        }
        let index_bytes = message_count
            .checked_mul(INDEX_ENTRY_SIZE as u64)
            .ok_or(FileError::IndexOverflow)?;
        let msgs_start = messages_offset(schema_size) as u64;
        if index_offset < msgs_start {
            return Err(FileError::InvalidIndex);
        }
        let index_end = index_offset
            .checked_add(index_bytes)
            .ok_or(FileError::IndexOverflow)?;
        let index_end_with_footer = index_end
            .checked_add(FILE_FOOTER_SIZE as u64)
            .ok_or(FileError::IndexOverflow)?;
        if index_end_with_footer != file_len {
            return Err(FileError::InvalidIndex);
        }
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
                let offset = u64::from_le_bytes(entry_buf[0..8].try_into().expect("slice len 8"));
                let size = u32::from_le_bytes(entry_buf[8..12].try_into().expect("slice len 4"));
                let reserved =
                    u32::from_le_bytes(entry_buf[12..16].try_into().expect("slice len 4"));
                let checksum =
                    u64::from_le_bytes(entry_buf[16..24].try_into().expect("slice len 8"));
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
        let mut prev_end: u64 = msgs_start;
        for entry in &index {
            if entry.offset < msgs_start {
                return Err(FileError::InvalidIndex);
            }
            if entry.offset < prev_end {
                return Err(FileError::InvalidIndex);
            }
            let end = entry
                .offset
                .checked_add(u64::from(entry.size))
                .ok_or(FileError::IndexOverflow)?;
            if end > index_offset {
                return Err(FileError::InvalidIndex);
            }
            prev_end = end;
        }
        Ok(Self {
            inner: source,
            schema_hash,
            page_class,
            schema_size,
            schema_source,
            message_count,
            index,
            compression_algo,
            options,
        })
    }
    #[must_use]
    pub fn message_count(&self) -> u64 {
        self.message_count
    }
    #[must_use]
    pub fn schema_hash(&self) -> u128 {
        self.schema_hash
    }
    /// Raw on-disk `page_class` byte, surfaced verbatim.
    ///
    /// `page_class` is opaque at the substrate boundary (ADR-0006
    /// §5–§6): any `u8` written by [`Writer::with_page_class`](crate::Writer::with_page_class)
    /// round-trips here unchanged. Callers that want a validated
    /// typed view of the four substrate-defined discriminants
    /// should use [`page_class_typed`](Self::page_class_typed).
    #[must_use]
    pub fn page_class(&self) -> u8 {
        self.page_class
    }
    /// Validated typed view of the stored `page_class` byte.
    ///
    /// Returns `Some(PageClass)` when the stored byte is one of the
    /// four substrate-defined discriminants (`0..=3`) and `None`
    /// otherwise — the byte itself is still legal on disk (ADR-0006
    /// §5–§6 opaque-byte contract) and remains visible via
    /// [`page_class`](Self::page_class). Equivalent to
    /// `PageClass::from_byte(self.page_class())`; sibling, not
    /// replacement.
    #[must_use]
    pub fn page_class_typed(&self) -> Option<PageClass> {
        PageClass::from_byte(self.page_class)
    }
    #[must_use]
    pub fn schema_source(&self) -> Option<&str> {
        self.schema_source.as_deref()
    }
    #[must_use]
    pub fn index(&self) -> &[IndexEntry] {
        &self.index
    }
    #[must_use]
    pub fn schema_size(&self) -> u32 {
        self.schema_size
    }
    /// Read the `idx`-th message body and verify its xxh64
    /// against the index.
    ///
    /// For `ALGO_ZSTD` files the checksum covers **stored
    /// (compressed) bytes** (ADR-0006 §4); decompression is
    /// bounded by
    /// [`ReaderOptions::with_max_decompressed_message_bytes`].
    ///
    /// # Errors
    /// `InvalidIndex` if `idx` out of range; `Io` on I/O
    /// failure; `ChecksumMismatch(idx)` on mismatch;
    /// `DecompressedTooLarge { limit }` if zstd output exceeds
    /// cap. Zstd decode errors surface as `Io(InvalidData)`.
    pub fn read_message(&mut self, idx: usize) -> Result<Vec<u8>, FileError> {
        let entry = self
            .index
            .get(idx)
            .copied()
            .ok_or(FileError::InvalidIndex)?;
        let size = entry.size as usize;
        let mut buf = vec![0u8; size];
        self.inner
            .seek(SeekFrom::Start(entry.offset))
            .map_err(FileError::Io)?;
        self.inner.read_exact(&mut buf).map_err(FileError::Io)?;
        let computed = xxh64(&buf, 0);
        if computed != entry.checksum {
            return Err(FileError::ChecksumMismatch(idx as u64));
        }
        match self.compression_algo {
            crate::format::ALGO_NONE => Ok(buf),
            #[cfg(feature = "zstd")]
            crate::format::ALGO_ZSTD => {
                decompress_zstd_capped(&buf, self.options.max_decompressed_message_bytes())
            }
            other => Err(FileError::UnsupportedCompression(other)),
        }
    }
    pub fn iter_messages(&mut self) -> MessageIter<'_, R> {
        MessageIter {
            reader: self,
            next: 0,
        }
    }
    pub fn into_inner(self) -> R {
        self.inner
    }
}
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
/// Decompress a zstd frame, bounded by `cap` decompressed bytes.
///
/// Uses a streaming decoder so that frames lacking a content-size header
/// are still bounded: the decoder writes into a fixed-capacity buffer and
/// stops the moment output would exceed `cap`. The cap is *strict*: a
/// frame producing exactly `cap` bytes is accepted; one producing
/// `cap + 1` is rejected with [`FileError::DecompressedTooLarge`].
#[cfg(feature = "zstd")]
fn decompress_zstd_capped(stored: &[u8], cap: usize) -> Result<Vec<u8>, FileError> {
    use std::io::Read as _;
    let cursor = std::io::Cursor::new(stored);
    let mut decoder = zstd::stream::read::Decoder::new(cursor).map_err(FileError::Io)?;
    let mut out = Vec::with_capacity(cap.min(64 * 1024));
    let cap_plus_one = u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1);
    let n = (&mut decoder)
        .take(cap_plus_one)
        .read_to_end(&mut out)
        .map_err(FileError::Io)?;
    if n > cap {
        return Err(FileError::DecompressedTooLarge { limit: cap });
    }
    let mut probe = [0u8; 1];
    match decoder.read(&mut probe) {
        Ok(0) => Ok(out),
        Ok(_) => Err(FileError::DecompressedTooLarge { limit: cap }),
        Err(e) => Err(FileError::Io(e)),
    }
}
