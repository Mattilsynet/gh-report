pub const MAGIC: [u8; 4] = *b"PGNO";
pub const FORMAT_VERSION: u16 = 6;
pub const FILE_HEADER_SIZE: usize = 40;
pub const FILE_FOOTER_SIZE: usize = 32;
pub const INDEX_ENTRY_SIZE: usize = 24;
pub const HEADER_MAGIC_OFFSET: usize = 0;
pub const HEADER_VERSION_OFFSET: usize = 4;
pub const HEADER_FLAGS_OFFSET: usize = 6;
pub const HEADER_SCHEMA_HASH_OFFSET: usize = 8;
pub const HEADER_SCHEMA_HASH_LEN: usize = 16;
pub const HEADER_DICT_ID_OFFSET: usize = 24;
pub const HEADER_PAGE_CLASS_OFFSET: usize = 28;
pub const HEADER_SCHEMA_SIZE_OFFSET: usize = 29;
pub const HEADER_RESERVED_OFFSET: usize = 33;
pub const HEADER_RESERVED_LEN: usize = 7;
pub const FOOTER_INDEX_OFFSET: usize = 0;
pub const FOOTER_MESSAGE_COUNT_OFFSET: usize = 8;
pub const FOOTER_RESERVED_OFFSET: usize = 16;
pub const FOOTER_RESERVED_LEN: usize = 4;
pub const FOOTER_MAGIC_OFFSET: usize = 20;
pub const FOOTER_CHECKSUM_OFFSET: usize = 24;
pub const BARE_HEADER_SIZE: usize = 2 + HEADER_SCHEMA_HASH_LEN + 1 + 4;
pub const BARE_HEADER_COMPRESSED_SIZE: usize = BARE_HEADER_SIZE + 4;
const _: () = assert!(
    BARE_HEADER_SIZE == 23,
    "BARE_HEADER_SIZE drifted from v2 wire value (23). \
     Update FORMAT_VERSION and downstream readers, then update this assert."
);
const _: () = assert!(
    BARE_HEADER_COMPRESSED_SIZE == 27,
    "BARE_HEADER_COMPRESSED_SIZE drifted from v2 wire value (27). \
     Update FORMAT_VERSION and downstream readers, then update this assert."
);
pub const ALGO_NONE: u8 = 0x00;
pub const ALGO_ZSTD: u8 = 0x01;
pub const NONE_SENTINEL: u32 = 0xFFFF_FFFF;
/// PGN-0021 R3/R5: presence discriminant for the v6 `adopter_epoch`
/// region, carried as bit 3 of header `flags` — distinct from
/// length, so `None` (bit clear) and `Some(&[])` (bit set, zero-length
/// region) are byte-distinguishable on disk.
pub const HEADER_EPOCH_PRESENT_FLAG: u16 = 0b1000;
/// Mask of header `flags` bits defined by `FORMAT_VERSION = 6`: the
/// low three compression-algorithm bits plus the epoch-presence bit.
/// Any other bit set is a reserved-flag violation.
pub const HEADER_FLAGS_KNOWN_MASK: u16 = 0b111 | HEADER_EPOCH_PRESENT_FLAG;
/// Byte width of the `adopter_epoch` region's in-band `u32` length
/// prefix. Unlike `schema_source`, the epoch length is not a fixed
/// header field (the 40-byte header's reserved bytes stay
/// zero-asserted per PGN-0021 R5) — it is carried inside the region
/// itself, immediately following `schema_source`.
pub const EPOCH_LEN_PREFIX_SIZE: usize = 4;
#[must_use]
pub const fn pad_to_8(size: usize) -> usize {
    (size + 7) & !7
}
#[must_use]
pub const fn messages_offset(schema_size: u32) -> usize {
    FILE_HEADER_SIZE + pad_to_8(schema_size as usize)
}
/// Byte offset of the `adopter_epoch` region, immediately after the
/// (padded) `schema_source` region and before the message region when
/// no epoch is present. PGN-0021 R5.
#[must_use]
pub const fn epoch_region_offset(schema_size: u32) -> usize {
    messages_offset(schema_size)
}
/// On-disk size of the `adopter_epoch` region: zero when absent
/// (`None`, presence bit clear, no bytes written); otherwise the
/// `u32` length prefix plus the epoch bytes, padded to an 8-byte
/// boundary (PGN-0021 R3/R5).
#[must_use]
pub const fn epoch_region_size(epoch_len: Option<u32>) -> usize {
    match epoch_len {
        None => 0,
        Some(len) => pad_to_8(EPOCH_LEN_PREFIX_SIZE + len as usize),
    }
}
/// Byte offset of the message region for a `FORMAT_VERSION = 6`
/// container, accounting for both the `schema_source` region and the
/// (possibly absent) `adopter_epoch` region. Equal to
/// [`messages_offset`] when `epoch_len` is `None` (PGN-0021 R8: an
/// absent epoch reproduces the pre-OSF byte layout exactly).
#[must_use]
pub const fn messages_offset_with_epoch(schema_size: u32, epoch_len: Option<u32>) -> usize {
    epoch_region_offset(schema_size) + epoch_region_size(epoch_len)
}
/// PGN-0021 R4: byte-for-byte opaque equality over `adopter_epoch`
/// values. `None` compares unequal to every `Some(_)` including
/// `Some(&[])`; two `Some(_)` values compare equal only on exact byte
/// identity. No case-folding, normalization, or trimming — pardosa
/// never interprets the bytes, it only stores and memcmps them.
#[must_use]
pub fn epoch_bytes_eq(a: Option<&[u8]>, b: Option<&[u8]>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}
pub const MIN_FILE_SIZE: usize = FILE_HEADER_SIZE + FILE_FOOTER_SIZE;
