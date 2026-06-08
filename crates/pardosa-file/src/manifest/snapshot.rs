use super::record::{ManifestRecord, decode_record, encode_header, encode_record};
use crate::error::FileError;
use std::io::Write;
use xxhash_rust::xxh64::xxh64;
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
    let mut footer = [0u8; super::MANIFEST_FOOTER_SIZE];
    footer[super::MANIFEST_FOOTER_MESSAGE_COUNT_OFFSET
        ..super::MANIFEST_FOOTER_MESSAGE_COUNT_OFFSET + 8]
        .copy_from_slice(&message_count.to_le_bytes());
    footer[super::MANIFEST_FOOTER_DATA_END_OFFSET..super::MANIFEST_FOOTER_DATA_END_OFFSET + 8]
        .copy_from_slice(&data_end.to_le_bytes());
    let checksum = hasher.digest();
    footer[super::MANIFEST_FOOTER_CHECKSUM_OFFSET..super::MANIFEST_FOOTER_CHECKSUM_OFFSET + 8]
        .copy_from_slice(&checksum.to_le_bytes());
    footer[super::MANIFEST_FOOTER_MAGIC_OFFSET..super::MANIFEST_FOOTER_MAGIC_OFFSET + 4]
        .copy_from_slice(super::MANIFEST_MAGIC);
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
    if bytes.len() < super::MANIFEST_HEADER_SIZE + super::MANIFEST_FOOTER_SIZE {
        return Err(FileError::InvalidIndex);
    }
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
    if version != super::MANIFEST_VERSION {
        return Err(FileError::UnsupportedVersion(version));
    }
    if bytes[super::MANIFEST_HEADER_RESERVED0_OFFSET..super::MANIFEST_HEADER_RESERVED0_OFFSET + 2]
        .iter()
        .any(|&b| b != 0)
    {
        return Err(FileError::InvalidReserved);
    }
    let schema_hash = u128::from_le_bytes(
        bytes[super::MANIFEST_HEADER_SCHEMA_HASH_OFFSET
            ..super::MANIFEST_HEADER_SCHEMA_HASH_OFFSET + 16]
            .try_into()
            .expect("slice len 16"),
    );
    let page_class = bytes[super::MANIFEST_HEADER_PAGE_CLASS_OFFSET];
    if bytes[super::MANIFEST_HEADER_RESERVED1_OFFSET..super::MANIFEST_HEADER_RESERVED1_OFFSET + 3]
        .iter()
        .any(|&b| b != 0)
    {
        return Err(FileError::InvalidReserved);
    }
    let schema_size = u32::from_le_bytes(
        bytes[super::MANIFEST_HEADER_SCHEMA_SIZE_OFFSET
            ..super::MANIFEST_HEADER_SCHEMA_SIZE_OFFSET + 4]
            .try_into()
            .expect("slice len 4"),
    );
    let footer_start = bytes.len() - super::MANIFEST_FOOTER_SIZE;
    let footer = &bytes[footer_start..];
    if &footer[super::MANIFEST_FOOTER_MAGIC_OFFSET..super::MANIFEST_FOOTER_MAGIC_OFFSET + 4]
        != super::MANIFEST_MAGIC
    {
        return Err(FileError::InvalidMagic);
    }
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
    let claimed_checksum = u64::from_le_bytes(
        footer[super::MANIFEST_FOOTER_CHECKSUM_OFFSET..super::MANIFEST_FOOTER_CHECKSUM_OFFSET + 8]
            .try_into()
            .expect("slice len 8"),
    );
    let records_byte_len = message_count
        .checked_mul(super::MANIFEST_RECORD_SIZE as u64)
        .ok_or(FileError::IndexOverflow)?;
    let expected_total = (super::MANIFEST_HEADER_SIZE as u64)
        .checked_add(records_byte_len)
        .and_then(|v| v.checked_add(super::MANIFEST_FOOTER_SIZE as u64))
        .ok_or(FileError::IndexOverflow)?;
    if expected_total != bytes.len() as u64 {
        return Err(FileError::InvalidIndex);
    }
    let computed_checksum = xxh64(&bytes[..footer_start], 0);
    if computed_checksum != claimed_checksum {
        return Err(FileError::InvalidChecksum);
    }
    let count = usize::try_from(message_count).map_err(|_| FileError::IndexOverflow)?;
    let mut records = Vec::with_capacity(count);
    let records_region = &bytes[super::MANIFEST_HEADER_SIZE..footer_start];
    for i in 0..count {
        let r = decode_record(
            records_region[i * super::MANIFEST_RECORD_SIZE..(i + 1) * super::MANIFEST_RECORD_SIZE]
                .try_into()
                .expect("slice len MANIFEST_RECORD_SIZE"),
        )?;
        records.push(r);
    }
    Ok(ManifestSnapshot {
        schema_hash,
        page_class,
        schema_size,
        records,
        data_end,
    })
}
