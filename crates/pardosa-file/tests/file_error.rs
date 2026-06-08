//! H2 — `FileError` hygiene tests.
//!
//! Pins ADR-0007 compliance for `pardosa_file::FileError`:
//! `#[non_exhaustive]`, `source()` chain walks to inner `io::Error`,
//! and `From<std::io::Error>` exists.
use pardosa_file::FileError;
use std::error::Error;
use std::io;
#[test]
fn file_error_io_source_walks_to_inner_io_error() {
    let inner = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
    let err: FileError = FileError::Io(inner);
    let src = err
        .source()
        .expect("FileError::Io must expose its inner std::io::Error as source()");
    let downcast = src
        .downcast_ref::<io::Error>()
        .expect("source must downcast to std::io::Error");
    assert_eq!(downcast.kind(), io::ErrorKind::PermissionDenied);
}
#[test]
fn file_error_non_io_variants_have_no_source() {
    assert!(FileError::InvalidMagic.source().is_none());
    assert!(FileError::UnsupportedVersion(99).source().is_none());
    assert!(FileError::InvalidChecksum.source().is_none());
}
#[test]
fn file_error_from_io_error_preserves_kind() {
    let io_err = io::Error::new(io::ErrorKind::NotFound, "missing");
    let fe: FileError = io_err.into();
    match fe {
        FileError::Io(inner) => assert_eq!(inner.kind(), io::ErrorKind::NotFound),
        other => panic!("From<io::Error> must produce FileError::Io, got {other:?}"),
    }
}
/// qf9h.28: pin the Display surface for the compression-mismatch
/// variants so operator-facing log lines remain actionable. The exact
/// phrasing is intentionally part of the public contract per ADR-0007:
/// callers grep these messages.
#[test]
fn file_error_display_names_zstd_feature_for_compression_not_available() {
    let msg = format!("{}", FileError::CompressionNotAvailable);
    assert!(
        msg.contains("zstd"),
        "Display must name the missing `zstd` Cargo feature; got: {msg:?}"
    );
    assert!(
        msg.contains("feature"),
        "Display must mention that a Cargo feature is missing; got: {msg:?}"
    );
}
#[test]
fn file_error_display_unsupported_compression_shows_algo_byte() {
    let msg = format!("{}", FileError::UnsupportedCompression(0xAB));
    assert!(
        msg.contains("0xAB"),
        "Display must include the offending algorithm byte in hex; got: {msg:?}"
    );
    assert!(
        msg.contains("compression"),
        "Display must mention compression; got: {msg:?}"
    );
}
