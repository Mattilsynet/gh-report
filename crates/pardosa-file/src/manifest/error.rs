use crate::error::FileError;
use std::io;
/// Errors surfaced by [`recover_footerless_prefix`] and
/// [`finalize_recovered_prefix`].
///
/// Each variant maps to a specific abort condition from the mission
/// brief: the recovery API must distinguish a valid synced prefix
/// from a corrupt or truncated tail. A [`RecoveryError`] return is
/// the substrate's signal that recovery is not safe to proceed
/// without operator-level remediation.
///
/// [`recover_footerless_prefix`]: super::recover_footerless_prefix
/// [`finalize_recovered_prefix`]: super::finalize_recovered_prefix
#[derive(Debug)]
#[non_exhaustive]
pub enum RecoveryError {
    /// Underlying manifest read / parse failure.
    Manifest(FileError),
    /// Underlying `.pgno` header read / parse failure.
    PgnoHeader(FileError),
    /// I/O failure on the `.pgno` or manifest source.
    Io(io::Error),
    /// Manifest claims more bytes are durable than the `.pgno`
    /// actually contains. Either the `.pgno` was truncated after
    /// the last manifest sync, or the manifest was rolled forward
    /// without a corresponding `.pgno` sync. **Not** recoverable
    /// without operator intervention.
    DataEndExceedsFile {
        /// Byte offset declared in the manifest footer.
        manifest_data_end: u64,
        /// Actual `.pgno` file length.
        pgno_len: u64,
    },
    /// Manifest schema-hash does not match the `.pgno` header
    /// schema-hash. Cross-file binding violation; the two files
    /// do not belong to the same append session.
    SchemaHashMismatch {
        /// Schema hash advertised by the manifest.
        manifest: u128,
        /// Schema hash advertised by the `.pgno` header.
        pgno: u128,
    },
    /// Manifest page-class does not match the `.pgno` header
    /// page-class. Same cross-file binding posture as
    /// [`Self::SchemaHashMismatch`].
    PageClassMismatch {
        /// Page class advertised by the manifest.
        manifest: u8,
        /// Page class advertised by the `.pgno` header.
        pgno: u8,
    },
    /// Manifest schema-size does not match the `.pgno` header
    /// schema-size. Same cross-file binding posture as
    /// [`Self::SchemaHashMismatch`].
    SchemaSizeMismatch {
        /// Schema size advertised by the manifest.
        manifest: u32,
        /// Schema size advertised by the `.pgno` header.
        pgno: u32,
    },
    /// One of the manifest records points at body bytes whose
    /// per-message xxh64 does not match the manifest record's
    /// checksum. Body corruption is **not** recoverable; the
    /// `.pgno` body region is the source of truth and a
    /// mismatching prefix is a tampering or hardware fault
    /// signal. Adopters route this as
    /// [`FileError::is_tamper_suspicious`](crate::FileError::is_tamper_suspicious).
    BodyChecksumMismatch {
        /// Index of the offending message within the manifest
        /// record set.
        message_index: u64,
        /// Byte offset within the `.pgno` of the offending body.
        offset: u64,
    },
    /// Manifest claims a body extends past `data_end`. Manifest
    /// is internally inconsistent; reject without finalising.
    BodyOverrunsDataEnd {
        message_index: u64,
        record_end: u64,
        data_end: u64,
    },
}
impl core::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Manifest(e) => write!(f, "manifest error: {e}"),
            Self::PgnoHeader(e) => write!(f, "pgno header error: {e}"),
            Self::Io(e) => write!(f, "i/o error: {e}"),
            Self::DataEndExceedsFile {
                manifest_data_end,
                pgno_len,
            } => {
                write!(
                    f,
                    "manifest declares data_end {manifest_data_end} but .pgno is only {pgno_len} bytes",
                )
            }
            Self::SchemaHashMismatch { manifest, pgno } => {
                write!(
                    f,
                    "manifest schema_hash {manifest:032x} does not match .pgno header schema_hash {pgno:032x}",
                )
            }
            Self::PageClassMismatch { manifest, pgno } => {
                write!(
                    f,
                    "manifest page_class {manifest} does not match .pgno header page_class {pgno}",
                )
            }
            Self::SchemaSizeMismatch { manifest, pgno } => {
                write!(
                    f,
                    "manifest schema_size {manifest} does not match .pgno header schema_size {pgno}",
                )
            }
            Self::BodyChecksumMismatch {
                message_index,
                offset,
            } => {
                write!(
                    f,
                    "per-message xxh64 mismatch at message_index={message_index} offset={offset}",
                )
            }
            Self::BodyOverrunsDataEnd {
                message_index,
                record_end,
                data_end,
            } => {
                write!(
                    f,
                    "manifest record {message_index} ends at {record_end} but data_end is {data_end}",
                )
            }
        }
    }
}
impl core::error::Error for RecoveryError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Manifest(e) | Self::PgnoHeader(e) => Some(e),
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}
impl From<io::Error> for RecoveryError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
