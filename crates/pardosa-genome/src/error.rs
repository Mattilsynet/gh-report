/// Error types for pardosa-genome serialization, deserialization, and file operations.
use core::fmt;

// ---------------------------------------------------------------------------
// Serialization errors
// ---------------------------------------------------------------------------

/// Errors during serialization.
#[derive(Debug)]
pub enum SerError {
    /// Serialized message exceeds `u32::MAX` bytes.
    MessageTooLarge,
    /// Unsupported serde attribute detected at runtime (defense-in-depth).
    UnsupportedAttribute(&'static str),
    /// `SizingSerializer` and `WritingSerializer` produced different byte counts.
    InternalSizingMismatch { expected: usize, actual: usize },
    /// Compression failed.
    CompressionFailed,
    /// serde `ser::Error::custom()`.
    Custom(SerMessage),
}

impl fmt::Display for SerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MessageTooLarge => write!(f, "serialized message exceeds 4 GiB"),
            Self::UnsupportedAttribute(attr) => {
                write!(f, "unsupported serde attribute: {attr}")
            }
            Self::InternalSizingMismatch { expected, actual } => {
                write!(
                    f,
                    "internal sizing mismatch: expected {expected} bytes, wrote {actual}"
                )
            }
            Self::CompressionFailed => write!(f, "compression failed"),
            Self::Custom(msg) => write!(f, "{msg}"),
        }
    }
}

impl serde::ser::Error for SerError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self::Custom(SerMessage::from_display(msg))
    }
}

impl std::error::Error for SerError {}

// ---------------------------------------------------------------------------
// Deserialization errors
// ---------------------------------------------------------------------------

/// Errors during deserialization.
#[derive(Debug)]
pub enum DeError {
    /// Buffer shorter than expected.
    BufferTooSmall,
    /// Offset points past buffer end.
    OffsetOutOfBounds { offset: u32, buf_len: usize },
    /// offset + len overflows.
    OffsetOverflow,
    /// String data is not valid UTF-8.
    InvalidUtf8,
    /// Char value is not a valid Unicode scalar.
    InvalidChar(u32),
    /// Bool value is not 0x00 or 0x01.
    InvalidBool(u8),
    /// Enum discriminant exceeds variant count.
    InvalidDiscriminant(u32),
    /// Owning type requested in core-only mode.
    AllocRequired,
    /// Nesting depth exceeds limit.
    DepthLimitExceeded,
    /// Total elements exceed page class limit.
    ElementLimitExceeded,
    /// Buffer has trailing bytes.
    TrailingBytes { expected: usize, actual: usize },
    /// Format version not supported.
    VersionMismatch { expected: u16, actual: u16 },
    /// Schema hash mismatch.
    SchemaMismatch { expected: u64, actual: u64 },
    /// Padding byte is not 0x00.
    NonZeroPadding { offset: usize },
    /// Offset points into inline region.
    BackwardOffset { offset: u32 },
    /// Per-message checksum mismatch.
    ChecksumMismatch,
    /// Decompression failed.
    DecompressionFailed,
    /// Uncompressed size exceeds limit.
    UncompressedSizeTooLarge(u32),
    /// Bare message size exceeds limit.
    MessageTooLarge(u32),
    /// Trailing bytes after decompression.
    PostDecompressionTrailingBytes,
    /// serde `de::Error::custom()`.
    Custom(SerMessage),
}

impl fmt::Display for DeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BufferTooSmall => write!(f, "buffer too small"),
            Self::OffsetOutOfBounds { offset, buf_len } => {
                write!(f, "offset {offset} out of bounds (buf_len={buf_len})")
            }
            Self::OffsetOverflow => write!(f, "offset + length overflows"),
            Self::InvalidUtf8 => write!(f, "invalid UTF-8 in string data"),
            Self::InvalidChar(v) => write!(f, "invalid char value: 0x{v:08X}"),
            Self::InvalidBool(v) => write!(f, "invalid bool value: 0x{v:02X}"),
            Self::InvalidDiscriminant(d) => write!(f, "invalid enum discriminant: {d}"),
            Self::AllocRequired => {
                write!(f, "owning type requires alloc feature")
            }
            Self::DepthLimitExceeded => write!(f, "nesting depth limit exceeded"),
            Self::ElementLimitExceeded => write!(f, "total element limit exceeded"),
            Self::TrailingBytes { expected, actual } => {
                write!(f, "trailing bytes: expected {expected}, got {actual}")
            }
            Self::VersionMismatch { expected, actual } => {
                write!(f, "version mismatch: expected {expected}, got {actual}")
            }
            Self::SchemaMismatch { expected, actual } => {
                write!(
                    f,
                    "schema mismatch: expected 0x{expected:016X}, got 0x{actual:016X}"
                )
            }
            Self::NonZeroPadding { offset } => {
                write!(f, "non-zero padding byte at offset {offset}")
            }
            Self::BackwardOffset { offset } => {
                write!(f, "backward offset: {offset}")
            }
            Self::ChecksumMismatch => write!(f, "checksum mismatch"),
            Self::DecompressionFailed => write!(f, "decompression failed"),
            Self::UncompressedSizeTooLarge(size) => {
                write!(f, "uncompressed size too large: {size}")
            }
            Self::MessageTooLarge(size) => {
                write!(f, "message too large: {size}")
            }
            Self::PostDecompressionTrailingBytes => {
                write!(f, "trailing bytes after decompression")
            }
            Self::Custom(msg) => write!(f, "{msg}"),
        }
    }
}

impl serde::de::Error for DeError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self::Custom(SerMessage::from_display(msg))
    }
}

impl std::error::Error for DeError {}

// ---------------------------------------------------------------------------
// File-level errors
// ---------------------------------------------------------------------------

/// Errors during file-level operations.
#[derive(Debug)]
pub enum FileError {
    /// Header or footer magic is not "PGNO".
    InvalidMagic,
    /// Format version not supported.
    UnsupportedVersion(u16),
    /// Unknown compression algorithm in header flags.
    UnsupportedCompression(u8),
    /// Footer checksum mismatch.
    InvalidChecksum,
    /// Index offset or entry is inconsistent.
    InvalidIndex,
    /// File uses compression but the feature is not enabled.
    CompressionNotAvailable,
    /// Error in a specific message.
    MessageError(u64, DeError),
    /// Embedded schema source is not valid UTF-8.
    InvalidSchemaSource,
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMagic => write!(f, "invalid magic bytes"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported format version: {v}"),
            Self::UnsupportedCompression(algo) => {
                write!(f, "unsupported compression algorithm: 0x{algo:02X}")
            }
            Self::InvalidChecksum => write!(f, "footer checksum mismatch"),
            Self::InvalidIndex => write!(f, "invalid message index"),
            Self::CompressionNotAvailable => {
                write!(f, "compression feature not enabled")
            }
            Self::MessageError(idx, err) => {
                write!(f, "message {idx}: {err}")
            }
            Self::InvalidSchemaSource => {
                write!(f, "embedded schema source is not valid UTF-8")
            }
        }
    }
}

impl std::error::Error for FileError {}

// ---------------------------------------------------------------------------
// Error message storage
// ---------------------------------------------------------------------------

/// Error message storage.
#[derive(Debug)]
pub struct SerMessage {
    inner: String,
}

impl SerMessage {
    fn from_display(msg: impl fmt::Display) -> Self {
        Self {
            inner: format!("{msg}"),
        }
    }
}

impl fmt::Display for SerMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}
