//! Binary format constants and header layout.
//!
//! All multi-byte fields are little-endian. The endianness contract for
//! `schema_hash` (bytes 8..24) is mechanically pinned by
//! `tests/header_wire_contract.rs`; a future header writer that uses
//! `to_be_bytes` or `to_ne_bytes` for this field fails that test.

/// File magic bytes: ASCII "PGNO" (0x50 0x47 0x4E 0x4F).
pub const MAGIC: [u8; 4] = *b"PGNO";

/// Current format version. First 2 bytes of any bare message, bytes 4-5 of
/// any file. Position and encoding will never change across versions.
///
/// v2 widens `schema_hash` from u64 (xxh64) to u128 (xxh3-128) per GEN-0035;
/// header layout grows from 32 to 40 bytes and downstream offsets shift +8.
pub const FORMAT_VERSION: u16 = 2;

// ---------------------------------------------------------------------------
// File header layout (40 bytes; v2)
// ---------------------------------------------------------------------------
//
// Offset  Size  Field           Description
// ──────  ────  ──────────────  ──────────────────────────────────────────
//  0      4     magic           ASCII "PGNO"
//  4      2     format_version  Format version (v2)
//  6      2     flags           compression_algo (bits 0-2), reserved (3-15)
//  8     16     schema_hash     Compile-time schema fingerprint (u128 LE, xxh3-128)
// 24      4     dict_id         Reserved for zstd dictionaries; must be 0 in v2
// 28      1     page_class      Page class hint (0–3)
// 29      4     schema_size     Byte count of embedded schema source (u32 LE)
// 33      7     reserved        Must be all zeros
//
// When schema_size > 0, a schema block of that many UTF-8 bytes follows the
// header at offset 40. The block is padded to an 8-byte boundary with zeros.
// Messages begin at offset 40 + padded(schema_size).

/// File header size in bytes.
pub const FILE_HEADER_SIZE: usize = 40;

/// File footer size in bytes.
pub const FILE_FOOTER_SIZE: usize = 32;

/// Message index entry size in bytes (offset:u64, size:u32, reserved:u32, checksum:u64).
pub const INDEX_ENTRY_SIZE: usize = 24;

// Header field offsets
pub const HEADER_MAGIC_OFFSET: usize = 0;
pub const HEADER_VERSION_OFFSET: usize = 4;
pub const HEADER_FLAGS_OFFSET: usize = 6;
pub const HEADER_SCHEMA_HASH_OFFSET: usize = 8;
/// Width of `schema_hash` field, in bytes. v2: u128 → 16 bytes.
pub const HEADER_SCHEMA_HASH_LEN: usize = 16;
pub const HEADER_DICT_ID_OFFSET: usize = 24;
pub const HEADER_PAGE_CLASS_OFFSET: usize = 28;
pub const HEADER_SCHEMA_SIZE_OFFSET: usize = 29;
pub const HEADER_RESERVED_OFFSET: usize = 33;
pub const HEADER_RESERVED_LEN: usize = 7;

// Footer field offsets
pub const FOOTER_INDEX_OFFSET: usize = 0;
pub const FOOTER_MESSAGE_COUNT_OFFSET: usize = 8;
pub const FOOTER_RESERVED_OFFSET: usize = 16;
pub const FOOTER_RESERVED_LEN: usize = 4;
pub const FOOTER_MAGIC_OFFSET: usize = 20;
pub const FOOTER_CHECKSUM_OFFSET: usize = 24;

// ---------------------------------------------------------------------------
// Bare message header
// ---------------------------------------------------------------------------
//
// Uncompressed (23 bytes; v2):
//   [format_version:u16][schema_hash:u128][algo:u8][msg_data_size:u32][data...]
//
// Compressed (27 bytes; v2):
//   [format_version:u16][schema_hash:u128][algo:u8][compressed_size:u32][msg_data_size:u32][data...]

/// Bare message header size (uncompressed).
pub const BARE_HEADER_SIZE: usize = 23;

/// Bare message header size (compressed).
pub const BARE_HEADER_COMPRESSED_SIZE: usize = 27;

// ---------------------------------------------------------------------------
// Compression algorithm codes
// ---------------------------------------------------------------------------

/// No compression.
pub const ALGO_NONE: u8 = 0x00;

/// Zstd compression.
pub const ALGO_ZSTD: u8 = 0x01;

// ---------------------------------------------------------------------------
// Sentinel values
// ---------------------------------------------------------------------------

/// `Option::None` offset sentinel.
pub const NONE_SENTINEL: u32 = 0xFFFF_FFFF;

// ---------------------------------------------------------------------------
// Schema block helpers
// ---------------------------------------------------------------------------

/// Round a byte count up to the next 8-byte boundary.
#[must_use]
pub const fn pad_to_8(size: usize) -> usize {
    (size + 7) & !7
}

/// Compute the offset where messages begin in a file, given the embedded
/// schema source size.
#[must_use]
pub const fn messages_offset(schema_size: u32) -> usize {
    FILE_HEADER_SIZE + pad_to_8(schema_size as usize)
}

/// Minimum valid file size: header + footer, no schema, no messages.
pub const MIN_FILE_SIZE: usize = FILE_HEADER_SIZE + FILE_FOOTER_SIZE;
