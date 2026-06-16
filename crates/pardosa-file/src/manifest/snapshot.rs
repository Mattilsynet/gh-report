use super::record::{ManifestRecord, decode_record, encode_footer, encode_header, encode_record};
use crate::error::FileError;
use std::io::Write;
use xxhash_rust::xxh64::xxh64;
const LEGACY_V1: u16 = 1;
const LEGACY_V1_FOOTER_SIZE: usize = 28;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManifestWireVersion {
    V1,
    V2,
}
impl ManifestWireVersion {
    fn parse(version: u16) -> Result<Self, FileError> {
        match version {
            LEGACY_V1 => Ok(Self::V1),
            super::MANIFEST_VERSION => Ok(Self::V2),
            _ => Err(FileError::UnsupportedVersion(version)),
        }
    }

    const fn footer_size(self) -> usize {
        match self {
            Self::V1 => LEGACY_V1_FOOTER_SIZE,
            Self::V2 => super::MANIFEST_FOOTER_SIZE,
        }
    }

    const fn magic_offset(self) -> usize {
        match self {
            Self::V1 => 24,
            Self::V2 => super::MANIFEST_FOOTER_MAGIC_OFFSET,
        }
    }

    const fn checksum_offset(self) -> usize {
        match self {
            Self::V1 => 16,
            Self::V2 => super::MANIFEST_FOOTER_CHECKSUM_OFFSET,
        }
    }
}
/// Parsed manifest contents: enough to reconstruct the in-memory
/// index of an interrupted [`AppendWriter`](crate::AppendWriter)
/// without rescanning the `.pgno`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestSnapshot {
    /// `.pgno` schema hash this manifest is bound to.
    pub schema_hash: u128,
    /// `.pgno` page class this manifest is bound to.
    pub page_class: u8,
    /// `.pgno` schema-source byte length this manifest is bound to.
    pub schema_size: u32,
    /// Recovered index records, in append order.
    pub records: Vec<ManifestRecord>,
    /// Byte offset within the `.pgno` immediately past the last
    /// manifested body. The append writer's
    /// [`data_end_offset`](crate::AppendWriter::data_end_offset)
    /// at the sync that produced this manifest.
    pub data_end: u64,
    /// Rolling BLAKE3 frontier stamped by version-2 manifests.
    pub frontier: Option<[u8; 32]>,
}
/// Write a complete manifest into `sink` from byte 0. Slice-1 helper
/// for tests; the incremental, IO-efficient on-disk update path
/// lives on the manifest-writer that drives the `.pgno` append
/// writer.
///
/// # Errors
/// [`FileError::Io`] from the sink; [`FileError::InvalidIndex`] when
/// the record count overflows `u64`.
pub fn write_complete_manifest<W: Write>(
    sink: &mut W,
    schema_hash: u128,
    page_class: u8,
    schema_size: u32,
    records: &[ManifestRecord],
    data_end: u64,
    frontier: [u8; 32],
) -> Result<(), FileError> {
    let header = encode_header(schema_hash, page_class, schema_size);
    sink.write_all(&header).map_err(FileError::Io)?;
    let mut hasher = xxhash_rust::xxh64::Xxh64::new(0);
    hasher.update(&header);
    let mut record_buf = [0u8; super::MANIFEST_RECORD_SIZE];
    for r in records {
        encode_record(r, &mut record_buf);
        sink.write_all(&record_buf).map_err(FileError::Io)?;
        hasher.update(&record_buf);
    }
    let message_count = u64::try_from(records.len()).map_err(|_| FileError::InvalidIndex)?;
    hasher.update(&frontier);
    let checksum = hasher.digest();
    let footer = encode_footer(message_count, data_end, frontier, checksum);
    sink.write_all(&footer).map_err(FileError::Io)?;
    Ok(())
}
/// Parse a complete manifest from a byte slice.
///
/// # Errors
/// [`FileError::InvalidMagic`] if the header or footer magic
/// is wrong; [`FileError::UnsupportedVersion`] for an unrecognised
/// version byte; [`FileError::InvalidReserved`] if any reserved
/// field is non-zero; [`FileError::InvalidIndex`] if the buffer is
/// shorter than the minimum size, or the declared
/// `message_count` does not match the body length;
/// [`FileError::InvalidChecksum`] on footer-checksum mismatch.
///
/// # Panics
/// Will not panic: every slice-to-array conversion is preceded by
/// a length check; the `.expect` strings document that invariant.
pub fn parse_manifest(bytes: &[u8]) -> Result<ManifestSnapshot, FileError> {
    if bytes.len() < super::MANIFEST_HEADER_SIZE + LEGACY_V1_FOOTER_SIZE {
        return Err(FileError::InvalidIndex);
    }
    let wire = parse_wire_version(bytes)?;
    let footer_size = wire.footer_size();
    if bytes.len() < super::MANIFEST_HEADER_SIZE + footer_size {
        return Err(FileError::InvalidIndex);
    }
    validate_header_reserved(bytes)?;
    let (schema_hash, page_class, schema_size) = parse_header_payload(bytes);
    let footer_start = bytes.len() - footer_size;
    let footer = &bytes[footer_start..];
    if &footer[wire.magic_offset()..wire.magic_offset() + 4] != super::MANIFEST_MAGIC {
        return Err(FileError::InvalidMagic);
    }
    let footer_fields = parse_footer_fields(wire, footer);
    validate_manifest_length(footer_fields.message_count, footer_size, bytes.len())?;
    validate_manifest_checksum(bytes, footer_start, footer_fields)?;
    let records = parse_records(
        footer_fields.message_count,
        &bytes[super::MANIFEST_HEADER_SIZE..footer_start],
    )?;
    Ok(ManifestSnapshot {
        schema_hash,
        page_class,
        schema_size,
        records,
        data_end: footer_fields.data_end,
        frontier: footer_fields.frontier,
    })
}
fn parse_wire_version(bytes: &[u8]) -> Result<ManifestWireVersion, FileError> {
    if &bytes[super::MANIFEST_HEADER_MAGIC_OFFSET..super::MANIFEST_HEADER_MAGIC_OFFSET + 4]
        != super::MANIFEST_MAGIC
    {
        return Err(FileError::InvalidMagic);
    }
    let version = u16::from_le_bytes(
        bytes[super::MANIFEST_HEADER_VERSION_OFFSET..super::MANIFEST_HEADER_VERSION_OFFSET + 2]
            .try_into()
            .expect("slice len 2"),
    );
    ManifestWireVersion::parse(version)
}
fn validate_header_reserved(bytes: &[u8]) -> Result<(), FileError> {
    if bytes[super::MANIFEST_HEADER_RESERVED0_OFFSET..super::MANIFEST_HEADER_RESERVED0_OFFSET + 2]
        .iter()
        .any(|&b| b != 0)
        || bytes
            [super::MANIFEST_HEADER_RESERVED1_OFFSET..super::MANIFEST_HEADER_RESERVED1_OFFSET + 3]
            .iter()
            .any(|&b| b != 0)
    {
        return Err(FileError::InvalidReserved);
    }
    Ok(())
}
fn parse_header_payload(bytes: &[u8]) -> (u128, u8, u32) {
    let schema_hash = u128::from_le_bytes(
        bytes[super::MANIFEST_HEADER_SCHEMA_HASH_OFFSET
            ..super::MANIFEST_HEADER_SCHEMA_HASH_OFFSET + 16]
            .try_into()
            .expect("slice len 16"),
    );
    let page_class = bytes[super::MANIFEST_HEADER_PAGE_CLASS_OFFSET];
    let schema_size = u32::from_le_bytes(
        bytes[super::MANIFEST_HEADER_SCHEMA_SIZE_OFFSET
            ..super::MANIFEST_HEADER_SCHEMA_SIZE_OFFSET + 4]
            .try_into()
            .expect("slice len 4"),
    );
    (schema_hash, page_class, schema_size)
}
#[derive(Debug, Clone, Copy)]
struct ManifestFooterFields {
    message_count: u64,
    data_end: u64,
    claimed_checksum: u64,
    frontier: Option<[u8; 32]>,
}
fn parse_footer_fields(wire: ManifestWireVersion, footer: &[u8]) -> ManifestFooterFields {
    let message_count = u64::from_le_bytes(
        footer[super::MANIFEST_FOOTER_MESSAGE_COUNT_OFFSET
            ..super::MANIFEST_FOOTER_MESSAGE_COUNT_OFFSET + 8]
            .try_into()
            .expect("slice len 8"),
    );
    let data_end = u64::from_le_bytes(
        footer[super::MANIFEST_FOOTER_DATA_END_OFFSET..super::MANIFEST_FOOTER_DATA_END_OFFSET + 8]
            .try_into()
            .expect("slice len 8"),
    );
    let frontier = match wire {
        ManifestWireVersion::V1 => None,
        ManifestWireVersion::V2 => Some(
            footer[super::MANIFEST_FOOTER_FRONTIER_OFFSET
                ..super::MANIFEST_FOOTER_FRONTIER_OFFSET + 32]
                .try_into()
                .expect("slice len 32"),
        ),
    };
    let claimed_checksum = u64::from_le_bytes(
        footer[wire.checksum_offset()..wire.checksum_offset() + 8]
            .try_into()
            .expect("slice len 8"),
    );
    ManifestFooterFields {
        message_count,
        data_end,
        claimed_checksum,
        frontier,
    }
}
fn validate_manifest_length(
    message_count: u64,
    footer_size: usize,
    byte_len: usize,
) -> Result<(), FileError> {
    let records_byte_len = message_count
        .checked_mul(super::MANIFEST_RECORD_SIZE as u64)
        .ok_or(FileError::IndexOverflow)?;
    let expected_total = (super::MANIFEST_HEADER_SIZE as u64)
        .checked_add(records_byte_len)
        .and_then(|v| v.checked_add(footer_size as u64))
        .ok_or(FileError::IndexOverflow)?;
    if expected_total != byte_len as u64 {
        return Err(FileError::InvalidIndex);
    }
    Ok(())
}
fn validate_manifest_checksum(
    bytes: &[u8],
    footer_start: usize,
    footer: ManifestFooterFields,
) -> Result<(), FileError> {
    let computed_checksum = if let Some(frontier) = footer.frontier {
        let mut hasher = xxhash_rust::xxh64::Xxh64::new(0);
        hasher.update(&bytes[..footer_start]);
        hasher.update(&frontier);
        hasher.digest()
    } else {
        xxh64(&bytes[..footer_start], 0)
    };
    if computed_checksum != footer.claimed_checksum {
        return Err(FileError::InvalidChecksum);
    }
    Ok(())
}
fn parse_records(
    message_count: u64,
    records_region: &[u8],
) -> Result<Vec<ManifestRecord>, FileError> {
    let count = usize::try_from(message_count).map_err(|_| FileError::IndexOverflow)?;
    let mut records = Vec::with_capacity(count);
    for i in 0..count {
        let r = decode_record(
            records_region[i * super::MANIFEST_RECORD_SIZE..(i + 1) * super::MANIFEST_RECORD_SIZE]
                .try_into()
                .expect("slice len MANIFEST_RECORD_SIZE"),
        )?;
        records.push(r);
    }
    Ok(records)
}
