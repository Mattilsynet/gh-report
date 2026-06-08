use core::fmt;
/// Public error type surfaced by `pardosa-file` operations.
///
/// ADR-0007 compliant: `#[non_exhaustive]`, stable `Display`, and a
/// `source()` chain that exposes the underlying `std::io::Error` for the
/// `Io` variant. Additive variants are non-breaking.
#[derive(Debug)]
#[non_exhaustive]
pub enum FileError {
    InvalidMagic,
    UnsupportedVersion(u16),
    UnsupportedCompression(u8),
    InvalidChecksum,
    ChecksumMismatch(u64),
    InvalidIndex,
    /// File header advertises a compression algorithm whose decoder is not
    /// linked into this build (typically `ALGO_ZSTD` without the `zstd`
    /// Cargo feature enabled).
    ///
    /// Remediation: rebuild the consuming crate with the corresponding
    /// Cargo feature enabled — for the zstd algorithm, depend on
    /// `pardosa-file` with `features = ["zstd"]` (or build the workspace
    /// with `--features pardosa-file/zstd`). This is **not** a corruption
    /// signal — see [`FileError::is_tamper_suspicious`].
    CompressionNotAvailable,
    InvalidSchemaSource,
    InvalidReserved,
    IndexOverflow,
    /// Decompressed payload would exceed the reader's configured cap.
    /// `limit` is the cap that was exceeded
    /// (see [`ReaderOptions::with_max_decompressed_message_bytes`](crate::ReaderOptions::with_max_decompressed_message_bytes)).
    DecompressedTooLarge {
        limit: usize,
    },
    /// W7 (roadmap correctness 2026-05-24): the header-declared
    /// `schema_size` exceeds the reader's configured cap. Rejected
    /// **before** the schema-source buffer is allocated, so a hostile
    /// header cannot drive a multi-GiB allocation. `claimed` is the
    /// raw `u32` from the header; `limit` is the configured cap
    /// (see [`ReaderOptions::with_max_schema_source_bytes`](crate::ReaderOptions::with_max_schema_source_bytes)).
    SchemaSourceTooLarge {
        claimed: u32,
        limit: u32,
    },
    /// W7: the footer-declared `message_count` exceeds the reader's
    /// configured cap. Rejected **before** the index `Vec` is
    /// allocated. `claimed` is the raw `u64` from the footer;
    /// `limit` is the configured cap
    /// (see [`ReaderOptions::with_max_message_count`](crate::ReaderOptions::with_max_message_count)).
    /// Default cap is ~44.7M entries (1 GiB / 24-byte entry).
    IndexTooLarge {
        claimed: u64,
        limit: u64,
    },
    Io(std::io::Error),
}
impl FileError {
    /// Whether on-disk bytes appear tampered or corrupted (vs
    /// schema/decode/feature mismatch).
    ///
    /// W8 (2026-05-24): adopters escalate without re-matching
    /// the taxonomy. Best-effort — `xxh64` is not a MAC.
    /// Surfaces *evidence* of corruption, not *intent*.
    /// ADR-0006 §D4.
    ///
    /// Returns `true` for: `InvalidChecksum`,
    /// `ChecksumMismatch`, `InvalidMagic`, `InvalidReserved`,
    /// `InvalidIndex`.
    ///
    /// Returns `false` for: `UnsupportedVersion`,
    /// `UnsupportedCompression`, `CompressionNotAvailable`,
    /// `InvalidSchemaSource`, `IndexOverflow`,
    /// `SchemaSourceTooLarge`, `IndexTooLarge`,
    /// `DecompressedTooLarge`, [`FileError::Io`].
    #[must_use]
    pub fn is_tamper_suspicious(&self) -> bool {
        matches!(
            self,
            Self::InvalidChecksum
                | Self::ChecksumMismatch(_)
                | Self::InvalidMagic
                | Self::InvalidReserved
                | Self::InvalidIndex
        )
    }
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
            Self::ChecksumMismatch(idx) => {
                write!(f, "per-message checksum mismatch at index {idx}")
            }
            Self::InvalidIndex => write!(f, "invalid message index"),
            Self::CompressionNotAvailable => {
                write!(
                    f,
                    "file declares zstd compression but this build was compiled without \
                 the `zstd` Cargo feature; rebuild `pardosa-file` with \
                 `features = [\"zstd\"]` to read it"
                )
            }
            Self::InvalidSchemaSource => {
                write!(f, "embedded schema source is not valid UTF-8")
            }
            Self::InvalidReserved => write!(f, "reserved bytes must be zero"),
            Self::IndexOverflow => {
                write!(f, "message_count × INDEX_ENTRY_SIZE overflows u64")
            }
            Self::DecompressedTooLarge { limit } => {
                write!(f, "decompressed payload exceeds cap of {limit} bytes")
            }
            Self::SchemaSourceTooLarge { claimed, limit } => {
                write!(
                    f,
                    "header-declared schema_size {claimed} exceeds cap of {limit} bytes"
                )
            }
            Self::IndexTooLarge { claimed, limit } => {
                write!(
                    f,
                    "footer-declared message_count {claimed} exceeds cap of {limit}"
                )
            }
            Self::Io(err) => write!(f, "i/o error: {err}"),
        }
    }
}
impl core::error::Error for FileError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}
impl From<std::io::Error> for FileError {
    fn from(err: std::io::Error) -> Self {
        FileError::Io(err)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    /// W8 (o1ix.18): pin the tamper/corruption taxonomy so adopters
    /// routing on `is_tamper_suspicious` get a stable surface.
    #[test]
    fn tamper_suspicious_classification_matches_taxonomy() {
        assert!(FileError::InvalidChecksum.is_tamper_suspicious());
        assert!(FileError::ChecksumMismatch(7).is_tamper_suspicious());
        assert!(FileError::InvalidMagic.is_tamper_suspicious());
        assert!(FileError::InvalidReserved.is_tamper_suspicious());
        assert!(FileError::InvalidIndex.is_tamper_suspicious());
        assert!(!FileError::UnsupportedVersion(99).is_tamper_suspicious());
        assert!(!FileError::UnsupportedCompression(0xFF).is_tamper_suspicious());
        assert!(!FileError::CompressionNotAvailable.is_tamper_suspicious());
        assert!(!FileError::InvalidSchemaSource.is_tamper_suspicious());
        assert!(!FileError::IndexOverflow.is_tamper_suspicious());
        assert!(!FileError::DecompressedTooLarge { limit: 1 << 20 }.is_tamper_suspicious());
        assert!(
            !FileError::SchemaSourceTooLarge {
                claimed: 1 << 20,
                limit: 1 << 16
            }
            .is_tamper_suspicious()
        );
        assert!(
            !FileError::IndexTooLarge {
                claimed: 1 << 30,
                limit: 1 << 20
            }
            .is_tamper_suspicious()
        );
        assert!(!FileError::Io(std::io::Error::other("disk gone")).is_tamper_suspicious());
    }
}
