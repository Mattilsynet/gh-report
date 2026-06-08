//! Reader/Writer configuration for `.pgno` files.
//!
//! The default options preserve byte-for-byte compatibility with the
//! pre-Z1 uncompressed file format. The `zstd` Cargo feature unlocks
//! the zstd variants of [`Compression`] in [`WriterOptions`] and
//! decompression in [`Reader`](crate::Reader); without that feature,
//! only [`Compression::None`] is constructible.
//!
//! See [ADR-0006](../../docs/adr/0006-pgno-file-format.md) and the
//! Z1 mission brief for the binding constraint that only event /
//! message body bytes are compressed; native structures stay raw.
/// Payload-body compression algorithm and (where applicable)
/// level.
///
/// Wire form: low three bits of the file-header `flags` word
/// (`ALGO_NONE = 0x00`, `ALGO_ZSTD = 0x01`). Level is a
/// writer-side encoder choice; the algo byte is identical for
/// every zstd variant.
///
/// Levels: `none`, `9` (balanced), `19` (high ratio, slow).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    /// No compression (default; byte-identical to pre-Z1 output).
    #[default]
    None,
    /// Zstandard compression at level 9. Requires the `zstd` Cargo feature.
    #[cfg(feature = "zstd")]
    Zstd9,
    /// Zstandard compression at level 19. Requires the `zstd` Cargo feature.
    #[cfg(feature = "zstd")]
    Zstd19,
}
/// Writer configuration. Use [`WriterOptions::default`] for legacy
/// uncompressed output; use the builder methods to opt in to compression.
#[derive(Debug, Clone, Copy, Default)]
pub struct WriterOptions {
    pub(crate) compression: Compression,
}
impl WriterOptions {
    /// Select the payload-body compression algorithm.
    #[must_use]
    pub fn with_compression(mut self, compression: Compression) -> Self {
        self.compression = compression;
        self
    }
    /// Configured payload-body compression algorithm.
    #[must_use]
    pub fn compression(&self) -> Compression {
        self.compression
    }
}
const DEFAULT_MAX_DECOMPRESSED_MESSAGE_BYTES: usize = 1024 * 1024 * 1024;
/// W7 (roadmap correctness 2026-05-24): bound the buffer allocated
/// for the embedded schema-source string. Default 16 MiB — three
/// orders of magnitude above any plausible schema source, well below
/// the 4 GiB attacker-controlled ceiling of the raw `u32`
/// `schema_size` header field.
const DEFAULT_MAX_SCHEMA_SOURCE_BYTES: u32 = 16 * 1024 * 1024;
/// W7: bound the `Vec::with_capacity` allocated for the index based
/// on the footer-declared `message_count`. The on-disk invariant
/// already implies `message_count * 24 ≤ file_size`, but the explicit
/// cap turns "tiny header advertises billions of messages, but the
/// file is actually small" into a typed `IndexTooLarge` rejection
/// loudly, instead of an `InvalidIndex` after a large speculative
/// `Vec::with_capacity`. Default ~44.7 million messages (1 GiB of
/// 24-byte index entries, integer-divided).
const DEFAULT_MAX_MESSAGE_COUNT: u64 =
    (1024 * 1024 * 1024) / (crate::format::INDEX_ENTRY_SIZE as u64);
/// Reader configuration. The default cap on decompressed per-message
/// payload size is 1 GiB; raise or lower with
/// [`ReaderOptions::with_max_decompressed_message_bytes`].
#[derive(Debug, Clone, Copy)]
#[allow(
    clippy::struct_field_names,
    reason = "the `max_` prefix names each field as an upper-bound cap, which is the \
              defining shape of ReaderOptions; renaming would obscure intent."
)]
pub struct ReaderOptions {
    pub(crate) max_decompressed_message_bytes: usize,
    pub(crate) max_schema_source_bytes: u32,
    pub(crate) max_message_count: u64,
}
impl Default for ReaderOptions {
    fn default() -> Self {
        Self {
            max_decompressed_message_bytes: DEFAULT_MAX_DECOMPRESSED_MESSAGE_BYTES,
            max_schema_source_bytes: DEFAULT_MAX_SCHEMA_SOURCE_BYTES,
            max_message_count: DEFAULT_MAX_MESSAGE_COUNT,
        }
    }
}
impl ReaderOptions {
    /// Override the decompressed per-message payload cap.
    ///
    /// The cap is enforced for compressed files only; uncompressed files
    /// are bounded by their index entry sizes and need no separate cap.
    #[must_use]
    pub fn with_max_decompressed_message_bytes(mut self, cap: usize) -> Self {
        self.max_decompressed_message_bytes = cap;
        self
    }
    /// Currently configured decompressed-payload cap, in bytes.
    #[must_use]
    pub fn max_decompressed_message_bytes(&self) -> usize {
        self.max_decompressed_message_bytes
    }
    /// W7: override the cap on the embedded schema-source string size
    /// read from the file header. A larger header field surfaces as
    /// [`FileError::SchemaSourceTooLarge`](crate::FileError::SchemaSourceTooLarge)
    /// **before** the buffer is allocated.
    #[must_use]
    pub fn with_max_schema_source_bytes(mut self, cap: u32) -> Self {
        self.max_schema_source_bytes = cap;
        self
    }
    /// Currently configured schema-source cap, in bytes.
    #[must_use]
    pub fn max_schema_source_bytes(&self) -> u32 {
        self.max_schema_source_bytes
    }
    /// W7: override the cap on the footer-declared `message_count`. A
    /// larger value surfaces as
    /// [`FileError::IndexTooLarge`](crate::FileError::IndexTooLarge)
    /// **before** the index buffer is allocated.
    #[must_use]
    pub fn with_max_message_count(mut self, cap: u64) -> Self {
        self.max_message_count = cap;
        self
    }
    /// Currently configured message-count cap.
    #[must_use]
    pub fn max_message_count(&self) -> u64 {
        self.max_message_count
    }
}
