use super::journal::{BackendDragline, RehydrateError, SyncError};
use crate::authoritative::jetstream::JetStreamBackendAdapter;
use crate::durability::AckPosition;
use crate::error::PardosaError;
use crate::event::EventId;
use pardosa_nats::JetStreamHandle;
use pardosa_schema::GenomeSafe;
use pardosa_wire::{Decode, Encode};
/// Reason a [`JetStreamRecoveryJournal::sync`] could not
/// complete (ADR-0007 in-crate error-taxonomy convention;
/// `#[non_exhaustive]` for forward-compatibility).
///
/// Holds the underlying in-crate `SyncError`'s
/// [`core::fmt::Display`] / [`std::error::Error::source`]
/// chain as a captured message string so the wrapper does
/// not widen the in-crate `SyncError` enum's visibility.
/// Test consumers compare via `Display` / `to_string`; the
/// structural enum is not part of the adopter-test contract.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum JetStreamRecoveryJournalSyncError {
    /// Underlying `BackendDragline::sync` failure, captured as
    /// a `Display`-rendered message.
    #[error("JetStream recovery journal sync failed: {message}")]
    Inner {
        /// `Display`-rendered substrate-side message
        /// (`SyncError::Persist` or `SyncError::Backend`).
        message: String,
    },
}
impl JetStreamRecoveryJournalSyncError {
    fn from_inner(e: &SyncError) -> Self {
        Self::Inner {
            message: e.to_string(),
        }
    }
}
/// Reason a [`JetStreamRecoveryJournal::rehydrate`] could not
/// complete (ADR-0007 in-crate error-taxonomy convention;
/// `#[non_exhaustive]` for forward-compatibility).
///
/// Same wrapping rationale as
/// [`JetStreamRecoveryJournalSyncError`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum JetStreamRecoveryJournalRehydrateError {
    /// Underlying `BackendDragline::rehydrate_from` failure,
    /// captured as a `Display`-rendered message.
    #[error("JetStream recovery journal rehydrate failed: {message}")]
    Inner {
        /// `Display`-rendered substrate-side message
        /// (`RehydrateError::Persist` or
        /// `RehydrateError::Backend`).
        message: String,
    },
}
impl JetStreamRecoveryJournalRehydrateError {
    fn from_inner(e: &RehydrateError) -> Self {
        Self::Inner {
            message: e.to_string(),
        }
    }
}
/// Adopter-test surface for a JetStream-authoritative recovery
/// journal (mission `nats-followups-jetstream-open-06`).
///
/// Hides
/// `super::journal::BackendDragline<T, JetStreamBackendAdapter>`
/// behind a public-typed handle exposing only:
/// [`Self::commit`], [`Self::sync`], [`Self::rehydrate`], and
/// [`Self::read_line_event_payloads`]. The wrapped
/// `BackendDragline` is private; callers cannot reach
/// `JetStreamBackendAdapter` directly.
pub struct JetStreamRecoveryJournal<T> {
    inner: BackendDragline<T, JetStreamBackendAdapter>,
}
/// Construct a [`JetStreamRecoveryJournal<T>`] from an opaque
/// [`pardosa_nats::JetStreamHandle`] minted by
/// [`pardosa_nats::JetStreamBackend::open`].
///
/// No I/O at construction time: the wrapped in-crate
/// `JetStreamBackendAdapter` stores the handle and defers all
/// network activity to the first
/// [`JetStreamRecoveryJournal::sync`] (mirroring the substrate's
/// lazy-connect contract — `pardosa_nats::JetStreamHandle`
/// lazy-connects on the first `append`).
#[must_use]
pub fn jetstream_recovery_journal<T>(handle: JetStreamHandle) -> JetStreamRecoveryJournal<T> {
    let adapter = JetStreamBackendAdapter::new(handle);
    JetStreamRecoveryJournal {
        inner: BackendDragline::new(adapter),
    }
}
impl<T> JetStreamRecoveryJournal<T>
where
    T: Encode + GenomeSafe,
{
    /// Append `event` to the in-memory dragline. The event is
    /// in-memory only until a subsequent [`Self::sync`] succeeds
    /// (ADR-0010 / ADR-0022 §D2 — `append` does not imply
    /// durability; `sync` is the fence).
    ///
    /// Returns the minted [`EventId`] (the public event-identity
    /// type re-exported via `pardosa::store`); the structural
    /// `AppendResult` is in-crate.
    ///
    /// # Errors
    ///
    /// Forwards [`PardosaError`] from the underlying
    /// `BackendDragline::commit_event`.
    pub fn commit(&mut self, event: T) -> Result<EventId, PardosaError> {
        self.inner.commit_event(event).map(|ar| ar.event_id)
    }
    /// Serialise the in-memory dragline into a `.pgno`-shaped
    /// byte blob and drive the `JetStream` substrate's
    /// publish + sync pair via the sealed
    /// [`super::BackendSink`] dispatch on the in-crate
    /// `JetStreamBackendAdapter`.
    ///
    /// Returns the post-fence [`AckPosition`] surfaced by the
    /// substrate (the `PubAck.seq` from the underlying
    /// [`pardosa_nats::JetStreamHandle::append`]). Each
    /// sync re-publishes the **complete** current dragline as
    /// one `JetStream` message — a recovery-side reader picks
    /// the most-recent message to reconstruct the latest
    /// state.
    ///
    /// # Errors
    ///
    /// [`JetStreamRecoveryJournalSyncError::Inner`] wrapping
    /// the underlying substrate-composition `SyncError`'s
    /// `Display` message (`.pgno` serialisation or `JetStream`
    /// publish/sync dispatch failure).
    pub fn sync(&mut self) -> Result<AckPosition, JetStreamRecoveryJournalSyncError> {
        self.inner
            .sync()
            .map_err(|e| JetStreamRecoveryJournalSyncError::from_inner(&e))
    }
}
impl<T> JetStreamRecoveryJournal<T>
where
    T: Decode + GenomeSafe,
{
    /// Reopen the `JetStream` substrate identified by `handle` and
    /// rehydrate the dragline from the most-recent sync-fenced
    /// `.pgno` blob via
    /// [`pardosa_nats::JetStreamHandle::replay_all`] (ADR-0022 §D2).
    ///
    /// `handle` must point at the same stream the writer's
    /// handle wrote to. Each [`Self::sync`] writes one complete
    /// `.pgno` blob; the reader picks the last record.
    ///
    /// # Errors
    ///
    /// [`JetStreamRecoveryJournalRehydrateError::Inner`] wrapping
    /// `.pgno` decode failure or `JetStream` replay dispatch failure.
    pub fn rehydrate(
        handle: JetStreamHandle,
    ) -> Result<Self, JetStreamRecoveryJournalRehydrateError> {
        let adapter = JetStreamBackendAdapter::new(handle);
        BackendDragline::rehydrate_from(adapter)
            .map(|inner| Self { inner })
            .map_err(|e| JetStreamRecoveryJournalRehydrateError::from_inner(&e))
    }
}
impl<T> JetStreamRecoveryJournal<T>
where
    T: Clone,
{
    /// Borrow each recovered event payload in commit order
    /// (cloned because [`crate::Event`] exposes the payload
    /// through a borrow accessor and the test surface owns
    /// the materialised payload vector).
    #[must_use]
    pub fn read_line_event_payloads(&self) -> Vec<T> {
        self.inner
            .line()
            .read_line()
            .iter()
            .map(|e| e.domain_event().clone())
            .collect()
    }
}
