pub const MAGIC: [u8; 4] = *b"PGNO";
pub const FORMAT_VERSION: u16 = 5;
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
#[must_use]
pub const fn pad_to_8(size: usize) -> usize {
    (size + 7) & !7
}
#[must_use]
pub const fn messages_offset(schema_size: u32) -> usize {
    FILE_HEADER_SIZE + pad_to_8(schema_size as usize)
}
pub const MIN_FILE_SIZE: usize = FILE_HEADER_SIZE + FILE_FOOTER_SIZE;
