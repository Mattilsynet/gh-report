#![allow(dead_code)]
use super::BackendSink;
use crate::dragline::Line;
use crate::durability::AckPosition;
use crate::error::{BackendError, BackendOp};
use crate::persist::{Error as PersistError, persist_with_source_append};
use pardosa_schema::GenomeSafe;
use pardosa_wire::{Decode, Encode};
use std::io::Cursor;
/// Reason a [`BackendDragline::sync`] could not complete
/// (ADR-0007; `#[non_exhaustive]`).
///
/// Two axes: `.pgno` serialisation ([`Self::Persist`], wrapping
/// [`PersistError`]) and substrate dispatch ([`Self::Backend`],
/// wrapping [`BackendError`] with a [`BackendOp`] discriminator
/// naming whether `append` or `sync` failed).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum SyncError {
    /// `.pgno` serialisation of the dragline failed before any
    /// bytes reached the backend (e.g. dragline unpersistable
    /// per [`crate::dragline::Line::check_persistable`]).
    #[error(".pgno serialisation failed before bytes reached the backend: {0}")]
    Persist(#[from] PersistError),
    /// The backend rejected the substrate dispatch. `op`
    /// names whether the failure occurred during
    /// [`BackendSink::append`] or [`BackendSink::sync`] —
    /// material at the recovery layer because append failures
    /// can retry the same payload, while sync failures need
    /// the substrate's actual fence position before retry.
    #[error("backend dispatch failed during `{op}`: {source}")]
    Backend {
        op: BackendOp,
        #[source]
        source: BackendError,
    },
}
/// Reason a [`BackendDragline::rehydrate`] could not complete
/// (ADR-0007 in-crate error-taxonomy convention;
/// `#[non_exhaustive]` for forward compatibility with the
/// validated-path counterpart).
///
/// Wraps the existing [`PersistError`] taxonomy returned by
/// [`super::rehydrate::from_pgno_bytes_unchecked`] verbatim.
/// The fetch-shaped reader-side seam
/// ([`BackendDragline::rehydrate_from`] +
/// [`RehydrateableBackend`]) additionally surfaces backend
/// transport failures through [`Self::Backend`], carrying
/// [`BackendError`] verbatim and the [`BackendOp`]
/// discriminator so the recovery layer can distinguish
/// transport-side fetch failures from `.pgno` decode failures
/// without re-querying the substrate.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum RehydrateError {
    #[error("rehydrate from backend bytes failed: {0}")]
    Persist(#[from] PersistError),
    /// The backend's fetch-shaped reader-side seam
    /// ([`RehydrateableBackend::fetch_durable_bytes`]) rejected
    /// the substrate dispatch before any bytes reached the
    /// `.pgno` decoder. `op` is the [`BackendOp`] discriminator
    /// for the failing sub-op (`BackendOp::Sync` for a fetch
    /// that conceptually mirrors the sync fence, mirroring
    /// [`SyncError::Backend`]'s `op`-discriminator shape).
    #[error("backend fetch failed during `{op}`: {source}")]
    Backend {
        op: BackendOp,
        #[source]
        source: BackendError,
    },
}
/// In-crate composition of a [`Line<T>`] with a sealed
/// [`BackendSink`] sink (ADR-0022 §D2 / §D11).
///
/// Generic over both the event type `T` and the backend `B`
/// so the same primitive serves both real (`PgnoFileSink`,
/// `JetStream` adapter shim) and in-tree fake
/// ([`crate::authoritative::fake::InMemoryBackend`])
/// substrates. The `B: BackendSink` bound is the runtime's
/// closed sealing posture — there is no path for an external
/// crate to instantiate `BackendDragline<T, B>` with a
/// non-sanctioned backend.
pub(crate) struct BackendDragline<T, B: BackendSink> {
    line: Line<T>,
    backend: B,
    schema_source: Option<&'static str>,
}
impl<T, B: BackendSink> BackendDragline<T, B> {
    /// Construct an empty backend-backed journal wrapping
    /// `backend` as the sealed substrate sink. No I/O.
    /// Mirrors the [`crate::dragline::Dragline::new`] shape:
    /// the in-memory dragline starts empty under
    /// [`Line::new`], so the substrate-composition layer
    /// is publisher-agnostic — anchors flow only on
    /// [`Self::sync`] via the `.pgno` blob the backend
    /// receives.
    pub(crate) fn new(backend: B) -> Self {
        Self {
            line: Line::new(),
            backend,
            schema_source: None,
        }
    }
    /// Attach a schema-source descriptor that will be embedded
    /// in the `.pgno` container header on the next
    /// [`Self::sync`]. Mirrors the
    /// [`crate::dragline::Dragline::sync_data_with_source`]
    /// shape.
    pub(crate) fn with_schema_source(mut self, source: Option<&'static str>) -> Self {
        self.schema_source = source;
        self
    }
    /// Borrow the underlying [`Line`] for inspection
    /// (in-tree test harness use only).
    pub(crate) fn line(&self) -> &Line<T> {
        &self.line
    }
    /// Consume the journal, returning the wrapped backend so
    /// in-tree tests can observe the bytes the substrate
    /// received (e.g.
    /// [`crate::authoritative::fake::InMemoryBackend::bytes`]).
    pub(crate) fn into_backend(self) -> B {
        self.backend
    }
}
impl<T, B> BackendDragline<T, B>
where
    T: Encode + GenomeSafe,
    B: BackendSink,
{
    /// Append a new event to the in-memory dragline. Mirrors
    /// the [`crate::dragline::Dragline::commit_event`] shape:
    /// the event is in-memory only until a subsequent
    /// [`Self::sync`] succeeds (ADR-0010 / ADR-0022 §D2 —
    /// `append` does not imply durability; `sync` is the
    /// fence).
    ///
    /// # Errors
    ///
    /// Forwards any [`crate::error::PardosaError`] from
    /// [`crate::dragline::Line::create`].
    pub(crate) fn commit_event(
        &mut self,
        event: T,
    ) -> Result<crate::AppendResult, crate::error::PardosaError> {
        self.line.create(event)
    }
    /// Serialise the in-memory dragline into `.pgno` bytes and
    /// drive [`BackendSink::append`] + [`BackendSink::sync`],
    /// returning the post-fence [`AckPosition`].
    ///
    /// Buffers via [`std::io::Cursor`] over `Vec<u8>` so this
    /// path works for non-`Seek`/non-`File` backends. Framing
    /// matches [`PgnoFileSink`].
    ///
    /// # Errors
    ///
    /// [`SyncError::Persist`] when serialisation fails before
    /// any bytes reach the backend; [`SyncError::Backend`] with
    /// `op: BackendOp::Append` or `op: BackendOp::Sync` on
    /// substrate failure. The `op` discriminator distinguishes
    /// payload-retriable from fence-state failures (ADR-0022 §D7).
    pub(crate) fn sync(&mut self) -> Result<AckPosition, SyncError> {
        let mut buf: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        persist_with_source_append(&self.line, &mut buf, self.schema_source)?;
        let bytes = buf.into_inner();
        let _staged = self
            .backend
            .append(&bytes)
            .map_err(|source| SyncError::Backend {
                op: BackendOp::Append,
                source,
            })?;
        self.backend.sync().map_err(|source| SyncError::Backend {
            op: BackendOp::Sync,
            source,
        })
    }
}
impl<T, B> BackendDragline<T, B>
where
    T: Decode + GenomeSafe,
    B: BackendSink,
{
    /// Re-fold a `.pgno`-encoded byte slice (previously surfaced
    /// by a backend's `append`+`sync`) into a fresh
    /// [`BackendDragline<T, B>`].
    ///
    /// Reads only the post-`sync` durable prefix exposed by
    /// [`ReadableBackend::durable_bytes`] (ADR-0022 §D2). Wraps
    /// [`super::rehydrate::from_pgno_bytes_unchecked`] — framing
    /// checks match the `.pgno`/`File` open path. No filesystem
    /// access. Reader bound is `T: Decode + GenomeSafe` only
    /// (ADR-0020); validated counterpart lives separately.
    ///
    /// # Errors
    ///
    /// [`RehydrateError::Persist`] forwarding the
    /// [`PersistError`] taxonomy.
    pub(crate) fn rehydrate(backend: B) -> Result<Self, RehydrateError>
    where
        B: ReadableBackend,
    {
        let bytes = backend.durable_bytes();
        let line = super::rehydrate::from_pgno_bytes_unchecked::<T>(bytes)?;
        Ok(Self {
            line,
            backend,
            schema_source: None,
        })
    }
}
/// Reader-side companion to [`BackendSink`]: surface the
/// post-`sync` **durable prefix** as a borrow (ADR-0022 §D2
/// sync-as-fence).
///
/// Use for substrates whose log is already borrowable as `&[u8]`
/// (in-memory `Vec<u8>`, mmap'd `.pgno`). Substrates whose log is
/// remote use [`RehydrateableBackend`] instead.
///
/// **Sync-as-fence invariant:** bytes accepted by
/// [`BackendSink::append`] but not yet fenced by [`BackendSink::sync`]
/// MUST NOT appear here — replaying them would surface state the
/// substrate has not acknowledged as durable.
///
/// Sealed via [`super::sealed`]; in-crate impls only.
pub(crate) trait ReadableBackend: super::sealed::Sealed {
    /// Borrow the bytes the substrate has fenced via
    /// [`BackendSink::sync`] — the post-`sync` durable
    /// prefix only (ADR-0022 §D2 sync-as-fence).
    /// Implementations are pure observers — no I/O, no
    /// allocation.
    ///
    /// Staged-but-unsynced bytes (accepted via
    /// [`BackendSink::append`] without a subsequent `sync`)
    /// MUST be excluded; rehydrating from those bytes would
    /// replay state the substrate has not acknowledged as
    /// durable.
    fn durable_bytes(&self) -> &[u8];
}
#[cfg(any(test, feature = "test-support"))]
impl ReadableBackend for crate::authoritative::fake::InMemoryBackend {
    fn durable_bytes(&self) -> &[u8] {
        let end = usize::try_from(self.synced_to).expect("64-bit target enforced at crate root");
        &self.storage[..end]
    }
}
/// Fetch-shaped reader-side companion to [`BackendSink`]:
/// materialise the post-`sync` durable prefix as an owned
/// `Vec<u8>` (ADR-0022 §D2).
///
/// Use for substrates whose log lives remotely (`JetStream` messages,
/// object-store keys yet to be fetched). `&mut self` and
/// [`Result`] are honest at the type level: connection retries and
/// per-call state advancement are fallible.
///
/// Byte contract identical to [`ReadableBackend`] — only the
/// post-`sync` fenced prefix may appear; staged-but-unsynced bytes
/// MUST be excluded.
///
/// Sealed via [`super::sealed`]; in-crate impls only.
pub(crate) trait RehydrateableBackend: super::sealed::Sealed {
    /// Materialise the post-`sync` durable prefix as an owned
    /// `Vec<u8>` (ADR-0022 §D2).
    ///
    /// `&mut self` allows substrates to advance per-call state
    /// (open connections, position cursors, retry budgets).
    /// Staged-but-unsynced bytes MUST be excluded.
    ///
    /// # Errors
    ///
    /// [`BackendError`] propagating any substrate-side fetch
    /// failure (transport, encoding, missing-stream).
    fn fetch_durable_bytes(&mut self) -> Result<Vec<u8>, BackendError>;
}
#[cfg(any(test, feature = "test-support"))]
impl RehydrateableBackend for crate::authoritative::fake::InMemoryBackend {
    fn fetch_durable_bytes(&mut self) -> Result<Vec<u8>, BackendError> {
        let end = usize::try_from(self.synced_to).expect("64-bit target enforced at crate root");
        Ok(self.storage[..end].to_vec())
    }
}
impl<T, B> BackendDragline<T, B>
where
    T: Decode + GenomeSafe,
    B: BackendSink,
{
    /// Fetch-based counterpart to [`Self::rehydrate`]
    /// (ADR-0022 §D2). Same byte contract; transport differs —
    /// pulls bytes through
    /// [`RehydrateableBackend::fetch_durable_bytes`] (owned,
    /// fallible) rather than
    /// [`ReadableBackend::durable_bytes`] (borrowed).
    ///
    /// Use for remote substrates (`JetStream` messages,
    /// object-store keys). Reader bound is `T: Decode + GenomeSafe`
    /// (ADR-0020); validated counterpart lives separately.
    ///
    /// # Errors
    ///
    /// [`RehydrateError::Persist`] forwarding the
    /// [`PersistError`] taxonomy; [`RehydrateError::Backend`]
    /// for any substrate-side fetch failure.
    pub(crate) fn rehydrate_from(mut backend: B) -> Result<Self, RehydrateError>
    where
        B: RehydrateableBackend,
    {
        let bytes = backend
            .fetch_durable_bytes()
            .map_err(|source| RehydrateError::Backend {
                op: BackendOp::Sync,
                source,
            })?;
        let line = super::rehydrate::from_pgno_bytes_unchecked::<T>(&bytes)?;
        Ok(Self {
            line,
            backend,
            schema_source: None,
        })
    }
}
