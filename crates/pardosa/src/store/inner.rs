use super::{
    AppendReceipt, Decode, DetachReceipt, DetachedFiber, Encode, Event, EventId, FiberId,
    FiberState, Frontier, FrontierPublisher, GenomeSafe, Index, LiveFiber, Lsn, PardosaError, Path,
    PathBuf, Precursor, Syncable, Validate, ValidatedReplayError,
};
use crate::cursor::{Cursor, JournalCursor};
use crate::dragline::Dragline;
use std::io::Seek;
mod causal;
mod history;
mod lifecycle;
pub use causal::{CausalChain, CausalChainError, CausalChainIter, CausalChainStrictIter};
pub use history::{FiberHistory, FiberHistoryIter, HistoryStream};
pub use lifecycle::StoreMetadata;
/// Adopter-facing typed event-log appliance (ADR-0018 §D1).
///
/// Adopter form is path-backed `EventStore<T>` (`W = File`) via
/// [`create`](Self::create) and
/// [`open_validated`](Self::open_validated) (the unchecked `open`
/// is `pub(crate)` on default features; only `feature =
/// "test-support"` re-exposes it as `pub` for in-tree parity
/// tests). Path retained so
/// [`StoreReader::cursor`] takes only sidecar (Amendment 2).
/// Capability split (§D2): reader has no authoring method.
/// `open` does not auto-migrate (§D7).
///
/// # Examples
///
/// ```no_run
/// use pardosa::store::{EventStore, GenomeSafe, HasEventSchemaSource};
///
/// #[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
/// struct MyEvent { v: u64 }
/// impl HasEventSchemaSource for MyEvent {
///     const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
/// }
///
/// let path = std::path::Path::new("/tmp/my-log.pgno");
/// let mut store = EventStore::<MyEvent>::create(path).expect("create");
/// let _ = store.writer().begin(MyEvent { v: 1 }).expect("begin");
/// store.writer().sync().expect("sync");
/// ```
pub struct EventStore<T, W: Syncable + Seek = std::fs::File> {
    inner: Dragline<T, W>,
    journal: PathBuf,
    schema_source: Option<&'static str>,
}
impl<T, W> EventStore<T, W>
where
    W: Syncable + Seek,
{
    /// Borrow a [`StoreReader`] with read-only capability over the
    /// underlying log.
    #[must_use]
    pub fn reader(&self) -> StoreReader<'_, T, W> {
        StoreReader {
            log: &self.inner,
            journal: &self.journal,
        }
    }
}
impl<T, W> EventStore<T, W>
where
    T: Encode + GenomeSafe,
    W: Syncable + Seek,
{
    /// Borrow a [`StoreWriter`] with authoring authority over the
    /// underlying log.
    ///
    /// Mutable borrow: only one writer handle may exist at a time.
    /// Bound `T: Encode + GenomeSafe` mirrors the writer side of
    /// the crate-internal journal substrate.
    pub fn writer(&mut self) -> StoreWriter<'_, T, W> {
        StoreWriter {
            log: &mut self.inner,
            journal: &self.journal,
            schema_source: self.schema_source,
        }
    }
}
/// Writer-capability handle borrowed from an [`EventStore`].
///
/// Payload-only authoring surface (ADR-0018 §D4): adopters pass
/// payload `T`; Pardosa mints `EventId`, `FiberId`, precursor
/// pointer, and live/detached state flag.
///
/// Fiber-lifecycle verbs: [`begin`](Self::begin),
/// [`append`](Self::append), [`detach`](Self::detach),
/// [`resume`](Self::resume), [`resume_defined`](Self::resume_defined),
/// [`rescue_detached`](Self::rescue_detached). Illegal token transitions
/// remain unrepresentable at the type layer (ADR-0017 §D1); identity-resume
/// verbs admit `FiberId` only after dragline-state validation (PGN-0014).
pub struct StoreWriter<'a, T, W: Syncable + Seek = std::fs::File> {
    log: &'a mut Dragline<T, W>,
    journal: &'a Path,
    schema_source: Option<&'static str>,
}
impl<T, W> StoreWriter<'_, T, W>
where
    T: Encode + GenomeSafe,
    W: Syncable + Seek,
{
    /// Open a new fiber and append its first event.
    ///
    /// Adopter-facing fiber-lifecycle verb (ADR-0018 §D4). Returns
    /// an [`AppendReceipt`] carrying the newly minted [`EventId`]
    /// and a [`LiveFiber`] token authorising further
    /// [`append`](Self::append) or [`detach`](Self::detach) calls
    /// on the same fiber.
    ///
    /// # Errors
    ///
    /// Forwards any [`PardosaError`] from the commit pipeline.
    pub fn begin(&mut self, event: T) -> Result<AppendReceipt, PardosaError> {
        let ar = self.log.commit_event(event)?;
        Ok(AppendReceipt {
            event_id: ar.event_id,
            fiber: LiveFiber(ar.fiber_id),
        })
    }
    /// Append a continuation event to a live fiber.
    ///
    /// Adopter-facing fiber-lifecycle verb (ADR-0018 §D4). The
    /// typestate boundary is preserved — [`DetachedFiber`] does
    /// not coerce to [`LiveFiber`], so a detached fiber cannot be
    /// silently continued via this entry; use
    /// [`resume`](Self::resume).
    ///
    /// # Errors
    ///
    /// Forwards any [`PardosaError`] from the commit pipeline.
    #[allow(clippy::needless_pass_by_value)]
    pub fn append(&mut self, fiber: LiveFiber, event: T) -> Result<AppendReceipt, PardosaError> {
        let ar = self.log.commit_update(fiber.0, event)?;
        Ok(AppendReceipt {
            event_id: ar.event_id,
            fiber: LiveFiber(ar.fiber_id),
        })
    }
    /// Detach a live fiber.
    ///
    /// Adopter-facing fiber-lifecycle verb (ADR-0018 §D4). Consumes
    /// a [`LiveFiber`] and returns a [`DetachReceipt`] carrying a
    /// [`DetachedFiber`] token authorising a subsequent
    /// [`resume`](Self::resume) on the same fiber.
    ///
    /// # Errors
    ///
    /// Forwards any [`PardosaError`] from the commit pipeline.
    #[allow(clippy::needless_pass_by_value)]
    pub fn detach(&mut self, fiber: LiveFiber, event: T) -> Result<DetachReceipt, PardosaError> {
        let ar = self.log.commit_detach(fiber.0, event)?;
        Ok(DetachReceipt {
            event_id: ar.event_id,
            fiber: DetachedFiber(ar.fiber_id),
        })
    }
    /// Resume a detached fiber.
    ///
    /// Adopter-facing fiber-lifecycle verb (ADR-0018 §D4). Consumes
    /// a [`DetachedFiber`] and returns an [`AppendReceipt`] carrying
    /// the rescue event's [`EventId`] and a fresh [`LiveFiber`]
    /// token.
    ///
    /// # Errors
    ///
    /// Forwards any [`PardosaError`] from the commit pipeline.
    #[allow(clippy::needless_pass_by_value)]
    pub fn resume(
        &mut self,
        fiber: DetachedFiber,
        event: T,
    ) -> Result<AppendReceipt, PardosaError> {
        let ar = self.log.commit_rescue(fiber.0, event)?;
        Ok(AppendReceipt {
            event_id: ar.event_id,
            fiber: LiveFiber(ar.fiber_id),
        })
    }
    /// Append to a rehydrated `Defined` fiber by validated identity.
    ///
    /// Admits the supplied [`FiberId`] only after checking the current dragline
    /// state. This is the write-side mirror of the `Defined`-filtered read
    /// predicate ratified by PGN-0014; the method mints a fresh [`LiveFiber`]
    /// only after validation succeeds.
    ///
    /// # Errors
    ///
    /// Returns [`PardosaError::FiberNotFound`] when the id is absent.
    /// Returns [`PardosaError::InvalidTransition`] when the id is present but
    /// not `Defined`. Otherwise forwards commit-pipeline errors.
    pub fn resume_defined(
        &mut self,
        fiber_id: FiberId,
        event: T,
    ) -> Result<AppendReceipt, PardosaError> {
        match self.log.fiber_state(fiber_id) {
            FiberState::Undefined | FiberState::Purged => {
                Err(PardosaError::FiberNotFound(fiber_id))
            }
            FiberState::Defined => {
                let ar = self.log.commit_update(fiber_id, event)?;
                Ok(AppendReceipt {
                    event_id: ar.event_id,
                    fiber: LiveFiber(ar.fiber_id),
                })
            }
            state => Err(PardosaError::InvalidTransition {
                state,
                action: crate::fiber_state::FiberAction::Update,
            }),
        }
    }
    /// Rescue a rehydrated `Detached` fiber by validated identity.
    ///
    /// Admits the supplied [`FiberId`] only after checking the current dragline
    /// state is `Detached`, then drives the existing rescue commit path to mint
    /// a fresh [`LiveFiber`].
    ///
    /// # Errors
    ///
    /// Returns [`PardosaError::FiberNotFound`] when the id is absent.
    /// Returns [`PardosaError::InvalidTransition`] when the id is present but
    /// not `Detached`. Otherwise forwards commit-pipeline errors.
    pub fn rescue_detached(
        &mut self,
        fiber_id: FiberId,
        event: T,
    ) -> Result<AppendReceipt, PardosaError> {
        match self.log.fiber_state(fiber_id) {
            FiberState::Undefined | FiberState::Purged => {
                Err(PardosaError::FiberNotFound(fiber_id))
            }
            FiberState::Detached => {
                let ar = self.log.commit_rescue(fiber_id, event)?;
                Ok(AppendReceipt {
                    event_id: ar.event_id,
                    fiber: LiveFiber(ar.fiber_id),
                })
            }
            state => Err(PardosaError::InvalidTransition {
                state,
                action: crate::fiber_state::FiberAction::Rescue,
            }),
        }
    }
    /// Persist all in-memory events to the sink and fence on
    /// durability.
    ///
    /// Rewinds to byte 0, writes a complete `.pgno` container,
    /// calls [`Syncable::sync_data`], returns the post-fence byte
    /// length as an [`Lsn`] (ADR-0010 §D5). When the store was
    /// constructed via [`EventStore::create`] with a non-`None`
    /// [`super::HasEventSchemaSource::EVENT_SCHEMA_SOURCE`], that
    /// descriptor is embedded on each sync.
    ///
    /// # Errors
    ///
    /// [`crate::persist::Error`] on sink I/O, `set_len`, or
    /// `sync_data` failure. The in-memory dragline is not rolled
    /// back; callers may retry or drop the store.
    pub fn sync(&mut self) -> Result<Lsn, crate::persist::Error> {
        self.log.sync_data_with_source(self.schema_source)
    }
    /// Last `sync`-acknowledged byte length, or `None` if `sync`
    /// has never been called on this store.
    #[must_use]
    pub fn acked_lsn(&self) -> Option<Lsn> {
        self.log.acked_lsn()
    }
    /// Read-side accessor: the writer transitively has read
    /// capability (ADR-0016 §D1 one-directional re-export).
    /// Returns a [`StoreReader`] borrowing the same underlying log.
    #[must_use]
    pub fn reader(&self) -> StoreReader<'_, T, W> {
        StoreReader {
            log: self.log,
            journal: self.journal,
        }
    }
}
/// Reader-capability handle borrowed from an [`EventStore`] or
/// [`StoreWriter::reader`].
///
/// Holds an immutable borrow of the underlying substrate
/// `EventLog`; cannot name any authoring method and cannot coerce
/// to a writer (ADR-0018 §D2). The three read views map to three
/// different primitives (see module docs).
///
/// Adopters name `StoreReader<'_, T>` — `W` defaults to
/// [`std::fs::File`] (ADR-0018 § Naming). Generic-`W` is retained
/// for the in-memory test substrate.
pub struct StoreReader<'a, T, W: Syncable + Seek = std::fs::File> {
    log: &'a Dragline<T, W>,
    journal: &'a Path,
}
impl<T, W> StoreReader<'_, T, W>
where
    W: Syncable + Seek,
{
    /// Per-fiber history view over the in-memory dragline
    /// (ADR-0018 §D3 (a)).
    ///
    /// `FiberId` is dragline-local runtime replay identity
    /// (ADR-0003 §1); this is not a domain-identity lookup. No
    /// sidecar, no ACK, no I/O.
    #[must_use]
    pub fn fiber(&self, id: FiberId) -> FiberHistory<'_, T, W> {
        FiberHistory { log: self.log, id }
    }
    /// Rolling BLAKE3 chain-frontier over the full event line
    /// (ADR-0018 §D3 observability; ADR-0004 § Security model).
    ///
    /// Returns [`Frontier::GENESIS`] for an empty store and
    /// advances after every successful append. Survives
    /// [`StoreWriter::sync`] + reopen (both [`EventStore::open_validated`]
    /// and the cfg-gated unchecked [`EventStore::open`] re-fold
    /// frontier from the persisted line).
    ///
    /// Read-only; no I/O. Mutation-detection vs. a trusted anchor,
    /// not authentication (ADR-0004 § Security model). Tamper
    /// detection requires an out-of-band anchor.
    #[must_use]
    pub fn frontier(&self) -> Frontier {
        self.log.frontier()
    }
    /// Last `EventId` that has been successfully published downstream
    /// *and* durably recorded in the publish-watermark sidecar
    /// (ADR-0016 §§D5–D7; ADR-0018 §D3 observability).
    ///
    /// `None` when the store was opened without a publisher
    /// (via [`EventStore::open_validated`]). When a publisher is attached
    /// ([`EventStore::open_with_publisher`]), the value is
    /// `Some(last_durably_published_event_id)`. Recovered from the
    /// fsync-ed sidecar across crash + reopen without publish
    /// activity (ADR-0016 §D6/§D7).
    ///
    /// Read-only; no I/O. The on-disk sidecar is the source of truth.
    #[must_use]
    pub fn publish_watermark(&self) -> Option<EventId> {
        self.log.publish_watermark()
    }
    /// Same-fiber checked-replay walk within the current dragline
    /// (ADR-0018 §D3 (b)).
    ///
    /// Walks precursor pointers (ADR-0003 §3) along **one fiber**.
    /// A `Precursor::Genesis` or a precursor pointing outside the
    /// current dragline / outside the head's fiber terminates the
    /// chain without error (ADR-0018 §D6). Cross-fiber and
    /// cross-dragline causality are payload/schema concerns, not
    /// substrate concerns.
    #[must_use]
    pub fn causal_chain(&self, head: EventId) -> CausalChain<'_, T, W> {
        CausalChain {
            log: self.log,
            head,
        }
    }
    /// Build a [`FiberIndex<K>`] over the current in-memory
    /// dragline by replaying every event through the closure-first
    /// zero-to-many `extractor` (ADR-0023 D1 / D5 opt-in
    /// construction).
    ///
    /// The index is in-memory only, log-derived, and bounded by
    /// the [`StoreReader`]'s borrow (D1 drop-on-close). The log is
    /// the sole durability boundary (D2): the index reflects every
    /// event currently visible to the reader, and nothing more.
    /// `K` is application-owned and opaque to pardosa (D6) —
    /// pardosa does not encode `K` into `.pgno` bytes or any
    /// sidecar.
    #[must_use]
    pub fn fiber_index<K, F, I>(&self, extractor: F) -> crate::FiberIndex<K>
    where
        K: std::hash::Hash + Eq + Clone,
        F: Fn(&Event<T>) -> I,
        I: IntoIterator<Item = K>,
    {
        let view = self.log.reader_view();
        crate::FiberIndex::build(view.read_line(), extractor)
    }
    /// Fallible variant of [`StoreReader::fiber_index`]
    /// (ADR-0023 D4 extractor error surface).
    ///
    /// Stops at the first extractor `Err`; the partially-built
    /// index is discarded (no silent partial state).
    ///
    /// # Errors
    ///
    /// [`crate::ExtractError::Extractor`] when the extractor
    /// returns `Err` for any event currently visible to the
    /// reader.
    pub fn try_fiber_index<K, F, I, E>(
        &self,
        extractor: F,
    ) -> Result<crate::FiberIndex<K>, crate::ExtractError>
    where
        K: std::hash::Hash + Eq + Clone,
        F: Fn(&Event<T>) -> Result<I, E>,
        I: IntoIterator<Item = K>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let view = self.log.reader_view();
        crate::FiberIndex::try_build(view.read_line(), extractor)
    }
}
impl<T> StoreReader<'_, T, std::fs::File>
where
    T: Decode + GenomeSafe,
{
    /// Global consumer ACK/resume cursor over the event line
    /// (ADR-0018 §D3 (c)/§D5, Amendment 2).
    ///
    /// Sidecar-backed; exclusive on resume (ADR-0011 §D2/§D5).
    /// Acknowledgement is event-oriented:
    /// [`LineCursor::commit_consumed`] advances the sidecar with
    /// one fsync per accepted call, taking the `&Event<T>` the
    /// adopter just processed. Not fiber history. Path retained
    /// by constructors, so adopters pass only `sidecar`.
    ///
    /// # Errors
    ///
    /// Returns [`PardosaError`] from the sidecar/journal open
    /// path. Bound is `T: Decode + GenomeSafe`, matching
    /// `persist::stream_checked` (ADR-0018 §D1).
    pub fn cursor(&self, sidecar: &Path) -> Result<LineCursor<T>, PardosaError> {
        let inner = JournalCursor::<std::fs::File, T>::from_path(self.journal, sidecar)?;
        Ok(LineCursor { inner })
    }
}
/// Global consumer ACK/resume cursor returned by
/// `StoreReader::cursor` (ADR-0018 §D3 (c) / §D5).
///
/// Sidecar-backed by the crate-internal `JournalCursor` sidecar
/// path. [`tail`](Self::tail) yields every event on the line in
/// commit order (regardless of fiber).
/// [`commit_consumed`](Self::commit_consumed) advances the sidecar
/// to the supplied `&Event<T>` (ADR-0011 §D5); a subsequent reopen
/// via `StoreReader::cursor` with the same sidecar path resumes
/// exclusively after the committed event (ADR-0011 §D2).
/// Acknowledgement is event-oriented at the public surface;
/// offset-typed commits are an internal helper.
pub struct LineCursor<T> {
    inner: JournalCursor<std::fs::File, T>,
}
impl<T> LineCursor<T>
where
    T: Decode + GenomeSafe,
{
    /// Resume iteration over the line, skipping events already
    /// covered by [`acked_offset`](Self::acked_offset). Each
    /// yielded item is a fallible [`Event<T>`].
    pub fn tail(&mut self) -> impl Iterator<Item = Result<Event<T>, PardosaError>> + '_ {
        self.inner.tail()
    }
    pub(crate) fn commit_offset(&mut self, id: EventId) -> Result<(), PardosaError> {
        self.inner.commit_offset(id)
    }
    /// Advance the acked watermark to `event.event_id()`.
    ///
    /// Adopter-facing acknowledgement verb: the consumer hands
    /// back the `&Event<T>` it just processed and the sidecar
    /// fsync-advances to that event's id. Monotonic per
    /// ADR-0011 §D2 / §D8; one sidecar fsync per accepted call.
    /// Stale or beyond-line ids are no-ops, not errors.
    ///
    /// # Errors
    ///
    /// [`PardosaError::CursorSidecar`] when sidecar truncate-write
    /// or `fsync` fails.
    pub fn commit_consumed(&mut self, event: &Event<T>) -> Result<(), PardosaError> {
        self.commit_offset(event.event_id())
    }
    /// Advance the acked watermark to `id` directly.
    ///
    /// Ergonomic variant of
    /// [`commit_consumed`](Self::commit_consumed) when the caller
    /// already holds the [`EventId`] (e.g. from a
    /// [`super::AppendReceipt`]).
    ///
    /// Same semantics: monotonic (ADR-0011 §D2 / §D8); one sidecar
    /// fsync per accepted call; stale or beyond-line ids no-op.
    ///
    /// # Errors
    ///
    /// [`PardosaError::CursorSidecar`] when sidecar
    /// truncate-write or `fsync` fails.
    pub fn commit_consumed_id(&mut self, id: EventId) -> Result<(), PardosaError> {
        self.commit_offset(id)
    }
    /// The most recently committed offset, or `None` if no commit
    /// has occurred against this cursor and the sidecar held no
    /// prior offset.
    #[must_use]
    pub fn acked_offset(&self) -> Option<EventId> {
        self.inner.acked_offset()
    }
}
#[cfg(test)]
mod fiber_index_integration_tests {
    use super::*;
    use crate::fiber_index::FiberLookup;
    fn empty_store() -> EventStore<u64, std::io::Cursor<Vec<u8>>> {
        let sink = std::io::Cursor::new(Vec::<u8>::new());
        let inner = Dragline::new(sink);
        EventStore {
            inner,
            journal: PathBuf::from("/tmp/fiber_index_integration"),
            schema_source: None,
        }
    }
    fn extract_mod3(e: &Event<u64>) -> std::iter::Once<u64> {
        std::iter::once(*e.domain_event() % 3)
    }
    #[test]
    fn empty_log_yields_empty_index() {
        let store = empty_store();
        let idx = store.reader().fiber_index(extract_mod3);
        assert_eq!(idx.key_count(), 0);
        assert_eq!(idx.lookup(&7u64), FiberLookup::Empty);
    }
    #[test]
    fn build_from_log_reflects_all_committed_events() {
        let mut store = empty_store();
        let _ = store.writer().begin(10u64).expect("begin");
        let _ = store.writer().begin(11u64).expect("begin");
        let idx = store.reader().fiber_index(extract_mod3);
        assert_eq!(idx.key_count(), 2);
        assert!(matches!(idx.lookup(&1u64), FiberLookup::Unique(_)));
        assert!(matches!(idx.lookup(&2u64), FiberLookup::Unique(_)));
    }
    #[test]
    fn divergence_surfaces_when_two_fibers_share_key() {
        let mut store = empty_store();
        let _ = store.writer().begin(3u64).expect("begin a");
        let _ = store.writer().begin(6u64).expect("begin b");
        let idx = store.reader().fiber_index(extract_mod3);
        match idx.lookup(&0u64) {
            FiberLookup::Diverged { fibers } => {
                assert_eq!(fibers.len(), 2);
            }
            other => panic!("expected Diverged, got {other:?}"),
        }
    }
    #[test]
    fn append_after_divergence_does_not_fail_at_the_writer() {
        let mut store = empty_store();
        let _ = store.writer().begin(3u64).expect("first begin");
        let _ = store
            .writer()
            .begin(6u64)
            .expect("second begin must not refuse on key divergence");
        let receipt = store
            .writer()
            .begin(9u64)
            .expect("third begin must succeed; D4 forbids substrate refusing on K");
        let _ = receipt.event_id();
    }
    #[test]
    fn rebuild_after_sync_is_deterministic() {
        let mut store = empty_store();
        for n in 0..4u64 {
            let _ = store.writer().begin(n).expect("begin");
        }
        let _ = store.writer().sync().expect("sync");
        let idx_a = store.reader().fiber_index(extract_mod3);
        let idx_b = store.reader().fiber_index(extract_mod3);
        for k in 0..3u64 {
            assert_eq!(idx_a.lookup(&k), idx_b.lookup(&k));
        }
        assert_eq!(idx_a.key_count(), idx_b.key_count());
    }
    #[test]
    fn zero_to_many_extractor_emits_one_mapping_per_k() {
        let mut store = empty_store();
        let _ = store.writer().begin(3u64).expect("begin");
        let idx = store
            .reader()
            .fiber_index(|e: &Event<u64>| (0..*e.domain_event()).collect::<Vec<u64>>());
        assert_eq!(idx.key_count(), 3);
        for k in 0..3u64 {
            assert!(matches!(idx.lookup(&k), FiberLookup::Unique(_)));
        }
    }
    #[test]
    fn separate_extractors_yield_independent_indices() {
        let mut store = empty_store();
        let _ = store.writer().begin(10u64).expect("begin");
        let idx_mod3 = store.reader().fiber_index(extract_mod3);
        let idx_double = store
            .reader()
            .fiber_index(|e: &Event<u64>| std::iter::once(*e.domain_event() * 2));
        assert!(matches!(idx_mod3.lookup(&1u64), FiberLookup::Unique(_)));
        assert!(matches!(idx_double.lookup(&20u64), FiberLookup::Unique(_)));
        assert_eq!(idx_mod3.lookup(&20u64), FiberLookup::Empty);
        assert_eq!(idx_double.lookup(&1u64), FiberLookup::Empty);
    }
    #[test]
    fn try_fiber_index_propagates_extractor_error() {
        use crate::ExtractError;
        #[derive(Debug, thiserror::Error)]
        #[error("reject")]
        struct Bad;
        let mut store = empty_store();
        let _ = store.writer().begin(7u64).expect("begin");
        let _ = store.writer().begin(13u64).expect("begin");
        let result: Result<crate::FiberIndex<u64>, ExtractError> =
            store.reader().try_fiber_index(|e: &Event<u64>| {
                if *e.domain_event() == 13 {
                    Err(Bad)
                } else {
                    Ok(std::iter::once(*e.domain_event()))
                }
            });
        let err = result.expect_err("must propagate");
        assert!(matches!(err, ExtractError::Extractor { .. }));
    }
    #[test]
    fn append_side_observe_keeps_in_memory_index_in_log_order() {
        let mut store = empty_store();
        let mut idx: crate::FiberIndex<u64> = crate::FiberIndex::empty();
        let r1 = store.writer().begin(3u64).expect("begin a");
        let fid_a = r1.fiber().fiber_id();
        let _ = store.writer().begin(6u64).expect("begin b");
        let line_full = store.reader().fiber_index(extract_mod3);
        match line_full.lookup(&0u64) {
            FiberLookup::Diverged { fibers } => {
                assert_eq!(fibers[0], fid_a);
                let events = store.reader().raw_events_for_test();
                idx.observe(events[0], extract_mod3);
                idx.observe(events[1], extract_mod3);
                match idx.lookup(&0u64) {
                    FiberLookup::Diverged { fibers: f2 } => {
                        assert_eq!(f2, fibers);
                    }
                    other => panic!("expected Diverged on idx, got {other:?}"),
                }
            }
            other => panic!("expected Diverged, got {other:?}"),
        }
    }
    impl<'a, T, W> StoreReader<'a, T, W>
    where
        W: Syncable + Seek,
    {
        fn raw_events_for_test(&self) -> Vec<&'a Event<T>> {
            self.log.reader_view().read_line().iter().collect()
        }
    }
}
