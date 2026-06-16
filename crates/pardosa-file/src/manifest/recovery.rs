use super::error::RecoveryError;
use super::record::ManifestRecord;
use super::snapshot::{ManifestSnapshot, parse_manifest};
use crate::error::FileError;
use crate::format::{
    FILE_FOOTER_SIZE, FILE_HEADER_SIZE, FOOTER_CHECKSUM_OFFSET, FOOTER_INDEX_OFFSET,
    FOOTER_MAGIC_OFFSET, FOOTER_MESSAGE_COUNT_OFFSET, FORMAT_VERSION as PGNO_FORMAT_VERSION,
    HEADER_FLAGS_OFFSET, HEADER_MAGIC_OFFSET, HEADER_PAGE_CLASS_OFFSET, HEADER_SCHEMA_HASH_LEN,
    HEADER_SCHEMA_HASH_OFFSET, HEADER_SCHEMA_SIZE_OFFSET, HEADER_VERSION_OFFSET, INDEX_ENTRY_SIZE,
    MAGIC as PGNO_MAGIC, messages_offset,
};
use std::io::{Seek, SeekFrom};
use xxhash_rust::xxh64::xxh64;
/// Recovered footerless-`.pgno` prefix returned by
/// [`recover_footerless_prefix`]. The prefix carries enough state
/// to either finalize the `.pgno` into a
/// [`Reader::open`](crate::Reader::open)-compatible file via
/// [`finalize_recovered_prefix`], or to surface the recovered
/// message index to a higher-level rehydration path.
#[derive(Debug, Clone)]
pub struct RecoveredPrefix {
    /// `.pgno` schema hash, as written in the header.
    pub schema_hash: u128,
    /// `.pgno` page class byte.
    pub page_class: u8,
    /// `.pgno` schema-source byte length.
    pub schema_size: u32,
    /// `.pgno` schema source string, when present in the header.
    pub schema_source: Option<String>,
    /// Recovered per-message index, in append order. Each entry
    /// corresponds to one durably-synced
    /// [`AppendWriter::append_message`](crate::AppendWriter::append_message)
    /// covered by the manifest.
    pub records: Vec<ManifestRecord>,
    /// Byte offset within the `.pgno` immediately past the last
    /// recovered body. `finalize_recovered_prefix` writes the
    /// index + footer starting at this offset.
    pub data_end: u64,
    /// Rolling BLAKE3 frontier carried by version-2 manifests.
    pub frontier: Option<[u8; 32]>,
}

/// Reader failure class that admitted torn-tail recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecoveryReaderErrorKind {
    /// `.pgno` reader saw invalid magic outside the durable region.
    InvalidMagic,
    /// `.pgno` reader saw invalid index bytes outside the durable region.
    InvalidIndex,
    /// `.pgno` reader saw invalid checksum outside the durable region.
    InvalidChecksum,
    /// `.pgno` reader saw non-zero reserved bytes outside the durable region.
    InvalidReserved,
}

impl RecoveryReaderErrorKind {
    /// Convert a reader [`FileError`] into an admitted recovery class.
    #[must_use]
    pub fn from_file_error(error: &FileError) -> Option<Self> {
        match error {
            FileError::InvalidMagic => Some(Self::InvalidMagic),
            FileError::InvalidIndex => Some(Self::InvalidIndex),
            FileError::InvalidChecksum => Some(Self::InvalidChecksum),
            FileError::InvalidReserved => Some(Self::InvalidReserved),
            _ => None,
        }
    }

    /// Stable field value for structured recovery telemetry.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidMagic => "InvalidMagic",
            Self::InvalidIndex => "InvalidIndex",
            Self::InvalidChecksum => "InvalidChecksum",
            Self::InvalidReserved => "InvalidReserved",
        }
    }
}

/// Successful torn-tail recovery data returned by the synchronous store facade.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecoveryOutcome {
    /// Reader failure class that triggered recovery admission.
    pub reader_error: RecoveryReaderErrorKind,
    /// Number of manifest records recovered into the finalized `.pgno`.
    pub recovered_records: u64,
    /// Number of trailing bytes discarded from the original `.pgno`.
    pub truncated_bytes: u64,
    /// Byte offset immediately after the last durable message body.
    pub last_durable_offset: u64,
    /// Message count declared by the manifest that authorized recovery.
    pub manifest_message_count: u64,
}

impl RecoveryOutcome {
    /// Construct successful torn-tail recovery data.
    #[must_use]
    pub fn new(
        reader_error: RecoveryReaderErrorKind,
        recovered_records: u64,
        truncated_bytes: u64,
        last_durable_offset: u64,
        manifest_message_count: u64,
    ) -> Self {
        Self {
            reader_error,
            recovered_records,
            truncated_bytes,
            last_durable_offset,
            manifest_message_count,
        }
    }
}

