//! Backend-author sealed-trait surface (ADR-0022 §D2 / §D11).
//!
//! [`BackendSink`] is the sealed substrate contract: backends drive
//! `append` + `sync` and return a backend-opaque [`crate::durability::AckPosition`].
//! Sealed via `sealed::Sealed`; in-crate impls only. [`PgnoFileSink`]
//! is the `.pgno`/[`std::fs::File`] impl.
//!
//! # Position vs. durability
//!
//! `sync` is the durability fence; `append` is *position* only.
//! Positions are monotonic within one backend instance and carry no
//! cross-backend meaning.
use crate::durability::AckPosition;
use crate::error::BackendError;
use std::io::Seek;
/// Private sealed-trait root for [`BackendSink`] (ADR-0022 §D2 / §D11).
///
/// Orthogonal to [`crate::authoritative::sealed::Sealed`]: admission
/// ([`crate::authoritative::AuthoritativeBackend`]) and behaviour
/// ([`BackendSink`]) are independent layers per ADR-0022, so they
/// seal under distinct private supertraits. A backend can be
/// admissible without being a sink implementor in principle.
mod sealed {
    pub trait Sealed {}
}
/// Backend-driven rehydrate seam (ADR-0022 §D2 reader-side companion
/// to [`BackendSink`]).
///
/// Byte-oriented (`&[u8]`) so a substrate that holds its log as
/// object-store blobs, `JetStream` payloads, or any non-`Seek` form
/// drives recovery through the same pipeline as the `.pgno`/`File`
/// adapter — no [`std::fs::File`] is opened.
///
/// `pub(crate)`: trybuild gates at
/// `tests/ui/no_generic_sink_on_event_store.rs` /
/// `no_external_backend_sink_impl.rs` /
/// `no_external_authoritative_backend_impl.rs` keep the surface closed.
pub(crate) mod rehydrate;
/// The sealed substrate contract: `append` stages event bytes;
/// `sync` fences durability. Both return [`AckPosition`]
/// (ADR-0022 §D2). Sealed via private supertrait; in-crate impls only
/// (ADR-0022 §D11). `append` is position-only; durability requires
/// a subsequent `sync`.
pub trait BackendSink: sealed::Sealed {
    /// Stage the supplied event bytes with the backend.
    ///
    /// Returns the [`AckPosition`] reached after the bytes have been
    /// accepted by the backend's append path. The bytes are not
    /// guaranteed durable until [`Self::sync`] returns a position
    /// at or beyond the value returned here (ADR-0022 §D2).
    ///
    /// # Errors
    ///
    /// [`BackendError::Publish`] when the underlying substrate
    /// rejects the write (transient class per ADR-0015). Other
    /// `BackendError` variants are permitted as the contract
    /// matures; the enum is `#[non_exhaustive]` per ADR-0007.
    fn append(&mut self, bytes: &[u8]) -> Result<AckPosition, BackendError>;
    /// Fence durability on the backend.
    ///
    /// Returns the [`AckPosition`] at which all bytes previously
    /// surfaced by [`Self::append`] (and any internal backend
    /// state preceding this call) are confirmed stable (ADR-0022
    /// §D2). For the in-crate `.pgno`/`File` adapter this is the
    /// `fsync`-fenced byte length of the file; for a future
    /// `JetStream` backend this is the last-known stable stream
    /// sequence.
    ///
    /// # Errors
    ///
    /// [`BackendError::Publish`] when the underlying substrate's
    /// durability fence fails.
    fn sync(&mut self) -> Result<AckPosition, BackendError>;
}
/// `.pgno`/[`std::fs::File`]-backed [`BackendSink`] (ADR-0022 §D11).
///
/// Generic over any [`pardosa_file::Syncable`] + [`Seek`] sink so
/// in-tree tests can drive it with `Cursor<Vec<u8>>`. Bytes flow
/// through `Syncable::Write` unchanged (ADR-0006); `sync` calls
/// [`pardosa_file::Syncable::sync_data`] for POSIX `fdatasync`
/// semantics (ADR-0010 §D3). [`AckPosition`] is derived from
/// [`Seek::stream_position`].
pub struct PgnoFileSink<W: pardosa_file::Syncable + Seek = std::fs::File> {
    inner: W,
}
impl<W> PgnoFileSink<W>
where
    W: pardosa_file::Syncable + Seek,
{
    /// Wrap the supplied `Syncable + Seek` sink as a
    /// `BackendSink`. The sink's initial position is preserved;
    /// callers wanting `.pgno`-shaped output should hand in a sink
    /// already positioned at byte 0 (mirrors the existing
    /// `Dragline::new` adopter contract).
    pub const fn new(inner: W) -> Self {
        Self { inner }
    }
}
impl<W> sealed::Sealed for PgnoFileSink<W> where W: pardosa_file::Syncable + Seek {}
impl<W> BackendSink for PgnoFileSink<W>
where
    W: pardosa_file::Syncable + Seek,
{
    fn append(&mut self, bytes: &[u8]) -> Result<AckPosition, BackendError> {
        self.inner
            .write_all(bytes)
            .map_err(|e| BackendError::Publish {
                source: Box::new(e),
            })?;
        let pos = self
            .inner
            .stream_position()
            .map_err(|e| BackendError::Publish {
                source: Box::new(e),
            })?;
        Ok(AckPosition::from_u64(pos))
    }
    fn sync(&mut self) -> Result<AckPosition, BackendError> {
        pardosa_file::Syncable::sync_data(&mut self.inner).map_err(|e| BackendError::Publish {
            source: Box::new(e),
        })?;
        let pos = self
            .inner
            .stream_position()
            .map_err(|e| BackendError::Publish {
                source: Box::new(e),
            })?;
        Ok(AckPosition::from_u64(pos))
    }
}
/// `BackendSink` impl for the cfg-gated in-memory fake in
/// [`crate::authoritative::fake`] (ADR-0022 §D11). Split across two
/// source files so each fake reaches its own file-private sealing
/// supertrait without widening either.
///
/// `append(bytes)` extends storage and returns the post-write
/// position; `sync()` advances the internal durability floor to the
/// current staged length. No-op `sync` is idempotent.
#[cfg(any(test, feature = "test-support"))]
pub(crate) mod fake;
/// `BackendSink` impl for the in-crate `JetStream` adapter shim in
/// [`crate::authoritative::jetstream`] (ADR-0022 §D11). Split across
/// two source files so each `super::sealed::Sealed` stays
/// file-private (mirrors [`fake`] above).
///
/// `append`/`sync` delegate to [`pardosa_nats::JetStreamHandle`];
/// `JetStreamAckPosition` maps to [`AckPosition`] and
/// `JetStreamRuntimeError` maps to [`BackendError`] per
/// ADR-0022 §D7. Mapping table pinned by the in-module `tests`.
pub(crate) mod jetstream;
/// Internal backend-backed journal composition (ADR-0022 §D2 / §D11).
///
/// [`BackendDragline<T, B>`](journal::BackendDragline) composes an
/// in-memory [`Line<T>`](crate::dragline::Line) with a sealed
/// [`BackendSink`] so the runtime can drive `commit_event`,
/// `sync` (`.pgno`-serialise → `backend.append` → `backend.sync`),
/// and `rehydrate` through any in-crate sink — no filesystem access
/// on the byte-only path.
///
/// All items are `pub(crate)`; trybuild gates at
/// `tests/ui/no_generic_sink_on_event_store.rs` /
/// `no_external_backend_sink_impl.rs` /
/// `no_external_authoritative_backend_impl.rs` keep the surface closed.
pub(crate) mod journal;
/// Adopter-test wrapper exposing a JetStream-authoritative recovery
/// journal without widening `BackendDragline` or
/// `JetStreamBackendAdapter` out of `pub(crate)` (ADR-0022 §D11).
///
/// Cfg-gated on `cfg(any(test, feature = "test-support"))` so it
/// does not enlarge the production-build public surface. Surface is
/// bounded to `commit` / `sync` / `rehydrate` /
/// `read_line_event_payloads` — what adopter-side recovery soaks
/// need and nothing more. Trybuild gates
/// (`tests/ui/no_*_sink_*.rs`) stay green: this is a parallel
/// test-only journal type, not an `EventStore<T, B>` overload.
#[cfg(any(test, feature = "test-support"))]
pub(crate) mod test_support_jetstream_recovery;
#[cfg(test)]
mod tests;
