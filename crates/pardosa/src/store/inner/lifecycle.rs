use super::{
    Decode, Encode, EventStore, FrontierPublisher, GenomeSafe, PardosaError, Path, PathBuf,
    Validate, ValidatedReplayError,
};
use crate::authoritative::{AuthoritativeBackend, BackendDispatch, admit_into_dispatch};
use crate::backend::rehydrate::from_pgno_bytes_unchecked;
use crate::backend::{BackendSink, journal::RehydrateableBackend};
use crate::dragline::Dragline;
use std::io::{Read, Seek, SeekFrom};
fn open_rw_seek_and_rehydrate_unchecked<T>(
    path: &Path,
) -> Result<(std::fs::File, crate::dragline::Line<T>), PardosaError>
where
    T: Decode + GenomeSafe,
{
    use std::io::SeekFrom;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| PardosaError::CursorJournalOpen {
            source: Box::new(e),
        })?;
    file.seek(SeekFrom::Start(0))
        .map_err(|e| PardosaError::CursorRead {
            source: Box::new(crate::persist::Error::Io(e)),
        })?;
    let dragline = crate::persist::rehydrate_unchecked::<T, _>(&mut file).map_err(|e| {
        PardosaError::CursorRead {
            source: Box::new(e),
        }
    })?;
    file.seek(SeekFrom::Start(0))
        .map_err(|e| PardosaError::CursorRead {
            source: Box::new(crate::persist::Error::Io(e)),
        })?;
    Ok((file, dragline))
}
fn open_rw_seek_and_rehydrate_validated<T>(
    path: &Path,
) -> Result<(std::fs::File, crate::dragline::Line<T>), ValidatedReplayError<<T as Validate>::Error>>
where
    T: Decode + GenomeSafe + Validate,
{
    use std::io::SeekFrom;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| ValidatedReplayError::Replay(crate::persist::Error::Io(e)))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|e| ValidatedReplayError::Replay(crate::persist::Error::Io(e)))?;
    let dragline = crate::persist::rehydrate_validated::<T, _>(&mut file)?;
    file.seek(SeekFrom::Start(0))
        .map_err(|e| ValidatedReplayError::Replay(crate::persist::Error::Io(e)))?;
    Ok((file, dragline))
}
fn persist_error_to_cursor_read(e: crate::persist::Error) -> PardosaError {
    PardosaError::CursorRead {
        source: Box::new(e),
    }
}
fn io_error_to_cursor_read(e: std::io::Error) -> PardosaError {
    persist_error_to_cursor_read(crate::persist::Error::Io(e))
}
fn backend_error_to_cursor_read(
    context: &'static str,
    e: &crate::error::BackendError,
) -> PardosaError {
    io_error_to_cursor_read(std::io::Error::other(format!("{context}: {e}")))
}
fn fetch_jetstream_bytes(
    adapter: &mut crate::authoritative::jetstream::JetStreamBackendAdapter,
) -> Result<Vec<u8>, PardosaError> {
    adapter
        .fetch_durable_bytes()
        .map_err(|e| backend_error_to_cursor_read("JetStream rehydrate fetch failed", &e))
}
fn seed_jetstream_backend<T>(
    adapter: &mut crate::authoritative::jetstream::JetStreamBackendAdapter,
) -> Result<crate::dragline::Line<T>, PardosaError>
where
    T: Encode + Decode + GenomeSafe + crate::typed::HasEventSchemaSource,
{
    let seed_bytes = canonical_empty_pgno_bytes::<T>()?;
    let _ = adapter
        .append(&seed_bytes)
        .map_err(|e| backend_error_to_cursor_read("JetStream create seed append failed", &e))?;
    let _ = adapter
        .sync()
        .map_err(|e| backend_error_to_cursor_read("JetStream create seed sync failed", &e))?;
    from_pgno_bytes_unchecked::<T>(&seed_bytes).map_err(persist_error_to_cursor_read)
}
fn canonical_empty_pgno_bytes<T>() -> Result<Vec<u8>, PardosaError>
where
    T: Encode + GenomeSafe + crate::typed::HasEventSchemaSource,
{
    let scratch = tempfile::tempfile().map_err(|e| PardosaError::CursorJournalOpen {
        source: Box::new(e),
    })?;
    let mut seed = EventStore::<T> {
        inner: Dragline::new(scratch),
        journal: PathBuf::new(),
        schema_source: <T as crate::typed::HasEventSchemaSource>::EVENT_SCHEMA_SOURCE,
    };
    let _ = seed.writer().sync().map_err(persist_error_to_cursor_read)?;
    let mut scratch = seed.inner.into_inner();
    scratch
        .seek(SeekFrom::Start(0))
        .map_err(io_error_to_cursor_read)?;
    let mut bytes = Vec::new();
    scratch
        .read_to_end(&mut bytes)
        .map_err(io_error_to_cursor_read)?;
    Ok(bytes)
}
impl<T> EventStore<T, std::fs::File>
where
    T: super::Encode + Decode + GenomeSafe + crate::typed::HasEventSchemaSource,
{
    /// Construct a fresh `EventStore<T>` over a freshly-created
    /// `.pgno` file at `path`. Overwrites any existing file.
    ///
    /// When `T` declares
    /// [`crate::typed::HasEventSchemaSource::EVENT_SCHEMA_SOURCE`] as
    /// `Some(source)`, that string is embedded in the container
    /// on the first [`super::StoreWriter::sync`].
    ///
    /// # Durability
    ///
    /// The parent directory is `sync_data`-fenced via
    /// [`pardosa_file::fsync_parent_dir`] so the new entry is
    /// durable per the host's POSIX contract (ADR-0010 §D3).
    ///
    /// # Errors
    ///
    /// [`PardosaError::CursorJournalOpen`] on file create failure
    /// or on parent-directory `sync_data` failure.
    pub fn create(path: &Path) -> Result<Self, PardosaError> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .map_err(|e| PardosaError::CursorJournalOpen {
                source: Box::new(e),
            })?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        pardosa_file::fsync_parent_dir(parent).map_err(|e| PardosaError::CursorJournalOpen {
            source: Box::new(e),
        })?;
        let inner = Dragline::new(file);
        let schema_source = <T as crate::typed::HasEventSchemaSource>::EVENT_SCHEMA_SOURCE;
        Ok(Self {
            inner,
            journal: path.to_path_buf(),
            schema_source,
        })
    }
    /// Construct a fresh typed-backend `EventStore<T>` from an
    /// admitted authoritative backend.
    ///
    /// Mirrors [`EventStore::create`] at the typed-backend seam:
    /// path-backed backends delegate to the `.pgno` create path;
    /// `JetStream` backends author the canonical empty `.pgno`
    /// container inside pardosa and seed it only when replay shows the
    /// stream is empty. Populated `JetStream` streams are rehydrated
    /// without writing, so repeated create attempts cannot clobber
    /// existing data.
    ///
    /// # Errors
    ///
    /// [`PardosaError::CursorJournalOpen`] when scratch or path-backed
    /// file creation fails. [`PardosaError::CursorRead`] when backend
    /// replay, canonical empty-container serialisation, seed append,
    /// seed sync, or `.pgno` rehydrate fails.
    pub fn create_with_backend<B: AuthoritativeBackend>(backend: B) -> Result<Self, PardosaError> {
        match admit_into_dispatch(backend) {
            BackendDispatch::Pgno(p) => Self::create(p.path()),
            BackendDispatch::JetStream(boxed_adapter) => {
                let mut adapter = *boxed_adapter;
                let bytes = fetch_jetstream_bytes(&mut adapter)?;
                let dragline = if bytes.is_empty() {
                    seed_jetstream_backend::<T>(&mut adapter)?
                } else {
                    from_pgno_bytes_unchecked::<T>(&bytes).map_err(persist_error_to_cursor_read)?
                };
                let scratch =
                    tempfile::tempfile().map_err(|e| PardosaError::CursorJournalOpen {
                        source: Box::new(e),
                    })?;
                let inner = Dragline::from_backend_for_open_jetstream(dragline, scratch, adapter);
                Ok(Self {
                    inner,
                    journal: PathBuf::new(),
                    schema_source: <T as crate::typed::HasEventSchemaSource>::EVENT_SCHEMA_SOURCE,
                })
            }
            #[cfg(any(test, feature = "test-support"))]
            BackendDispatch::InMem(_) => Err(PardosaError::CursorJournalOpen {
                source: Box::new(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "InMemoryBackend is reserved for in-crate test fixtures \
                     and is not admissible via EventStore::create_with_backend",
                )),
            }),
        }
    }
}
impl<T> EventStore<T, std::fs::File>
where
    T: Decode + GenomeSafe,
{
    /// Open an existing `.pgno` log at `path` (ADR-0018 §D7).
    ///
    /// Validates the container header (schema-hash mismatch →
    /// [`PardosaError::CursorRead`]) and rehydrates the dragline.
    /// No auto-migration; [`super::super::migrate::migrate_keep`]
    /// is the only public migration path.
    ///
    /// ADR-0020 scope: framing, schema-hash, and contiguity checks
    /// only. Per-event precursor-hash and [`Validate`] payload
    /// checks live on [`EventStore::open_validated`].
    ///
    /// Visibility: `pub(crate)` by default; widened to `pub` under
    /// `feature = "test-support"` so integration tests can compare
    /// against the validated open path.
    ///
    /// # Errors
    ///
    /// [`PardosaError`] from the rehydrate pipeline.
    #[cfg(not(any(test, feature = "test-support")))]
    #[expect(
        dead_code,
        reason = "pub(crate) mirror of the test-support pub variant below; \
                  retained for visibility-symmetry across the cfg split so \
                  the rehydrate pipeline has a single in-crate entry shape"
    )]
    pub(crate) fn open(path: &Path) -> Result<Self, PardosaError> {
        let (file, dragline) = open_rw_seek_and_rehydrate_unchecked::<T>(path)?;
        let inner = Dragline::from_line_for_open(dragline, file);
        Ok(Self {
            inner,
            journal: path.to_path_buf(),
            schema_source: None,
        })
    }
    /// Test-support variant of [`EventStore::open`]: same
    /// rehydrate pipeline, broader visibility so integration tests
    /// and adopters under `feature = "test-support"` can exercise
    /// the unchecked open path against the validated one
    /// ([`EventStore::open_validated`]). Mirrors the `pub(crate)`
    /// form bit-for-bit; the cfg split only widens visibility
    /// under the gate.
    #[cfg(any(test, feature = "test-support"))]
    pub fn open(path: &Path) -> Result<Self, PardosaError> {
        let (file, dragline) = open_rw_seek_and_rehydrate_unchecked::<T>(path)?;
        let inner = Dragline::from_line_for_open(dragline, file);
        Ok(Self {
            inner,
            journal: path.to_path_buf(),
            schema_source: None,
        })
    }
    /// Open the substrate identified by `backend` (ADR-0022 §D1 /
    /// §D11 / §D12). Canonical typed-backend constructor; accepts
    /// any in-crate sealed [`AuthoritativeBackend`]:
    ///
    /// * [`PgnoBackend`] — delegates to the `.pgno` rehydrate path.
    /// * [`crate::store::JetStreamBackend`] — rehydrates from the
    ///   sync-fenced blob via the §D2 reader-side seam; writer
    ///   `sync` routes through sealed
    ///   [`crate::backend::BackendSink`]. Alias arity preserved.
    ///
    /// ADR-0022 §D12 admits only `open_with_backend` to the
    /// audit allowlist.
    ///
    /// # Errors
    ///
    /// [`PardosaError`] from the rehydrate or scratch-tempfile
    /// path (surfacing as [`PardosaError::CursorRead`] /
    /// [`PardosaError::CursorJournalOpen`]).
    pub fn open_with_backend<B: AuthoritativeBackend>(backend: B) -> Result<Self, PardosaError> {
        match admit_into_dispatch(backend) {
            BackendDispatch::Pgno(p) => {
                let (file, dragline) = open_rw_seek_and_rehydrate_unchecked::<T>(p.path())?;
                let inner = Dragline::from_backend_for_open(dragline, file);
                Ok(Self {
                    inner,
                    journal: p.path().to_path_buf(),
                    schema_source: None,
                })
            }
            BackendDispatch::JetStream(boxed_adapter) => {
                let mut adapter = *boxed_adapter;
                let bytes = fetch_jetstream_bytes(&mut adapter)?;
                let dragline =
                    from_pgno_bytes_unchecked::<T>(&bytes).map_err(persist_error_to_cursor_read)?;
                let scratch =
                    tempfile::tempfile().map_err(|e| PardosaError::CursorJournalOpen {
                        source: Box::new(e),
                    })?;
                let inner = Dragline::from_backend_for_open_jetstream(dragline, scratch, adapter);
                Ok(Self {
                    inner,
                    journal: PathBuf::new(),
                    schema_source: None,
                })
            }
            #[cfg(any(test, feature = "test-support"))]
            BackendDispatch::InMem(_) => Err(PardosaError::CursorJournalOpen {
                source: Box::new(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "InMemoryBackend is reserved for in-crate test fixtures \
                     and is not admissible via EventStore::open_with_backend",
                )),
            }),
        }
    }
    /// Open an existing `.pgno` log at `path` and attach a durable
    /// [`FrontierPublisher`] (ADR-0018 §12 bullet 3;
    /// ADR-0016 §§D5–D8).
    ///
    /// Pairs the rehydrated dragline with `publisher` plus a
    /// publish-watermark sidecar at `publish_sidecar` (fsynced
    /// after each successful anchor dispatch). On reopen,
    /// unpublished anchors are reconstructed from the persisted
    /// line (ADR-0016 §D6).
    ///
    /// `stream_name` interpolates into
    /// `pardosa.{stream_name}.frontier` (ADR-0015 §D3).
    /// `anchor_interval` is per-tick event count (`0` → `1`).
    ///
    /// # Errors
    ///
    /// [`PardosaError`] from rehydrate, [`PardosaError::PublishWatermark`]
    /// from sidecar read, or [`PardosaError::CursorJournalOpen`]
    /// from the file open.
    pub fn open_with_publisher(
        path: &Path,
        publish_sidecar: PathBuf,
        stream_name: String,
        anchor_interval: u64,
        publisher: Box<dyn FrontierPublisher>,
    ) -> Result<Self, PardosaError> {
        let (file, dragline) = open_rw_seek_and_rehydrate_unchecked::<T>(path)?;
        let inner = Dragline::with_line_and_publisher_path(
            dragline,
            file,
            publish_sidecar,
            stream_name,
            anchor_interval,
            publisher,
        )?;
        Ok(Self {
            inner,
            journal: path.to_path_buf(),
            schema_source: None,
        })
    }
}
impl<T> EventStore<T, std::fs::File>
where
    T: Decode + GenomeSafe + Validate,
{
    /// Open `path` with full per-event validation
    /// (Fiber-semantics goal 6; ADR-0018 §D7).
    ///
    /// Same invariants as [`EventStore::open`] plus per-event
    /// envelope-shape check and payload
    /// [`Validate::validate`]. Prefer this when foreign-payload
    /// `Decode` impls may produce domain-invalid `T`. No
    /// auto-migration; use [`super::super::migrate::migrate_keep`].
    ///
    /// # Errors
    ///
    /// Returns [`ValidatedReplayError`] for any per-event failure.
    /// File-open I/O surfaces as
    /// [`ValidatedReplayError::Replay`] wrapping
    /// [`crate::persist::Error::Io`].
    pub fn open_validated(
        path: &Path,
    ) -> Result<Self, ValidatedReplayError<<T as Validate>::Error>> {
        let (file, dragline) = open_rw_seek_and_rehydrate_validated::<T>(path)?;
        let inner = Dragline::from_line_for_open(dragline, file);
        Ok(Self {
            inner,
            journal: path.to_path_buf(),
            schema_source: None,
        })
    }
}
/// Adopter-facing snapshot of a persisted `.pgno`'s container
/// metadata (ADR-0018 §D7).
///
/// Returned by `EventStore::<T>::metadata`. Carries the values
/// adopters typically want before deciding whether to invoke
/// `EventStore::<T>::open_validated`: event count, the composed
/// `Event::<T>::ENVELOPE_HASH` from the header, and the optional
/// schema source embedded at create time. Owns its strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreMetadata {
    len: u64,
    schema_hash: u128,
    schema_source: Option<String>,
}
impl StoreMetadata {
    /// Number of events persisted in the log.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.len
    }
    /// `true` when the log holds zero events.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Composed `Event::<T>::ENVELOPE_HASH` recorded in the
    /// container header (ADR-0005 / ADR-0006).
    #[must_use]
    pub fn schema_hash(&self) -> u128 {
        self.schema_hash
    }
    /// Embedded human-readable schema source, if the writer set
    /// `T::EVENT_SCHEMA_SOURCE` to `Some(_)` at create time.
    #[must_use]
    pub fn schema_source(&self) -> Option<&str> {
        self.schema_source.as_deref()
    }
}
impl<T> EventStore<T, std::fs::File>
where
    T: Decode + GenomeSafe,
{
    /// Read container metadata from the `.pgno` at `path` without
    /// rehydrating a dragline (ADR-0018 §D7 / § Naming).
    ///
    /// Opens the file read-only, validates the container header's
    /// schema hash against `Event::<T>::ENVELOPE_HASH`, and returns
    /// a [`StoreMetadata`] snapshot. No fiber-state, line, or
    /// cursor data is materialised; the file handle is dropped
    /// before return.
    ///
    /// # Errors
    ///
    /// Returns [`PardosaError::CursorJournalOpen`] when the file
    /// cannot be opened, and [`PardosaError::CursorRead`] wrapping
    /// [`crate::persist::Error::SchemaHashMismatch`] (or other
    /// framing errors) when the header is invalid for `T`.
    pub fn metadata(path: &Path) -> Result<StoreMetadata, PardosaError> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|e| PardosaError::CursorJournalOpen {
                source: Box::new(e),
            })?;
        let reader = crate::typed::TypedReader::<std::fs::File, T>::open(file).map_err(|e| {
            PardosaError::CursorRead {
                source: Box::new(e),
            }
        })?;
        Ok(StoreMetadata {
            len: reader.message_count(),
            schema_hash: reader.schema_hash(),
            schema_source: reader.schema_source().map(String::from),
        })
    }
}
