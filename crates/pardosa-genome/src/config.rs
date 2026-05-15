// Configuration types for encode/decode options and page classes.

// ---------------------------------------------------------------------------
// Page classes
// ---------------------------------------------------------------------------

/// Page classes define per-message element budgets.
///
/// Stored in the file header (`page_class` byte at offset 20). The reader
/// treats the stored page class as a default for `max_total_elements` and
/// may override it with caller-supplied limits.
///
/// Formula: `256 × 16^N` where N is the page class number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PageClass {
    /// 256 elements — small config.
    Page0 = 0,
    /// 4,096 elements — moderate struct.
    Page1 = 1,
    /// 65,536 elements — standard dataset.
    Page2 = 2,
    /// 1,048,576 elements — large batch.
    Page3 = 3,
}

impl PageClass {
    #[must_use]
    pub const fn max_elements(self) -> usize {
        match self {
            Self::Page0 => 256,
            Self::Page1 => 4_096,
            Self::Page2 => 65_536,
            Self::Page3 => 1_048_576,
        }
    }

    /// Parse from the raw byte in the file header. Returns `None` for
    /// unknown values.
    #[must_use]
    pub const fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Page0),
            1 => Some(Self::Page1),
            2 => Some(Self::Page2),
            3 => Some(Self::Page3),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Compression
// ---------------------------------------------------------------------------

/// Compression algorithm selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    /// No compression.
    #[default]
    None,
    /// Zstd compression at the given level (1–22). Default: 3.
    #[cfg(feature = "zstd")]
    Zstd { level: i32 },
}

// ---------------------------------------------------------------------------
// Encode options
// ---------------------------------------------------------------------------

/// Options for encoding.
#[derive(Debug, Clone)]
pub struct EncodeOptions {
    /// Compression algorithm. Default: `None`.
    pub compression: Compression,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            compression: Compression::None,
        }
    }
}

// ---------------------------------------------------------------------------
// Decode options
// ---------------------------------------------------------------------------

/// Options for decoding. Controls resource limits to prevent denial-of-service
/// via crafted inputs.
#[derive(Debug, Clone)]
pub struct DecodeOptions {
    /// Maximum nesting depth for recursive types.
    /// Default: 128.
    pub max_depth: usize,

    /// Maximum total elements across all sequences and maps.
    /// Default: `PageClass::Page0` (256).
    pub max_total_elements: usize,

    /// Maximum uncompressed message size in bytes.
    /// Default: 256 MiB.
    pub max_uncompressed_size: usize,

    /// Maximum bare message size in bytes.
    /// Default: 256 MiB.
    pub max_message_size: usize,

    /// Maximum zstd window log. Default: 22 (4 MiB).
    pub max_zstd_window_log: u32,

    /// Reject buffers with trailing bytes after the message. Default: true.
    pub reject_trailing_bytes: bool,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            max_depth: 128,
            max_total_elements: PageClass::Page0.max_elements(),
            max_uncompressed_size: 268_435_456,
            max_message_size: 268_435_456,
            max_zstd_window_log: 22,
            reject_trailing_bytes: true,
        }
    }
}

impl DecodeOptions {
    /// Create options appropriate for the given page class.
    #[must_use]
    pub fn for_page_class(page: PageClass) -> Self {
        Self {
            max_total_elements: page.max_elements(),
            ..Self::default()
        }
    }
}
