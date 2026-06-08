use super::checked::{CheckedEventStream, stream_checked};
use super::error::{Error, ValidatedReplayError};
use crate::{Event, EventId};
use pardosa_schema::GenomeSafe;
use pardosa_wire::{Decode, Validate};
use std::io::{Read, Seek};
use std::iter::FusedIterator;
/// Iterator yielded by [`stream_validated`] (o1ix.6, roadmap correctness 6).
///
/// Wraps a [`CheckedEventStream`] and applies, per yielded event:
///   1. `event.validate_envelope()` (per-envelope shape, o1ix.5/.15).
///   2. `event.domain_event().validate()` (open `Validate` impl on `T`).
///
/// The wrapped checked stream still performs the cross-event structural
/// checks first; only events that pass those reach the per-envelope and
/// payload checks. Resume semantics, container-header checks, and
/// poison-on-error behaviour are inherited from [`CheckedEventStream`].
#[derive(Debug)]
pub struct ValidatedEventStream<R: Read + Seek, T> {
    pub(super) inner: CheckedEventStream<R, T>,
    poisoned: bool,
}
impl<R: Read + Seek, T> Iterator for ValidatedEventStream<R, T>
where
    T: Decode + GenomeSafe + Validate,
{
    type Item = Result<Event<T>, ValidatedReplayError<<T as Validate>::Error>>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.poisoned {
            return None;
        }
        match self.inner.next()? {
            Err(e) => {
                self.poisoned = true;
                Some(Err(ValidatedReplayError::Replay(e)))
            }
            Ok(event) => {
                if let Err(env_err) = event.validate_envelope() {
                    self.poisoned = true;
                    return Some(Err(ValidatedReplayError::Envelope(env_err)));
                }
                if let Err(payload_err) = event.domain_event().validate() {
                    self.poisoned = true;
                    return Some(Err(ValidatedReplayError::Payload(payload_err)));
                }
                Some(Ok(event))
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}
impl<R: Read + Seek, T> FusedIterator for ValidatedEventStream<R, T> where
    T: Decode + GenomeSafe + Validate
{
}
impl<R: Read + Seek, T> ValidatedEventStream<R, T> {
    /// Consume the stream and return the underlying reader. Symmetric
    /// to [`CheckedEventStream::into_inner`].
    pub fn into_inner(self) -> R {
        self.inner.into_inner()
    }
}
/// Open a `.pgno` source and return a [`ValidatedEventStream<R, T>`]
/// (o1ix.6).
///
/// Additive: same container-header validation and exclusive
/// `resume_after` as [`stream_checked`], plus
/// [`Event::validate_envelope`] and `T::validate()` per yielded event.
/// Adopters with foreign-payload `Decode` impls that may produce
/// domain-invalid `T` should prefer this over [`stream_checked`].
///
/// # Errors
///
/// [`Error::File`] / [`Error::SchemaHashMismatch`] surface raw from
/// this function (symmetry with [`stream_checked`]); per-event errors
/// surface from the iterator as [`ValidatedReplayError`].
pub fn stream_validated<R, T>(
    source: R,
    resume_after: Option<EventId>,
) -> Result<ValidatedEventStream<R, T>, Error>
where
    R: Read + Seek,
    T: Decode + GenomeSafe + Validate,
{
    let inner = stream_checked::<R, T>(source, resume_after)?;
    Ok(ValidatedEventStream {
        inner,
        poisoned: false,
    })
}
