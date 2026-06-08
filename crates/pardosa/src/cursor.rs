//! Consumer-side replay primitives: tail → ack → resume.
//!
//! The [`Cursor`] trait abstracts over the on-disk [`JournalCursor`]
//! (the public [`pardosa::store::LineCursor`] wraps the file-backed
//! specialisation). See ADR-0011 for the locked decisions; ADR-0003
//! binds a cursor to a single journal. ADR-0018 §D3/§D5: the public
//! cursor surface is the sidecar-backed [`LineCursor`].
//!
//! [`pardosa::store::LineCursor`]: crate::store::LineCursor
use crate::PardosaError;
use crate::event::{Event, EventId};
use crate::persist;
use pardosa_schema::GenomeSafe;
use pardosa_wire::Decode;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
/// Consumer-side replay primitive over an event source.
///
/// The sole crate impl is [`JournalCursor`] (an on-disk `.pgno`
/// journal). The trait holds the *what* of "tail → ack → resume";
/// the impl holds the *how* of its underlying source.
///
/// ## Resume semantics
///
/// Exclusive (ADR-0011 D2). `tail()` yields events whose `event_id >
/// acked_offset()` if `acked_offset()` is `Some`, else every event in
/// source order.
pub(crate) trait Cursor<T> {
    /// Iterator type returned by `tail`. Borrows from `&mut self`; the
    /// GAT lifetime ties the iterator's lifetime to the cursor's
    /// borrow.
    type Iter<'a>: Iterator<Item = Result<Event<T>, PardosaError>>
    where
        Self: 'a,
        T: 'a;
    /// Start (or resume) iteration over the underlying source. Events
    /// already covered by `acked_offset()` are skipped.
    fn tail(&mut self) -> Self::Iter<'_>;
    /// Advance the acked watermark to `id`.
    ///
    /// Monotonic: a stale `id < acked_offset()` is a no-op; equal or
    /// strictly-greater advances. Subsequent `tail()` calls yield
    /// events with `event_id > id` (ADR-0011 D2, D8).
    ///
    /// # Errors
    ///
    /// Implementation-specific; persistence-backed cursors may surface
    /// sidecar I/O failures.
    fn commit_offset(&mut self, id: EventId) -> Result<(), PardosaError>;
    /// The most recently committed offset, or `None` if no commit has
    /// occurred since the cursor was opened (and no persisted state
    /// was found on disk for journal-backed impls).
    fn acked_offset(&self) -> Option<EventId>;
}
/// Map a `persist::Error` into the `PardosaError::CursorRead` variant.
/// Single-site helper so the `Iterator::next` body stays terse and the
/// boxing/discipline lives in one place.
fn wrap_persist_err(source: persist::Error) -> PardosaError {
    PardosaError::CursorRead {
        source: Box::new(source),
    }
}
/// Dragline-backed cursor over a `.pgno` source.
///
/// Reads through [`persist::stream`], which handles header validation,
/// schema-hash checking, decode, and exclusive `resume_after`
/// filtering. The cursor owns the reader; each `tail()` re-opens the
/// underlying [`EventStream`](persist::CheckedEventStream) so a fresh
/// `commit_offset` threshold takes effect immediately.
///
/// # Constructors
///
/// - [`from_path`](JournalCursor::from_path) (`R = File`): pairs the
///   journal with a sidecar file; `commit_offset` truncate-writes and
///   `fsync`s the sidecar. Production `pardosa::store` uses this
///   constructor exclusively.
#[derive(Debug)]
pub(crate) struct JournalCursor<R: Read + Seek, T> {
    reader: Option<R>,
    acked: Option<EventId>,
    /// Path to the sidecar file when constructed via [`from_path`].
    /// `None` when no sidecar is paired (test-only constructors):
    /// `commit_offset` is then in-memory only. `Some(path)` triggers a
    /// fsync-ed write-through on every successful [`commit_offset`].
    ///
    /// [`from_path`]: JournalCursor::from_path
    /// [`commit_offset`]: Cursor::commit_offset
    sidecar_path: Option<PathBuf>,
    _t: std::marker::PhantomData<fn() -> T>,
}
#[cfg(test)]
impl<R: Read + Seek, T> JournalCursor<R, T>
where
    T: Decode + GenomeSafe,
{
    /// Open a cursor over a `.pgno` source. Validates the container
    /// header up front (schema hash, magic, footer); subsequent
    /// `tail()` calls will re-open `persist::stream` on the same
    /// reader to honour the current `acked_offset` threshold.
    ///
    /// Test-only constructor: production `pardosa::store` uses the
    /// `File`-specialised [`from_path`](JournalCursor::from_path).
    ///
    /// # Errors
    /// Returns `PardosaError::CursorRead` wrapping any `persist::Error`
    /// produced by the initial header validation.
    pub fn from_source(source: R) -> Result<Self, PardosaError> {
        let probe: persist::CheckedEventStream<R, T> =
            persist::stream_checked(source, None).map_err(wrap_persist_err)?;
        let reader = probe.into_inner();
        Ok(Self {
            reader: Some(reader),
            acked: None,
            sidecar_path: None,
            _t: std::marker::PhantomData,
        })
    }
}
/// Sidecar file format: 8 little-endian bytes encoding the acked
/// `EventId.value()`.
///
/// A missing file means "no offset yet" (cursor restarts from the
/// beginning); any other length surfaces as
/// [`PardosaError::CursorSidecar`]. [`commit_offset`] truncate-writes
/// and `fsync`s before returning. ADR-0011 D5 documents the no
/// atomic-rename / no parent-directory-fsync choice: cursor offsets
/// are soft watermarks, exclusive resume is idempotent, and a torn
/// write is recovered by deleting the sidecar.
///
/// [`commit_offset`]: Cursor::commit_offset
const SIDECAR_LEN: usize = 8;
impl<T> JournalCursor<File, T>
where
    T: Decode + GenomeSafe,
{
    /// Open a journal-backed cursor with sidecar offset persistence.
    ///
    /// Reads `sidecar` if present, then validates the journal header.
    /// `commit_offset` truncate-writes and `fsync`s the sidecar.
    ///
    /// # Errors
    ///
    /// - [`PardosaError::CursorSidecar`] — sidecar unreadable or not
    ///   8 bytes. Recovery: delete it.
    /// - [`PardosaError::CursorJournalOpen`] — journal cannot be
    ///   opened. The sidecar is healthy; do not delete.
    /// - [`PardosaError::CursorRead`] — journal header invalid.
    ///
    /// # Stale sidecar
    ///
    /// An acked offset beyond every event-id is accepted; `tail()`
    /// then yields nothing (ADR-0011 D8).
    pub fn from_path(journal: &Path, sidecar: &Path) -> Result<Self, PardosaError> {
        let acked = read_sidecar(sidecar)?;
        let file = File::open(journal).map_err(|e| PardosaError::CursorJournalOpen {
            source: Box::new(e),
        })?;
        let probe: persist::CheckedEventStream<File, T> =
            persist::stream_checked(file, acked).map_err(wrap_persist_err)?;
        let reader = probe.into_inner();
        Ok(Self {
            reader: Some(reader),
            acked,
            sidecar_path: Some(sidecar.to_path_buf()),
            _t: std::marker::PhantomData,
        })
    }
}
/// Read the sidecar file. Returns `Ok(None)` if the file does not
/// exist (cursor starts from beginning); `Ok(Some(id))` if the file
/// is exactly 8 bytes; `Err(CursorSidecar)` for any other I/O or
/// length condition.
fn read_sidecar(path: &Path) -> Result<Option<EventId>, PardosaError> {
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(PardosaError::CursorSidecar {
                source: Box::new(e),
            });
        }
    };
    let mut buf = [0u8; SIDECAR_LEN];
    let mut total = 0usize;
    while total < SIDECAR_LEN {
        match f.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) => {
                return Err(PardosaError::CursorSidecar {
                    source: Box::new(e),
                });
            }
        }
    }
    if total != SIDECAR_LEN {
        return Err(PardosaError::CursorSidecar {
            source: Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("sidecar length {total} != expected {SIDECAR_LEN}"),
            )),
        });
    }
    let mut overflow = [0u8; 1];
    match f.read(&mut overflow) {
        Ok(0) => {}
        Ok(_) => {
            return Err(PardosaError::CursorSidecar {
                source: Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "sidecar longer than 8 bytes",
                )),
            });
        }
        Err(e) => {
            return Err(PardosaError::CursorSidecar {
                source: Box::new(e),
            });
        }
    }
    Ok(Some(EventId::from_decoded(u64::from_le_bytes(buf))))
}
/// Write+fsync the sidecar with the supplied `EventId`. Truncates any
/// previous contents.
fn write_sidecar(path: &Path, id: EventId) -> Result<(), PardosaError> {
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|e| PardosaError::CursorSidecar {
            source: Box::new(e),
        })?;
    f.write_all(&id.value().to_le_bytes())
        .map_err(|e| PardosaError::CursorSidecar {
            source: Box::new(e),
        })?;
    f.sync_data().map_err(|e| PardosaError::CursorSidecar {
        source: Box::new(e),
    })?;
    Ok(())
}
impl<R: Read + Seek, T> Cursor<T> for JournalCursor<R, T>
where
    T: Decode + GenomeSafe,
{
    type Iter<'i>
        = JournalCursorIter<'i, R, T>
    where
        Self: 'i,
        T: 'i;
    /// Re-open [`persist::stream`] against the cursor's reader with
    /// the current `acked_offset` as the resume threshold.
    ///
    /// If a prior `tail()` consumed the reader via a deferred
    /// `persist::stream` failure, every subsequent `tail()` returns
    /// a fresh iterator that surfaces a single
    /// [`PardosaError::CursorExhausted`] then stops. Callers must
    /// drop the cursor and reopen against a fresh source to retry
    /// (ADR-0011 D6).
    fn tail(&mut self) -> Self::Iter<'_> {
        match self.reader.take() {
            Some(reader) => {
                let stream = persist::stream_checked(reader, self.acked);
                JournalCursorIter {
                    inner: Some(IterInner::from_result(stream)),
                    cursor_slot: &mut self.reader,
                }
            }
            None => JournalCursorIter {
                inner: Some(IterInner::DeferredErr(Some(PardosaError::CursorExhausted))),
                cursor_slot: &mut self.reader,
            },
        }
    }
    /// Advance the acked watermark, persisting through the sidecar
    /// when constructed via [`from_path`].
    ///
    /// Monotonic per the trait contract: a stale `id <
    /// acked_offset()` is a no-op (no sidecar write). Out-of-range
    /// upper-bound commits are accepted (ADR-0011 D8); the cursor
    /// does not materialise the line.
    ///
    /// # Errors
    ///
    /// [`PardosaError::CursorSidecar`] when sidecar truncate-write or
    /// `fsync` fails.
    ///
    /// [`from_path`]: JournalCursor::from_path
    fn commit_offset(&mut self, id: EventId) -> Result<(), PardosaError> {
        if let Some(prev) = self.acked
            && id.value() <= prev.value()
        {
            return Ok(());
        }
        if let Some(path) = self.sidecar_path.as_deref() {
            write_sidecar(path, id)?;
        }
        self.acked = Some(id);
        Ok(())
    }
    fn acked_offset(&self) -> Option<EventId> {
        self.acked
    }
}
/// Iterator yielded by `JournalCursor::tail`. Holds either a live
/// `EventStream<R, T>` or the deferred construction error from
/// `persist::stream`. On drop the underlying reader is returned to
/// the parent cursor's slot so a subsequent `tail()` can re-borrow.
pub(crate) struct JournalCursorIter<'c, R: Read + Seek, T> {
    inner: Option<IterInner<R, T>>,
    cursor_slot: &'c mut Option<R>,
}
enum IterInner<R: Read + Seek, T> {
    Live(persist::CheckedEventStream<R, T>),
    /// Construction error captured at `tail()` time; surfaced once,
    /// then the iterator yields `None` forever after.
    DeferredErr(Option<PardosaError>),
}
impl<R: Read + Seek, T> IterInner<R, T> {
    fn from_result(r: Result<persist::CheckedEventStream<R, T>, persist::Error>) -> Self {
        match r {
            Ok(s) => IterInner::Live(s),
            Err(e) => IterInner::DeferredErr(Some(wrap_persist_err(e))),
        }
    }
}
impl<R: Read + Seek, T> Iterator for JournalCursorIter<'_, R, T>
where
    T: Decode + GenomeSafe,
{
    type Item = Result<Event<T>, PardosaError>;
    fn next(&mut self) -> Option<Self::Item> {
        match self.inner.as_mut()? {
            IterInner::Live(stream) => match stream.next()? {
                Ok(ev) => Some(Ok(ev)),
                Err(e) => Some(Err(wrap_persist_err(e))),
            },
            IterInner::DeferredErr(slot) => slot.take().map(Err),
        }
    }
}
impl<R: Read + Seek, T> JournalCursor<R, T> {
    /// Test-only helper that synthesises the post-DeferredErr state
    /// (reader consumed, slot left `None`). Used to exercise the
    /// hardening path in `tail()` without contriving an external
    /// `.pgno` truncation race. The corresponding production path is
    /// reachable when an external process truncates or removes the
    /// underlying journal between [`JournalCursor::from_source`] / [`JournalCursor::from_path`]
    /// and a subsequent `tail()` — `persist::stream` then fails at
    /// `tail()` time and the iterator's `DeferredErr` arm consumes
    /// the reader without restoring it; the next `tail()` finds
    /// `self.reader == None` and surfaces `CursorExhausted`.
    #[cfg(test)]
    pub(crate) fn force_exhausted_for_test(&mut self) {
        self.reader = None;
    }
}
impl<R: Read + Seek, T> Drop for JournalCursorIter<'_, R, T> {
    fn drop(&mut self) {
        if let Some(IterInner::Live(stream)) = self.inner.take() {
            *self.cursor_slot = Some(stream.into_inner());
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn journal_cursor_tail_after_exhausted_reader_surfaces_typed_error() {
        use crate::dragline::Dragline;
        use std::io::Cursor as IoCursor;
        let sink: IoCursor<Vec<u8>> = IoCursor::new(Vec::new());
        let mut j: Dragline<u64, _> = Dragline::new(sink);
        let _ = j.commit_event(0).unwrap();
        let _ = j.sync_data_with_source(None).unwrap();
        let bytes = j.into_inner().into_inner();
        let mut c: JournalCursor<_, u64> =
            JournalCursor::from_source(IoCursor::new(bytes)).unwrap();
        c.force_exhausted_for_test();
        let mut it = c.tail();
        match it.next() {
            Some(Err(PardosaError::CursorExhausted)) => {}
            other => panic!("expected first item Err(CursorExhausted), got {other:?}"),
        }
        assert!(
            it.next().is_none(),
            "exhausted iterator must yield None after the single error"
        );
    }
}
