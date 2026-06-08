use crate::dragline::Line;
use crate::persist::{Error, ValidatedReplayError, rehydrate_unchecked, rehydrate_validated};
use pardosa_schema::GenomeSafe;
use pardosa_wire::{Decode, Validate};
use std::io::Cursor;
/// Rebuild a [`Line<T>`] from a `.pgno`-encoded byte slice
/// (ADR-0022 §D2; ADR-0020 reader bound).
///
/// Delegates to [`crate::persist::rehydrate_unchecked`] over a
/// [`std::io::Cursor`] — framing, schema-hash, and contiguity
/// checks match the `.pgno`/`File` open path. No filesystem
/// access. Per-event precursor-hash and payload [`Validate`]
/// checks live on [`from_pgno_bytes_validated`].
///
/// # Errors
///
/// Propagates [`crate::persist::Error`] verbatim.
pub(crate) fn from_pgno_bytes_unchecked<T>(bytes: &[u8]) -> Result<Line<T>, Error>
where
    T: Decode + GenomeSafe,
{
    let source = Cursor::new(bytes);
    rehydrate_unchecked::<T, _>(source)
}
/// Validated counterpart to [`from_pgno_bytes_unchecked`]
/// (ADR-0020 reader bound + payload [`Validate`]).
///
/// Same byte-only contract; adds per-event envelope-shape and
/// payload [`Validate::validate`] checks. Prefer this when
/// foreign-payload [`Decode`] impls may produce domain-invalid
/// `T`.
///
/// # Errors
///
/// Returns [`ValidatedReplayError`] for any per-event failure
/// or container-header error.
#[allow(dead_code)]
pub(crate) fn from_pgno_bytes_validated<T>(
    bytes: &[u8],
) -> Result<Line<T>, ValidatedReplayError<<T as Validate>::Error>>
where
    T: Decode + GenomeSafe + Validate,
{
    let source = Cursor::new(bytes);
    rehydrate_validated::<T, _>(source)
}
