//! Regression: a buffer crafted with a *prior* `FORMAT_VERSION` must be
//! rejected by `Reader::open` with `FileError::UnsupportedVersion(prior)`,
//! NOT silently accepted and NOT panic. Guards ADR-0006 §1: any
//! wire-format change bumps `FORMAT_VERSION` and old files surface a typed
//! rejection.
//!
//! The prior version is hard-coded (`3`) rather than derived: the whole
//! point is to pin the bump. When the next break lands, add a new entry
//! to `PRIOR_VERSIONS` rather than mutating the existing one.
use pardosa_file::format::{
    FILE_FOOTER_SIZE, FILE_HEADER_SIZE, FORMAT_VERSION, HEADER_MAGIC_OFFSET, HEADER_VERSION_OFFSET,
    MAGIC,
};
use pardosa_file::{FileError, Reader};
use std::io::Cursor;
/// Every `FORMAT_VERSION` value that has ever shipped from this crate and
/// is now retired. `Reader::open` must reject each one as
/// `UnsupportedVersion(prior)`.
const PRIOR_VERSIONS: &[u16] = &[3];
#[test]
fn current_format_version_differs_from_all_prior() {
    for &prior in PRIOR_VERSIONS {
        assert_ne!(
            FORMAT_VERSION, prior,
            "FORMAT_VERSION must differ from every retired version; collision with {prior}",
        );
    }
}
#[test]
fn old_format_version_rejected_with_typed_error() {
    for &prior in PRIOR_VERSIONS {
        let mut buf = vec![0u8; FILE_HEADER_SIZE + FILE_FOOTER_SIZE];
        buf[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
        buf[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2].copy_from_slice(&prior.to_le_bytes());
        let result = Reader::open(Cursor::new(buf));
        match result {
            Err(FileError::UnsupportedVersion(v)) => {
                assert_eq!(
                    v, prior,
                    "expected UnsupportedVersion({prior}), got UnsupportedVersion({v})",
                );
            }
            Err(other) => {
                panic!("expected UnsupportedVersion({prior}), got different typed error: {other:?}")
            }
            Ok(_) => {
                panic!(
                    "old-format buffer (version {prior}) was silently accepted; \
                 ADR-0006 requires typed rejection",
                )
            }
        }
    }
}