/// Recover the index of a footerless `.pgno` synced prefix from a
/// companion manifest. Validates cross-file binding (schema hash,
/// page class, schema size) and per-record xxh64 against body bytes.
///
/// With [`finalize_recovered_prefix`] this is the only path that
/// produces a [`Reader::open`](crate::Reader::open)-compatible
/// `.pgno` from a footerless prefix (ADR-0006).
///
/// # Errors
///
/// See [`RecoveryError`].
///
/// # Panics
///
/// Will not panic: slice-to-array conversions are length-checked.
pub fn recover_footerless_prefix(
    pgno_bytes: &[u8],
    manifest_bytes: &[u8],
) -> Result<RecoveredPrefix, RecoveryError> {
    let snap = parse_manifest(manifest_bytes).map_err(RecoveryError::Manifest)?;
    let schema_source = validate_pgno_header_and_extract_schema(pgno_bytes, &snap)?;
    let body_start = messages_offset(snap.schema_size) as u64;
    let mut computed_frontier = super::GENESIS_FRONTIER;
    for (i, rec) in snap.records.iter().enumerate() {
        let i_u64 = i as u64;
        if rec.offset < body_start {
            return Err(RecoveryError::Manifest(FileError::InvalidIndex));
        }
        let rec_end = rec
            .offset
            .checked_add(u64::from(rec.size))
            .ok_or(RecoveryError::Manifest(FileError::IndexOverflow))?;
        if rec_end > snap.data_end {
            return Err(RecoveryError::BodyOverrunsDataEnd {
                message_index: i_u64,
                record_end: rec_end,
                data_end: snap.data_end,
            });
        }
        let offset_usize = usize::try_from(rec.offset)
            .map_err(|_| RecoveryError::Manifest(FileError::IndexOverflow))?;
        let end_usize = usize::try_from(rec_end)
            .map_err(|_| RecoveryError::Manifest(FileError::IndexOverflow))?;
        let body = &pgno_bytes[offset_usize..end_usize];
        let computed = xxh64(body, 0);
        if computed != rec.checksum {
            return Err(RecoveryError::BodyChecksumMismatch {
                message_index: i_u64,
                offset: rec.offset,
            });
        }
        computed_frontier = super::roll_frontier(computed_frontier, body);
    }
    if let Some(expected) = snap.frontier
        && expected != computed_frontier
    {
        return Err(RecoveryError::FrontierMismatch {
            expected,
            computed: computed_frontier,
        });
    }
    Ok(RecoveredPrefix {
        schema_hash: snap.schema_hash,
        page_class: snap.page_class,
        schema_size: snap.schema_size,
        schema_source,
        records: snap.records,
        data_end: snap.data_end,
        frontier: snap.frontier,
    })
}
fn validate_pgno_header_and_extract_schema(
    pgno_bytes: &[u8],
    snap: &ManifestSnapshot,
) -> Result<Option<String>, RecoveryError> {
    if pgno_bytes.len() < FILE_HEADER_SIZE {
        return Err(RecoveryError::DataEndExceedsFile {
            manifest_data_end: snap.data_end,
            pgno_len: pgno_bytes.len() as u64,
        });
    }
    let pgno_header = &pgno_bytes[..FILE_HEADER_SIZE];
    if pgno_header[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 4] != PGNO_MAGIC {
        return Err(RecoveryError::PgnoHeader(FileError::InvalidMagic));
    }
    let pgno_version = u16::from_le_bytes(
        pgno_header[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2]
            .try_into()
            .expect("slice len 2"),
    );
    if pgno_version != PGNO_FORMAT_VERSION {
        return Err(RecoveryError::PgnoHeader(FileError::UnsupportedVersion(
            pgno_version,
        )));
    }
    let flags = u16::from_le_bytes(
        pgno_header[HEADER_FLAGS_OFFSET..HEADER_FLAGS_OFFSET + 2]
            .try_into()
            .expect("slice len 2"),
    );
    if flags & !0b111 != 0 {
        return Err(RecoveryError::PgnoHeader(
            FileError::UnsupportedCompression((flags & 0b111) as u8),
        ));
    }
    let pgno_schema_hash = u128::from_le_bytes(
        pgno_header[HEADER_SCHEMA_HASH_OFFSET..HEADER_SCHEMA_HASH_OFFSET + HEADER_SCHEMA_HASH_LEN]
            .try_into()
            .expect("slice len 16"),
    );
    if pgno_schema_hash != snap.schema_hash {
        return Err(RecoveryError::SchemaHashMismatch {
            manifest: snap.schema_hash,
            pgno: pgno_schema_hash,
        });
    }
    let pgno_page_class = pgno_header[HEADER_PAGE_CLASS_OFFSET];
    if pgno_page_class != snap.page_class {
        return Err(RecoveryError::PageClassMismatch {
            manifest: snap.page_class,
            pgno: pgno_page_class,
        });
    }
    let pgno_schema_size = u32::from_le_bytes(
        pgno_header[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4]
            .try_into()
            .expect("slice len 4"),
    );
    if pgno_schema_size != snap.schema_size {
        return Err(RecoveryError::SchemaSizeMismatch {
            manifest: snap.schema_size,
            pgno: pgno_schema_size,
        });
    }
    if (pgno_bytes.len() as u64) < snap.data_end {
        return Err(RecoveryError::DataEndExceedsFile {
            manifest_data_end: snap.data_end,
            pgno_len: pgno_bytes.len() as u64,
        });
    }
    if snap.schema_size == 0 {
        Ok(None)
    } else {
        let start = FILE_HEADER_SIZE;
        let size = snap.schema_size as usize;
        let end = start
            .checked_add(size)
            .ok_or(RecoveryError::PgnoHeader(FileError::IndexOverflow))?;
        if pgno_bytes.len() < end {
            return Err(RecoveryError::DataEndExceedsFile {
                manifest_data_end: snap.data_end,
                pgno_len: pgno_bytes.len() as u64,
            });
        }
        let raw = &pgno_bytes[start..end];
        let s = std::str::from_utf8(raw)
            .map_err(|_| RecoveryError::PgnoHeader(FileError::InvalidSchemaSource))?
            .to_owned();
        Ok(Some(s))
    }
}
/// Write the index, footer, and footer checksum into `sink` at the
/// recovered prefix's `data_end`, producing a
/// [`Reader::open`](crate::Reader::open)-compatible `.pgno`
/// byte-identical to [`Writer::finish`](crate::Writer::finish).
///
/// `sink` must already contain the `.pgno` header + body region the
/// [`RecoveredPrefix`] was extracted from. Bytes past `data_end` are
/// truncated via [`Syncable::set_len`](crate::Syncable::set_len), so
/// a `.pgno` with an un-manifested tail is salvaged down to its last
/// durable sync.
///
/// # Errors
///
/// [`FileError::Io`] from the sink; [`FileError::InvalidIndex`] when
/// the record count overflows `u64`.
pub fn finalize_recovered_prefix<W>(prefix: &RecoveredPrefix, sink: &mut W) -> Result<(), FileError>
where
    W: crate::Syncable + Seek,
{
    let total_message_count =
        u64::try_from(prefix.records.len()).map_err(|_| FileError::InvalidIndex)?;
    let _index_bytes = total_message_count
        .checked_mul(INDEX_ENTRY_SIZE as u64)
        .ok_or(FileError::InvalidIndex)?;
    <W as crate::Syncable>::set_len(sink, prefix.data_end).map_err(FileError::Io)?;
    sink.seek(SeekFrom::Start(prefix.data_end))
        .map_err(FileError::Io)?;
    for entry in &prefix.records {
        let mut buf = [0u8; INDEX_ENTRY_SIZE];
        buf[0..8].copy_from_slice(&entry.offset.to_le_bytes());
        buf[8..12].copy_from_slice(&entry.size.to_le_bytes());
        buf[16..24].copy_from_slice(&entry.checksum.to_le_bytes());
        sink.write_all(&buf).map_err(FileError::Io)?;
    }
    let mut footer = [0u8; FILE_FOOTER_SIZE];
    footer[FOOTER_INDEX_OFFSET..FOOTER_INDEX_OFFSET + 8]
        .copy_from_slice(&prefix.data_end.to_le_bytes());
    footer[FOOTER_MESSAGE_COUNT_OFFSET..FOOTER_MESSAGE_COUNT_OFFSET + 8]
        .copy_from_slice(&total_message_count.to_le_bytes());
    footer[FOOTER_MAGIC_OFFSET..FOOTER_MAGIC_OFFSET + 4].copy_from_slice(&PGNO_MAGIC);
    let cksum = xxh64(&footer[..FOOTER_CHECKSUM_OFFSET], 0);
    footer[FOOTER_CHECKSUM_OFFSET..FOOTER_CHECKSUM_OFFSET + 8]
        .copy_from_slice(&cksum.to_le_bytes());
    sink.write_all(&footer).map_err(FileError::Io)?;
    Ok(())
}
