use crate::error::FileError;
/// One manifest record: the same `(offset, size, checksum)` triple
/// the `.pgno` footer's index carries, persisted incrementally so
/// recovery does not have to scan the body region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManifestRecord {
    /// Body offset within the `.pgno` container (matches
    /// [`IndexEntry::offset`](crate::IndexEntry::offset)).
    pub offset: u64,
    /// Stored body size in bytes (matches
    /// [`IndexEntry::size`](crate::IndexEntry::size)).
    pub size: u32,
    /// `xxh64` over the stored body bytes (matches
    /// [`IndexEntry::checksum`](crate::IndexEntry::checksum)).
    pub checksum: u64,
}
pub(crate) fn encode_header(
    schema_hash: u128,
    page_class: u8,
    schema_size: u32,
) -> [u8; super::MANIFEST_HEADER_SIZE] {
    let mut buf = [0u8; super::MANIFEST_HEADER_SIZE];
    buf[super::MANIFEST_HEADER_MAGIC_OFFSET..super::MANIFEST_HEADER_MAGIC_OFFSET + 4]
        .copy_from_slice(super::MANIFEST_MAGIC);
    buf[super::MANIFEST_HEADER_VERSION_OFFSET..super::MANIFEST_HEADER_VERSION_OFFSET + 2]
        .copy_from_slice(&super::MANIFEST_VERSION.to_le_bytes());
    buf[super::MANIFEST_HEADER_SCHEMA_HASH_OFFSET..super::MANIFEST_HEADER_SCHEMA_HASH_OFFSET + 16]
        .copy_from_slice(&schema_hash.to_le_bytes());
    buf[super::MANIFEST_HEADER_PAGE_CLASS_OFFSET] = page_class;
    buf[super::MANIFEST_HEADER_SCHEMA_SIZE_OFFSET..super::MANIFEST_HEADER_SCHEMA_SIZE_OFFSET + 4]
        .copy_from_slice(&schema_size.to_le_bytes());
    buf
}
pub(crate) fn encode_record(r: &ManifestRecord, buf: &mut [u8; super::MANIFEST_RECORD_SIZE]) {
    buf[0..8].copy_from_slice(&r.offset.to_le_bytes());
    buf[8..12].copy_from_slice(&r.size.to_le_bytes());
    buf[12..16].copy_from_slice(&[0u8; 4]);
    buf[16..24].copy_from_slice(&r.checksum.to_le_bytes());
}
pub(crate) fn encode_footer(
    message_count: u64,
    data_end: u64,
    frontier: [u8; 32],
    checksum: u64,
) -> [u8; super::MANIFEST_FOOTER_SIZE] {
    let mut buf = [0u8; super::MANIFEST_FOOTER_SIZE];
    buf[super::MANIFEST_FOOTER_MESSAGE_COUNT_OFFSET
        ..super::MANIFEST_FOOTER_MESSAGE_COUNT_OFFSET + 8]
        .copy_from_slice(&message_count.to_le_bytes());
    buf[super::MANIFEST_FOOTER_DATA_END_OFFSET..super::MANIFEST_FOOTER_DATA_END_OFFSET + 8]
        .copy_from_slice(&data_end.to_le_bytes());
    buf[super::MANIFEST_FOOTER_FRONTIER_OFFSET..super::MANIFEST_FOOTER_FRONTIER_OFFSET + 32]
        .copy_from_slice(&frontier);
    buf[super::MANIFEST_FOOTER_CHECKSUM_OFFSET..super::MANIFEST_FOOTER_CHECKSUM_OFFSET + 8]
        .copy_from_slice(&checksum.to_le_bytes());
    buf[super::MANIFEST_FOOTER_MAGIC_OFFSET..super::MANIFEST_FOOTER_MAGIC_OFFSET + 4]
        .copy_from_slice(super::MANIFEST_MAGIC);
    buf
}
pub(crate) fn decode_record(
    buf: [u8; super::MANIFEST_RECORD_SIZE],
) -> Result<ManifestRecord, FileError> {
    let offset = u64::from_le_bytes(buf[0..8].try_into().expect("slice len 8"));
    let size = u32::from_le_bytes(buf[8..12].try_into().expect("slice len 4"));
    let reserved = u32::from_le_bytes(buf[12..16].try_into().expect("slice len 4"));
    let checksum = u64::from_le_bytes(buf[16..24].try_into().expect("slice len 8"));
    if reserved != 0 {
        return Err(FileError::InvalidReserved);
    }
    Ok(ManifestRecord {
        offset,
        size,
        checksum,
    })
}
