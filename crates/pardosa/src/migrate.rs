//! Schema migration: whole-stream `Keep` from `Old` to `New`
//! (mission `phase4-migrate-keep-20260528`).
//!
//! [`migrate_keep`] decodes a `.pgno` as `Event<Old>`, calls the
//! adopter upcast in order, writes `New` payloads through a fresh
//! crate-internal journal mirroring fiber topology (`commit_event`
//! / `commit_update` / `commit_detach` / `commit_rescue`), then
//! syncs.
//!
//! `Keep` preserves order, upcast coverage, and topology shape.
//! Old `EventId`/`FiberId` are **not** preserved (ADR-0003 —
//! substrate-issued); embed correlation ids in `T` if needed.
//!
//! Read/upcast errors abort before sync; [`MigrationError`]
//! carries positions and adopter `E` typed (ADR-0007).
use crate::dragline::Dragline;
use crate::durability::Lsn;
use crate::error::PardosaError;
use crate::event::{Event, FiberId};
use crate::persist;
use crate::typed::TypedReader;
use pardosa_file::Syncable;
use pardosa_schema::GenomeSafe;
use pardosa_wire::{Decode, Encode, from_bytes};
use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::Path;
/// Report returned by a successful [`migrate_keep`] call. Carries
/// the old/new event counts, the post-sync [`Lsn`], and the new
/// sink so callers can re-open it for verification or hand it off.
#[derive(Debug)]
#[must_use]
pub struct MigrationReport<W> {
    old_event_count: u64,
    new_event_count: u64,
    synced_lsn: Lsn,
    new_sink: W,
}
impl<W> MigrationReport<W> {
    /// Number of `Event<Old>` records read from the old source.
    #[must_use]
    pub fn old_event_count(&self) -> u64 {
        self.old_event_count
    }
    /// Number of `Event<New>` records written into the new sink.
    /// Equal to [`old_event_count`](Self::old_event_count) on a
    /// `Keep` migration.
    #[must_use]
    pub fn new_event_count(&self) -> u64 {
        self.new_event_count
    }
    /// Post-`sync` byte length of the new sink. This is the exact
    /// [`Lsn`] minted by the terminal sync; it corresponds to the
    /// `Some(lsn)` value observable through
    /// `StoreWriter::acked_lsn` before the log is
    /// consumed.
    #[must_use = "migration reports should inspect the terminal sync LSN"]
    pub fn synced_lsn(&self) -> Lsn {
        self.synced_lsn
    }
    /// Consume the report and return the new sink, positioned
    /// wherever the underlying file left it after the final
    /// `sync`.
    pub fn into_inner(self) -> W {
        self.new_sink
    }
    /// Deprecated alias for [`MigrationReport::into_inner`]; kept
    /// for `SemVer` compatibility with the pre-0.5 naming.
    #[deprecated(
        since = "0.5.0",
        note = "renamed to `into_inner` for parity with the std consume-and-return convention; use ::into_inner instead"
    )]
    pub fn into_new_sink(self) -> W {
        self.into_inner()
    }
}
/// Typed error envelope for [`migrate_keep`].
///
/// The adopter upcast error `E` is preserved as a typed `source`
/// rather than being stringified (ADR-0007 typed errors,
/// non-cyclic).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MigrationError<E> {
    /// Opening the old source failed at the framing layer (bad
    /// header, schema-hash mismatch on the old container, etc.).
    #[error("opening old stream: {0}")]
    OldOpen(#[source] PardosaError),
    /// Opening (or creating) the new sink path failed at the
    /// filesystem layer (`OpenOptions::open` returned an
    /// `io::Error`, wrapped as [`PardosaError::CursorJournalOpen`]).
    /// Surfaced only by the path-backed [`migrate_keep`] wrapper;
    /// aborts before any bytes are written, so no rollback is
    /// required.
    #[error("opening new stream: {0}")]
    NewOpen(#[source] PardosaError),
    /// A per-event read or decode on the old stream failed.
    /// `position` is the zero-based event-line index at which the
    /// error surfaced.
    #[error("reading old stream at position {position}: {source}")]
    OldRead {
        position: u64,
        #[source]
        source: PardosaError,
    },
    /// The adopter's upcast closure rejected an event. `position`
    /// is the zero-based event-line index of the offending event.
    /// `source` carries the adopter error verbatim (typed; not
    /// stringified).
    #[error("upcast at position {position}: {source}")]
    Upcast {
        position: u64,
        #[source]
        source: E,
    },
    /// Writing a `New` event into the fresh sink failed
    /// (commit-pipeline error).
    #[error("writing new stream at position {position}: {source}")]
    NewWrite {
        position: u64,
        #[source]
        source: PardosaError,
    },
    /// The terminal `sync` on the new sink failed. By construction
    /// this is the only variant that can surface after any durable
    /// bytes have been written; every other variant aborts before
    /// `sync` is attempted.
    #[error("syncing new stream: {0}")]
    NewSync(#[source] persist::Error),
}
enum FiberHandle {
    Live(FiberId),
    Detached(FiberId),
}
fn wrap_persist(source: persist::Error) -> PardosaError {
    PardosaError::CursorRead {
        source: Box::new(source),
    }
}
/// Whole-stream `Keep` migration from `Old` to `New`, driven by
/// filesystem paths. Sole public migration path (ADR-0018 Amendment 1).
///
/// Opens `old_path` read-only and `new_path` read-write
/// (`create(true).truncate(true)`), then decodes as `Event<Old>`,
/// applies `upcast`, and writes through a fresh journal mirroring
/// fiber topology. The new sink is `sync_data`-ed once at end.
///
/// New `EventId`/`FiberId` are substrate-minted; order, count,
/// upcast coverage, and lifecycle shape preserved.
///
/// # Errors
///
/// [`MigrationError`]. Variants before `NewSync` abort before sync;
/// `NewSync` can fire after partial writes — discard the file.
pub fn migrate_keep<Old, New, E, F>(
    old_path: &Path,
    new_path: &Path,
    upcast: F,
) -> Result<MigrationReport<std::fs::File>, MigrationError<E>>
where
    Old: Decode + GenomeSafe,
    New: Encode + GenomeSafe,
    F: FnMut(Event<Old>) -> Result<New, E>,
{
    let old_source = std::fs::OpenOptions::new()
        .read(true)
        .open(old_path)
        .map_err(|e| {
            MigrationError::OldOpen(PardosaError::CursorJournalOpen {
                source: Box::new(e),
            })
        })?;
    let new_sink = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(new_path)
        .map_err(|e| {
            MigrationError::NewOpen(PardosaError::CursorJournalOpen {
                source: Box::new(e),
            })
        })?;
    migrate_keep_generic(old_source, new_sink, upcast)
}
/// Generic-W `Keep` migration over arbitrary `Read + Seek` and
/// `Syncable + Seek`. Crate-internal: in-tree callers (tests,
/// future in-memory adopters) use this directly; external adopters
/// go through the path-backed [`migrate_keep`] wrapper above
/// (ADR-0018 §D7 sole-interface boundary).
pub(crate) fn migrate_keep_generic<R, W, Old, New, E, F>(
    old_source: R,
    new_sink: W,
    mut upcast: F,
) -> Result<MigrationReport<W>, MigrationError<E>>
where
    R: Read + Seek,
    W: Syncable + Seek,
    Old: Decode + GenomeSafe,
    New: Encode + GenomeSafe,
    F: FnMut(Event<Old>) -> Result<New, E>,
{
    let mut reader: TypedReader<R, Old> =
        TypedReader::open(old_source).map_err(|e| MigrationError::OldOpen(wrap_persist(e)))?;
    let mut journal: Dragline<New, W> = Dragline::new(new_sink);
    let mut fibers: HashMap<FiberId, FiberHandle> = HashMap::new();
    let mut old_count: u64 = 0;
    let mut new_count: u64 = 0;
    for item in reader.inner_mut().iter_messages() {
        let position = old_count;
        let bytes = item.map_err(|e| MigrationError::OldRead {
            position,
            source: wrap_persist(persist::Error::File(e)),
        })?;
        let event: Event<Old> = from_bytes(&bytes).map_err(|e| MigrationError::OldRead {
            position,
            source: wrap_persist(persist::Error::Decode(e)),
        })?;
        let old_fid = event.fiber_id();
        let was_detached_event = event.detached();
        old_count = old_count.checked_add(1).ok_or(MigrationError::NewWrite {
            position,
            source: PardosaError::IndexOverflow,
        })?;
        let new_payload =
            upcast(event).map_err(|source| MigrationError::Upcast { position, source })?;
        let next = match fibers.remove(&old_fid) {
            None => {
                let ar = journal
                    .commit_event(new_payload)
                    .map_err(|source| MigrationError::NewWrite { position, source })?;
                FiberHandle::Live(ar.fiber_id)
            }
            Some(FiberHandle::Live(fid)) => {
                if was_detached_event {
                    let ar = journal
                        .commit_detach(fid, new_payload)
                        .map_err(|source| MigrationError::NewWrite { position, source })?;
                    FiberHandle::Detached(ar.fiber_id)
                } else {
                    let ar = journal
                        .commit_update(fid, new_payload)
                        .map_err(|source| MigrationError::NewWrite { position, source })?;
                    FiberHandle::Live(ar.fiber_id)
                }
            }
            Some(FiberHandle::Detached(fid)) => {
                let ar = journal
                    .commit_rescue(fid, new_payload)
                    .map_err(|source| MigrationError::NewWrite { position, source })?;
                FiberHandle::Live(ar.fiber_id)
            }
        };
        fibers.insert(old_fid, next);
        new_count = new_count.checked_add(1).ok_or(MigrationError::NewWrite {
            position,
            source: PardosaError::IndexOverflow,
        })?;
    }
    let synced_lsn = journal
        .sync_data_with_source(None)
        .map_err(MigrationError::NewSync)?;
    let new_sink = journal.into_inner();
    Ok(MigrationReport {
        old_event_count: old_count,
        new_event_count: new_count,
        synced_lsn,
        new_sink,
    })
}
