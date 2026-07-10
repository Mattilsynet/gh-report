//! `Dragline<T, W>` — runtime composition of an in-memory [`Line<T>`]
//! with a [`Syncable`] byte sink.
//!
//! [`Dragline::commit_event`] appends in-memory only;
//! [`Dragline::sync_data_with_source`] re-serialises the line via
//! [`persist::persist_with_source`], truncates the sink (W2), and fences via
//! [`Syncable::sync_data`], returning a new `Lsn` ack-point.
//!
//! ADR-0010: durability is observed via [`Dragline::acked_lsn`] (and
//! the adopter-facing [`crate::store::StoreWriter::acked_lsn`]) after
//! a successful `sync_data_with_source`.
//! ADR-0002: only place where the in-memory line, `Syncable`, and `Lsn`
//! compose; the substrate (`pardosa-file`) stays unaware of
//! `AppendResult` / `Lsn`. SM-2 = full-rewrite;
//! SM-3 introduces the incremental-write path.
use super::state::{AppendResult, Line};
use crate::durability::Lsn;
use crate::error::PardosaError;
use crate::event::{Event, EventId};
use crate::frontier::{Frontier, FrontierPublisher};
use crate::persist;
use pardosa_file::Syncable;
use pardosa_file::manifest::RecoveredPrefix;
use pardosa_schema::GenomeSafe;
use pardosa_wire::{Encode, to_vec};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
/// Fixed length of the publish-watermark sidecar file: an 8-byte
/// little-endian [`EventId`] value (ADR-0016 §D5). Mirrors the cursor
/// sidecar precedent (ADR-0011 §D7); any other length on disk
/// surfaces as [`crate::PardosaError::PublishWatermark`] at open
/// time and as [`crate::persist::Error::PublishWatermark`] during
/// the post-publish update path.
const PUBLISH_SIDECAR_LEN: usize = 8;
/// Read the publish-watermark sidecar. `Ok(None)` = file absent (no
/// publishes have ever advanced the watermark; recovery republishes
/// every reconstructible anchor). `Ok(Some(id))` = file present and
/// exactly 8 bytes. Any other condition → typed
/// [`PardosaError::PublishWatermark`].
fn read_publish_sidecar(path: &Path) -> Result<Option<EventId>, PardosaError> {
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(PardosaError::PublishWatermark {
                source: Box::new(e),
            });
        }
    };
    let mut buf = [0u8; PUBLISH_SIDECAR_LEN];
    let mut total = 0usize;
    while total < PUBLISH_SIDECAR_LEN {
        match f.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) => {
                return Err(PardosaError::PublishWatermark {
                    source: Box::new(e),
                });
            }
        }
    }
    if total != PUBLISH_SIDECAR_LEN {
        return Err(PardosaError::PublishWatermark {
            source: Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("publish sidecar length {total} != expected {PUBLISH_SIDECAR_LEN}"),
            )),
        });
    }
    let mut overflow = [0u8; 1];
    match f.read(&mut overflow) {
        Ok(0) => {}
        Ok(_) => {
            return Err(PardosaError::PublishWatermark {
                source: Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "publish sidecar longer than 8 bytes",
                )),
            });
        }
        Err(e) => {
            return Err(PardosaError::PublishWatermark {
                source: Box::new(e),
            });
        }
    }
    Ok(Some(EventId::from_decoded(u64::from_le_bytes(buf))))
}
/// Write+fsync the publish-watermark sidecar. Truncates any prior
/// contents. Returns `persist::Error::PublishWatermark` on failure
/// because this lives on the [`Dragline::sync_data`] return path.
fn write_publish_sidecar(path: &Path, id: EventId) -> Result<(), persist::Error> {
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|e| persist::Error::PublishWatermark { source: e })?;
    f.write_all(&id.value().to_le_bytes())
        .map_err(|e| persist::Error::PublishWatermark { source: e })?;
    f.sync_data()
        .map_err(|e| persist::Error::PublishWatermark { source: e })?;
    Ok(())
}
fn backend_error_to_persist_error(e: crate::error::BackendError) -> persist::Error {
    use crate::error::BackendError;
    match e {
        BackendError::Publish { source, .. } => match source.downcast::<std::io::Error>() {
            Ok(io) => persist::Error::Io(*io),
            Err(boxed) => persist::Error::Io(std::io::Error::other(boxed)),
        },
        other => persist::Error::Io(std::io::Error::other(other)),
    }
}
/// Runtime composition of an in-memory [`Line<T>`] with a
/// [`Syncable`] sink.
///
/// [`commit_event`](Self::commit_event) appends in-memory only;
/// [`sync_data_with_source`](Self::sync_data_with_source) drains the
/// full event line to the sink and calls [`Syncable::sync_data`] to
/// fence on durability.
///
/// `Dragline<T, W>` is `Send` but intentionally not `Sync` (F5 /
/// ADR-0014): the embedded `PublishMode` may hold a
/// `Box<dyn FrontierPublisher>`, which is `Send + 'static` (not
/// `Sync`). Anchor dispatch is single-threaded by contract.
pub(crate) struct Dragline<T, W: Syncable + Seek> {
    line: Line<T>,
    sink: W,
    acked_lsn: Option<Lsn>,
    /// Publish-mode capability state — collapses the previous quadrant
    /// of four optional fields (`publisher`, `publish_sidecar_path`,
    /// `publish_watermark` plus the implicit "no publisher" combinator)
    /// into three valid variants. The invalid combination
    /// (sidecar=Some, publisher=None) is now unrepresentable; the
    /// in-memory line / sidecar drain dispatch in
    /// [`Self::sync_data_with_source`] is a match on the mode rather
    /// than a runtime branch with an `expect` invariant. See
    /// ADR-0014 F5 — `Dragline` remains `Send` + !`Sync` because
    /// [`FrontierPublisher`] is declared `Send + 'static` (not
    /// `Sync`).
    mode: PublishMode,
    strategy: WriteStrategy,
    jetstream_synced_events: usize,
}
enum WriteStrategy {
    Direct,
    BackendBacked {
        manifest_path: PathBuf,
        synced_events: usize,
        prefix: Option<RecoveredPrefix>,
        manifest_synced_records: usize,
        manifest_header_synced: bool,
    },
    JetStreamBacked(Box<crate::authoritative::jetstream::JetStreamBackendAdapter>),
}
fn pgno_manifest_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".pgix");
    PathBuf::from(os)
}
/// Capability state of a [`Dragline`]'s publish surface.
///
/// * `NoPublisher` — [`Dragline::new`] / [`Dragline::from_line_for_open`]:
///   no anchors are ever dispatched.
/// * `Durable` — [`Dragline::with_line_and_publisher_path`]: anchors
///   flow through a publisher and a fsync-ed publish-watermark sidecar
///   (ADR-0016 §§D5–D8). [`crate::store::EventStore::open_with_publisher`]
///   is the adopter-facing entry (ADR-0018).
enum PublishMode {
    NoPublisher,
    Durable {
        publisher: Box<dyn FrontierPublisher>,
        sidecar_path: PathBuf,
        watermark: Option<EventId>,
    },
}
impl<T, W> Dragline<T, W>
where
    W: Syncable + Seek,
{
    /// Construct a Dragline over an empty in-memory line and a
    /// `Syncable` sink.
    ///
    /// The sink's initial position is irrelevant;
    /// [`sync_data_with_source`](Self::sync_data_with_source) rewinds
    /// to byte 0 before writing the `.pgno` from scratch. The composed
    /// envelope hash (`schema_hash_combine(T::SCHEMA_HASH,
    /// ENVELOPE_SHAPE_HASH)`) is recovered via `GenomeSafe`;
    /// envelope-shape changes surface at [`Reader::open`].
    pub fn new(sink: W) -> Self {
        Self {
            line: Line::new(),
            sink,
            acked_lsn: None,
            mode: PublishMode::NoPublisher,
            strategy: WriteStrategy::Direct,
            jetstream_synced_events: 0,
        }
    }
    /// Construct a Dragline with an attached [`FrontierPublisher`]
    /// and a durable publish-watermark sidecar (ADR-0016 §§D5–D8).
    ///
    /// Accepts an already-rehydrated [`Line<T>`] and re-configures
    /// its `stream_name` and `anchor_interval`. Absent sidecar ⇒
    /// `watermark = None`, recovery republishes every
    /// reconstructible anchor.
    ///
    /// `T`-independent here; the anchor drain runs from
    /// [`Self::sync_data_with_source`] under `T: Encode + GenomeSafe`
    /// (ADR-0020).
    ///
    /// # Errors
    ///
    /// [`PardosaError::PublishWatermark`] on sidecar I/O failure or
    /// on-disk length ≠ `PUBLISH_SIDECAR_LEN` (8).
    pub fn with_line_and_publisher_path<P: FrontierPublisher>(
        mut line: Line<T>,
        sink: W,
        publish_sidecar_path: PathBuf,
        stream_name: String,
        anchor_interval: u64,
        publisher: P,
    ) -> Result<Self, PardosaError> {
        let watermark = read_publish_sidecar(&publish_sidecar_path)?;
        line.configure_recover(stream_name, anchor_interval);
        Ok(Self {
            line,
            sink,
            acked_lsn: None,
            mode: PublishMode::Durable {
                publisher: Box::new(publisher),
                sidecar_path: publish_sidecar_path,
                watermark,
            },
            strategy: WriteStrategy::Direct,
            jetstream_synced_events: 0,
        })
    }
    /// Restart-without-publisher variant used by the
    /// `store::EventStore::open` / `open_validated` rehydrate paths
    /// (ADR-0018 §D7). Wraps an already-rehydrated
    /// [`Line<T>`] and an existing sink in
    /// [`PublishMode::NoPublisher`]. Crate-internal — the public
    /// adopter entry is [`crate::store::EventStore::open_validated`].
    pub(crate) fn from_line_for_open(line: Line<T>, sink: W) -> Self {
        Self {
            line,
            sink,
            acked_lsn: None,
            mode: PublishMode::NoPublisher,
            strategy: WriteStrategy::Direct,
            jetstream_synced_events: 0,
        }
    }
    pub(crate) fn from_backend_for_open(
        line: Line<T>,
        sink: W,
        journal_path: &Path,
        prefix: Option<RecoveredPrefix>,
        manifest_already_synced: bool,
    ) -> Self {
        let synced_events = line.read_line().len();
        let manifest_synced_records = prefix
            .as_ref()
            .map_or(0, |p| p.records.len())
            .checked_mul(usize::from(manifest_already_synced))
            .unwrap_or(0);
        Self {
            line,
            sink,
            acked_lsn: None,
            mode: PublishMode::NoPublisher,
            strategy: WriteStrategy::BackendBacked {
                manifest_path: pgno_manifest_path(journal_path),
                synced_events,
                prefix,
                manifest_synced_records,
                manifest_header_synced: manifest_already_synced,
            },
            jetstream_synced_events: 0,
        }
    }
    /// Open variant routing sync writes through the supplied
    /// sealed JetStream-backed substrate adapter (ADR-0022 §D2 +
    /// §D11).
    ///
    /// The `sink` slot is a scratch [`std::fs::File`] retained
    /// only to satisfy the `Dragline<T, std::fs::File>` shape the
    /// public `EventStore<T>` alias fixes. The `JetStream`
    /// write-strategy arm of [`Self::sync_data_with_source`]
    /// routes the `.pgno` blob through the sealed
    /// [`crate::backend::BackendSink`] on the adapter — the
    /// scratch sink is never written to. Durability is fenced by
    /// the substrate's publish-ack.
    pub(crate) fn from_backend_for_open_jetstream(
        line: Line<T>,
        sink: W,
        adapter: crate::authoritative::jetstream::JetStreamBackendAdapter,
        synced_events: usize,
    ) -> Self {
        Self {
            line,
            sink,
            acked_lsn: None,
            mode: PublishMode::NoPublisher,
            strategy: WriteStrategy::JetStreamBacked(Box::new(adapter)),
            jetstream_synced_events: synced_events,
        }
    }
    /// The most recently acked `Lsn`, or `None` if `sync_data` has not
    /// been called since construction. `T`-independent (ADR-0020 reader
    /// bound).
    #[must_use]
    pub fn acked_lsn(&self) -> Option<Lsn> {
        self.acked_lsn
    }
    #[must_use]
    pub(crate) fn fiber_state(&self, fiber_id: crate::event::FiberId) -> crate::FiberState {
        self.line.fiber_state(fiber_id)
    }
    /// Borrow the runtime's in-memory line as a read-only
    /// [`DraglineView<'_, T>`](crate::dragline::DraglineView).
    ///
    /// `T`-independent: the view is a zero-cost capability borrow
    /// and its accessors carry no `T` bounds (ADR-0016 §D2,
    /// ADR-0020 reader bound). This is the canonical hand-out path
    /// from writer to reader component per ADR-0016 §D3.
    #[must_use]
    pub fn reader_view(&self) -> crate::dragline::DraglineView<'_, T> {
        crate::dragline::DraglineView::new(&self.line)
    }
    /// Consume the runtime, returning the underlying sink. Used by
    /// tests (and crash-recovery code paths) that need to re-open the
    /// sink for reading. `T`-independent (ADR-0020 reader bound).
    pub fn into_inner(self) -> W {
        self.sink
    }
    #[cfg(test)]
    fn strategy_recovered_prefix_for_test(&self) -> Option<RecoveredPrefix> {
        match &self.strategy {
            WriteStrategy::BackendBacked { prefix, .. } => prefix.clone(),
            WriteStrategy::Direct | WriteStrategy::JetStreamBacked(_) => None,
        }
    }
}
impl<T, W> Dragline<T, W>
where
    T: Encode + GenomeSafe,
    W: Syncable + Seek,
{
    /// Append a single event to the in-memory line. Returns an
    /// `AppendResult` — the event is visible to in-process readers but
    /// is **not** durable. Callers must invoke `sync_data` to fence
    /// on durability and observe the resulting [`Lsn`] via
    /// [`Self::acked_lsn`].
    ///
    /// # Errors
    /// Forwards any `PardosaError` from `Line::create` (commit-
    /// pipeline failures such as `EventIdOverflow`,
    /// `MonotonicityViolation`, or `InvalidTransition`).
    pub fn commit_event(&mut self, event: T) -> Result<AppendResult, PardosaError> {
        self.line.create(event)
    }
    /// Append a continuation event to an existing fiber. The
    /// `EventLog` facade's `append_to(handle, event)` path; the
    /// underlying `Line::update` advances the fiber's current
    /// pointer and links the new event back via `Precursor::Of(_)`.
    ///
    /// # Errors
    /// Forwards any `PardosaError` from `Line::update`
    /// (`FiberNotFound`, `InvalidTransition`, `EventIdOverflow`,
    /// `IndexOverflow`).
    pub fn commit_update(
        &mut self,
        fiber_id: crate::event::FiberId,
        event: T,
    ) -> Result<AppendResult, PardosaError> {
        self.line.update(fiber_id, event)
    }
    /// Append a detach event marking the fiber `Detached`. The
    /// `EventLog` facade's `detach(live, event)` path; the
    /// underlying `Line::detach` advances the fiber's current
    /// pointer, transitions state `Defined → Detached`, and
    /// chains the new event back via `Precursor::Of(_)`.
    ///
    /// # Errors
    /// Forwards any `PardosaError` from `Line::detach`
    /// (`FiberNotFound`, `InvalidTransition`, `EventIdOverflow`,
    /// `IndexOverflow`).
    pub fn commit_detach(
        &mut self,
        fiber_id: crate::event::FiberId,
        event: T,
    ) -> Result<AppendResult, PardosaError> {
        self.line.detach(fiber_id, event)
    }
    /// Append a rescue event transitioning the fiber `Detached →
    /// Defined`. The `EventLog` facade's `rescue(detached, event)`
    /// path; the underlying `Line::rescue` advances the
    /// fiber's current pointer and chains the new event via
    /// `Precursor::Of(_)` (the `Detached → Defined` arm).
    ///
    /// The `LockedRescuePolicy` parameter on the substrate is
    /// fixed to [`crate::fiber_state::LockedRescuePolicy::PreserveAuditTrail`]
    /// here; the public facade does not yet expose `Locked` fibers
    /// (which is where the policy would matter), so the choice is
    /// not observable.
    ///
    /// # Errors
    /// Forwards any `PardosaError` from `Line::rescue`
    /// (`FiberNotFound`, `InvalidTransition`, `EventIdOverflow`,
    /// `IndexOverflow`).
    pub fn commit_rescue(
        &mut self,
        fiber_id: crate::event::FiberId,
        event: T,
    ) -> Result<AppendResult, PardosaError> {
        self.line.rescue(
            fiber_id,
            crate::fiber_state::LockedRescuePolicy::PreserveAuditTrail,
            event,
        )
    }
    /// Persist all in-memory events and fence on durability.
    ///
    /// Rewinds to byte 0, writes a complete `.pgno`, calls
    /// [`Syncable::sync_data`], advances `acked_lsn`, returns it.
    /// Not crash-atomic file replacement; see ADR-0010 §D3.
    ///
    /// Pending anchors then dispatch to the attached
    /// [`FrontierPublisher`] in commit order. Publish failure
    /// requeues the suffix (ADR-0015 D3); local durability is
    /// independent of publish (ADR-0015 D4).
    ///
    /// `schema_source`, when `Some`, is embedded in the container
    /// footer as opaque metadata (ADR-0002).
    ///
    /// # Errors
    ///
    /// [`persist::Error`]. `PublishError` is not propagated.
    pub fn sync_data_with_source(
        &mut self,
        schema_source: Option<&'static str>,
    ) -> Result<Lsn, persist::Error> {
        let lsn_value = match &mut self.strategy {
            WriteStrategy::Direct => {
                self.sink.seek(SeekFrom::Start(0))?;
                persist::persist_with_source(&self.line, &mut self.sink, schema_source)?;
                let pos = self.sink.stream_position()?;
                <W as Syncable>::set_len(&mut self.sink, pos)?;
                <W as Syncable>::sync_data(&mut self.sink)?;
                pos
            }
            WriteStrategy::BackendBacked { .. } => self.sync_backend_backed(schema_source)?,
            WriteStrategy::JetStreamBacked(adapter) => {
                self.line
                    .check_persistable()
                    .map_err(|kind| persist::Error::UnpersistableState { kind })?;
                let events = self.line.read_line();
                let start = self.jetstream_synced_events.min(events.len());
                let mut ack_value = self.acked_lsn.map_or(0, Lsn::value);
                for event in &events[start..] {
                    let bytes = to_vec(event);
                    let ack = crate::backend::BackendSink::append(adapter.as_mut(), &bytes)
                        .map_err(backend_error_to_persist_error)?;
                    ack_value = ack.as_u64();
                    self.jetstream_synced_events = self.jetstream_synced_events.saturating_add(1);
                }
                let ack = crate::backend::BackendSink::sync(adapter.as_mut())
                    .map_err(backend_error_to_persist_error)?;
                if ack.as_u64() > ack_value {
                    ack_value = ack.as_u64();
                }
                ack_value
            }
        };
        let lsn = Lsn::new(lsn_value);
        self.acked_lsn = Some(lsn);
        match &mut self.mode {
            PublishMode::NoPublisher => {}
            PublishMode::Durable {
                publisher,
                sidecar_path,
                watermark,
            } => {
                Self::drain_reconstructed_anchors(
                    &self.line,
                    publisher.as_mut(),
                    sidecar_path,
                    watermark,
                )?;
            }
        }
        Ok(lsn)
    }
    fn sync_backend_backed(
        &mut self,
        schema_source: Option<&'static str>,
    ) -> Result<u64, persist::Error> {
        self.line
            .check_persistable()
            .map_err(|kind| persist::Error::UnpersistableState { kind })?;
        let WriteStrategy::BackendBacked {
            manifest_path,
            synced_events,
            prefix,
            manifest_synced_records,
            manifest_header_synced,
        } = &mut self.strategy
        else {
            unreachable!("sync_backend_backed called for non-backend strategy")
        };
        let events = self.line.read_line();
        let start = (*synced_events).min(events.len());
        let mut manifest = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(manifest_path)
            .map_err(persist::Error::Io)?;
        let mut writer = if let Some(recovered) = prefix.take() {
            <W as Syncable>::set_len(&mut self.sink, recovered.data_end)?;
            self.sink.seek(SeekFrom::Start(recovered.data_end))?;
            pardosa_file::AppendWriter::resume_from_recovered_prefix(&mut self.sink, &recovered)
                .with_manifest_synced_records(
                    &mut manifest,
                    *manifest_synced_records,
                    *manifest_header_synced,
                )
        } else {
            self.sink.seek(SeekFrom::End(0))?;
            let mut append_writer =
                pardosa_file::AppendWriter::new(&mut self.sink, Event::<T>::ENVELOPE_HASH);
            if let Some(source) = schema_source {
                append_writer = append_writer.with_schema_source(source);
            }
            append_writer.with_manifest(&mut manifest)
        };
        for event in &events[start..] {
            let bytes = to_vec(event);
            writer.append_message(&bytes)?;
        }
        writer
            .sync_data_with_frontier(*self.line.frontier().as_bytes())
            .map_err(persist::Error::Io)?;
        let recovered = writer.recovered_prefix();
        *synced_events = events.len();
        *manifest_synced_records = recovered.records.len();
        *manifest_header_synced = true;
        *prefix = Some(recovered.clone());
        Ok(recovered.data_end)
    }
    /// ADR-0016 §D6 drain: re-fold the persisted line via
    /// [`crate::dragline::recover::reconstruct_unpublished_anchors`],
    /// filtered by the publish watermark.
    ///
    /// Each successful publish `fsync`-s the sidecar before the
    /// in-memory watermark advances — durable witness lands first,
    /// so a crash between `publish` and watermark-advance still
    /// reconstructs the same `event_id <= sidecar` state (ADR-0016
    /// §D5). Publish or sidecar-write failure halts the drain; the
    /// suffix retries on the next `sync_data`. Per-anchor fsync is
    /// load-bearing; a future optimisation may batch at the cost of
    /// republishing the in-batch tail on restart.
    fn drain_reconstructed_anchors(
        line: &Line<T>,
        publisher: &mut dyn FrontierPublisher,
        sidecar_path: &Path,
        watermark: &mut Option<EventId>,
    ) -> Result<(), persist::Error> {
        let anchors = crate::dragline::recover::reconstruct_unpublished_anchors(line, *watermark);
        for anchor in anchors {
            match publisher.publish(&anchor.subject, &anchor.payload) {
                Ok(()) => {
                    write_publish_sidecar(sidecar_path, anchor.event_id)?;
                    *watermark = Some(anchor.event_id);
                }
                Err(_) => {
                    break;
                }
            }
        }
        Ok(())
    }
}
impl<T, W> Dragline<T, W>
where
    W: Syncable + Seek,
{
    /// Rolling BLAKE3 chain-frontier over the in-memory event line.
    ///
    /// `T`-independent: the frontier is folded from already-encoded
    /// event bytes, so no `Encode + GenomeSafe` bound is required.
    /// Mirrors the bound-free shape of the accessors `StoreReader`
    /// exposes through the public surface (ADR-0018 §D3
    /// observability).
    #[must_use]
    pub(crate) fn frontier(&self) -> Frontier {
        self.line.frontier()
    }
    /// In-memory mirror of the on-disk publish watermark. `None` means
    /// no anchor has ever been published; `Some(id)` means every
    /// anchor whose source event has `event_id <= id` has been
    /// successfully published and durably recorded in the sidecar
    /// (ADR-0016 §D5).
    ///
    /// `T`-independent: only the mode discriminant and its
    /// `watermark` slot are read, so no `Encode + GenomeSafe` bound
    /// is required. Mirrors the bound-free shape of the accessors
    /// `StoreReader` exposes through the public surface (ADR-0018
    /// §D3 observability).
    #[must_use]
    pub(crate) fn publish_watermark(&self) -> Option<EventId> {
        match &self.mode {
            PublishMode::Durable { watermark, .. } => *watermark,
            PublishMode::NoPublisher => None,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Event;
    use crate::typed::HasEventSchemaSource;
    use crate::typed::TypedReader;
    use pardosa_schema::{GenomeSafe, schema_hash_bytes};
    use pardosa_wire::from_bytes;
    use pardosa_wire::{Decode, DecodeError, Decoder, EventSafe};
    use std::io::Cursor;
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct P3aZeroSeedPayload(u64);
    impl pardosa_wire::sealed::Sealed for P3aZeroSeedPayload {}
    impl EventSafe for P3aZeroSeedPayload {}
    impl GenomeSafe for P3aZeroSeedPayload {
        const SCHEMA_HASH: u128 = schema_hash_bytes(b"P3aZeroSeedPayload");
        const SCHEMA_SOURCE: &'static str = "P3aZeroSeedPayload";
    }
    impl Encode for P3aZeroSeedPayload {
        fn encode(&self, out: &mut Vec<u8>) {
            self.0.encode(out);
        }
    }
    impl Decode for P3aZeroSeedPayload {
        fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
            u64::decode(d).map(Self)
        }
    }
    impl HasEventSchemaSource for P3aZeroSeedPayload {
        const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
    }
    /// W2 truncation invariant (ADR-0010): a shorter rewrite of a
    /// sink that previously held a longer payload must `set_len` the
    /// sink to the post-sync `Lsn`. Relocated from the retired
    /// `EventLog` substrate adapter (mission rescue-pardosa-ku9t).
    #[test]
    fn sync_truncates_stale_trailing_bytes_from_prior_longer_payload() {
        let stale_bytes = vec![0xAAu8; 4096];
        let prior_len = stale_bytes.len() as u64;
        let sink: Cursor<Vec<u8>> = Cursor::new(stale_bytes);
        let mut runtime: Dragline<u64, _> = Dragline::new(sink);
        for i in 0..5u64 {
            let _ = runtime.commit_event(i).expect("commit_event");
        }
        let acked = runtime.sync_data_with_source(None).expect("sync");
        assert!(
            acked.value() < prior_len,
            "test premise: shorter rewrite (acked {} < prior {})",
            acked.value(),
            prior_len
        );
        let cursor = runtime.into_inner();
        let buf_len = cursor.get_ref().len() as u64;
        assert_eq!(
            buf_len,
            acked.value(),
            "W2: sink must be truncated to acked lsn; stale trailing bytes survived"
        );
        let mut cursor = cursor;
        cursor.set_position(0);
        let mut reader: TypedReader<_, u64> =
            TypedReader::open(cursor).expect("reopen after truncation");
        assert_eq!(reader.message_count(), 5);
        let last_bytes = reader
            .inner_mut()
            .iter_messages()
            .last()
            .expect("at least one message")
            .expect("read last message");
        let last: Event<u64> = from_bytes(&last_bytes).expect("decode last event");
        assert_eq!(*last.domain_event(), 4u64);
    }
    /// I1 (oracle bead rescue-pardosa-v0id): the backend-keyed write
    /// path on [`Dragline`] must persist bytes byte-identical to the
    /// legacy direct path for the same in-memory line.
    #[test]
    fn from_backend_for_open_sync_bytes_byte_identical_to_from_line_for_open_sync() {
        let legacy_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let mut legacy_runtime: Dragline<u64, _> = Dragline::new(legacy_sink);
        for i in 0..5u64 {
            let _ = legacy_runtime.commit_event(i).expect("commit legacy");
        }
        let _ = legacy_runtime
            .sync_data_with_source(None)
            .expect("sync legacy");
        let legacy_sink = legacy_runtime.into_inner();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("runtime-test.pgno");
        let backend_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let mut backend_runtime: Dragline<u64, _> =
            Dragline::from_backend_for_open(Line::new(), backend_sink, &path, None, false);
        for i in 0..5u64 {
            let _ = backend_runtime.commit_event(i).expect("commit backend");
        }
        let _ = backend_runtime
            .sync_data_with_source(None)
            .expect("sync backend");
        let mut legacy_reader = pardosa_file::Reader::open(legacy_sink).expect("open legacy");
        let legacy_events: Vec<u64> = legacy_reader
            .iter_messages()
            .map(|bytes| {
                let bytes = bytes.expect("read legacy message");
                let event: Event<u64> = from_bytes(&bytes).expect("decode legacy event");
                *event.domain_event()
            })
            .collect();
        let recovered = backend_runtime
            .strategy_recovered_prefix_for_test()
            .expect("backend prefix");
        let backend_sink = backend_runtime.into_inner();
        let mut backend_sink = backend_sink;
        pardosa_file::manifest::finalize_recovered_prefix(&recovered, &mut backend_sink)
            .expect("finalize backend");
        backend_sink.set_position(0);
        let mut backend_reader =
            pardosa_file::Reader::open(backend_sink).expect("open finalized backend");
        let backend_events: Vec<u64> = backend_reader
            .iter_messages()
            .map(|bytes| {
                let bytes = bytes.expect("read backend message");
                let event: Event<u64> = from_bytes(&bytes).expect("decode backend event");
                *event.domain_event()
            })
            .collect();
        assert_eq!(backend_events, legacy_events);
    }
    /// Sealed-substrate parity: the bytes the backend-keyed write
    /// path on [`Dragline`] hands its in-place sink must equal the
    /// bytes [`crate::backend::journal::BackendDragline::sync`] hands
    /// its sealed [`crate::backend::BackendSink`] for the same
    /// in-memory line (sub-mission 03 cycle 1 contract; ADR-0022 §D2).
    #[test]
    fn from_backend_for_open_sync_bytes_byte_identical_to_backend_dragline_sync() {
        use crate::authoritative::fake::InMemoryBackend;
        use crate::backend::journal::BackendDragline;
        let backend = InMemoryBackend::new();
        let mut bj: BackendDragline<u64, InMemoryBackend> = BackendDragline::new(backend);
        for i in 0..5u64 {
            let _ = bj.commit_event(i).expect("commit backend dragline");
        }
        let _ = bj.sync().expect("sync backend dragline");
        let reference_bytes: Vec<u8> = bj.into_backend().bytes().to_vec();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("runtime-test.pgno");
        let prod_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let mut prod_runtime: Dragline<u64, _> =
            Dragline::from_backend_for_open(Line::new(), prod_sink, &path, None, false);
        for i in 0..5u64 {
            let _ = prod_runtime
                .commit_event(i)
                .expect("commit production runtime");
        }
        let _ = prod_runtime
            .sync_data_with_source(None)
            .expect("sync production runtime");
        let mut reference_reader =
            pardosa_file::Reader::open(Cursor::new(reference_bytes)).expect("open reference");
        let reference_events: Vec<u64> = reference_reader
            .iter_messages()
            .map(|bytes| {
                let bytes = bytes.expect("read reference message");
                let event: Event<u64> = from_bytes(&bytes).expect("decode reference event");
                *event.domain_event()
            })
            .collect();
        let recovered = prod_runtime
            .strategy_recovered_prefix_for_test()
            .expect("backend prefix");
        let prod_sink = prod_runtime.into_inner();
        let mut prod_sink = prod_sink;
        pardosa_file::manifest::finalize_recovered_prefix(&recovered, &mut prod_sink)
            .expect("finalize production runtime");
        prod_sink.set_position(0);
        let mut prod_reader = pardosa_file::Reader::open(prod_sink).expect("open finalized prod");
        let prod_events: Vec<u64> = prod_reader
            .iter_messages()
            .map(|bytes| {
                let bytes = bytes.expect("read production message");
                let event: Event<u64> = from_bytes(&bytes).expect("decode production event");
                *event.domain_event()
            })
            .collect();
        assert_eq!(prod_events, reference_events);
    }
    #[test]
    fn from_backend_for_open_recovers_footerless_torn_tail_after_multiple_syncs() {
        use crate::store::{EventStore, PgnoBackend};
        use std::io::Write as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("incremental.pgno");
        {
            let mut seed = EventStore::<P3aZeroSeedPayload>::create(&path).expect("create seed");
            let _ = seed.writer().sync().expect("sync empty seed");
        }
        {
            let mut store =
                EventStore::<P3aZeroSeedPayload>::open_with_backend(PgnoBackend::open(&path))
                    .expect("open pgno backend");
            for event in 0..3u64 {
                let _ = store
                    .writer()
                    .begin(P3aZeroSeedPayload(event))
                    .expect("commit first batch");
            }
            let _ = store.writer().sync().expect("sync first batch");
            for event in 3..8u64 {
                let _ = store
                    .writer()
                    .begin(P3aZeroSeedPayload(event))
                    .expect("commit second batch");
            }
            let _ = store.writer().sync().expect("sync second batch");
        }
        let mut manifest_os = path.as_os_str().to_os_string();
        manifest_os.push(".pgix");
        let manifest_path = std::path::PathBuf::from(manifest_os);
        assert!(
            manifest_path.exists(),
            "FILE-mode incremental sync must create the same-directory .pgix manifest sidecar",
        );
        {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open for torn tail");
            file.write_all(&[0xDE, 0xAD, 0xBE, 0xEF])
                .expect("append torn tail bytes");
        }
        let direct_open = pardosa_file::Reader::open(
            std::fs::File::open(&path).expect("open torn pgno for direct reader"),
        );
        assert!(
            direct_open.is_err(),
            "simulated footerless/torn-tail file must not open directly before recovery",
        );
        let _recovered =
            EventStore::<P3aZeroSeedPayload>::open_with_backend(PgnoBackend::open(&path))
                .expect("open should recover and finalize footerless prefix");
        let mut reader = pardosa_file::Reader::open(
            std::fs::File::open(&path).expect("open recovered pgno for direct reader"),
        )
        .expect("Reader::open after runtime recovery");
        let decoded: Vec<u64> = reader
            .iter_messages()
            .map(|bytes| {
                let bytes = bytes.expect("read recovered message");
                let event: Event<P3aZeroSeedPayload> = from_bytes(&bytes).expect("decode event");
                event.domain_event().0
            })
            .collect();
        assert_eq!(decoded, (0..8u64).collect::<Vec<_>>());
    }
    #[test]
    #[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission g3-jetstream-schema-gate-exec)"]
    fn jetstream_backed_sync_publishes_one_message_per_new_event() {
        use crate::authoritative::jetstream::JetStreamBackendAdapter;
        use pardosa_nats::test_support::LiveNatsServer;
        use pardosa_nats::{JetStreamBackend, JetStreamConfig, RuntimeHandle};
        use pardosa_wire::to_vec;
        use std::sync::Arc;
        use std::time::{SystemTime, UNIX_EPOCH};
        use tokio::runtime::Runtime;
        fn tag() -> String {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            format!("{}_{}", std::process::id(), nanos)
        }
        fn config(tag: &str, rt: &Runtime, server: &LiveNatsServer) -> JetStreamConfig {
            JetStreamConfig::builder()
                .stream_name(format!("P3A_PER_EVENT_{tag}"))
                .subject(format!("p3a.per_event.{tag}"))
                .durable_consumer(format!("p3a-per-event-{tag}"))
                .runtime_handle(RuntimeHandle::from_tokio(rt.handle().clone()))
                .nats_url(server.url().to_owned())
                .build()
                .expect("config valid")
        }
        async fn delete_stream(server: &LiveNatsServer, stream_name: &str) {
            let Ok(client) = async_nats::connect(server.url()).await else {
                return;
            };
            let js = async_nats::jetstream::new(client);
            let _ = js.delete_stream(stream_name).await;
        }
        let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
        let rt = Runtime::new().expect("tokio runtime");
        let tag = tag();
        let stream_name = format!("P3A_PER_EVENT_{tag}");
        let handle = JetStreamBackend::open(config(&tag, &rt, &server));
        let adapter = JetStreamBackendAdapter::new(handle);
        let mut runtime: Dragline<u64, _> = Dragline::from_backend_for_open_jetstream(
            Line::new(),
            Cursor::new(Vec::new()),
            adapter,
            0,
        );
        for event in 0..4u64 {
            let _ = runtime.commit_event(event).expect("commit event");
        }
        let expected_frames: Vec<Vec<u8>> = runtime
            .reader_view()
            .read_line()
            .iter()
            .map(to_vec)
            .collect();
        let _ = runtime.sync_data_with_source(None).expect("sync events");
        let replay = JetStreamBackend::open(config(&tag, &rt, &server));
        let records = replay.replay_all().expect("replay records");
        assert_eq!(
            records.len(),
            expected_frames.len(),
            "JetStream-backed sync must publish one NATS message per new event; \
             full-blob sync publishes one growing snapshot instead"
        );
        for (i, (record, expected)) in records.iter().zip(expected_frames.iter()).enumerate() {
            assert_eq!(
                record.payload.as_ref(),
                expected.as_slice(),
                "record {i} body must equal that event's canonical bytes"
            );
        }
        rt.block_on(delete_stream(&server, &stream_name));
    }
    #[test]
    #[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission g3-jetstream-schema-gate-exec)"]
    fn event_frame_rehydrate_frontier_matches_pgno_blob_path() {
        use crate::store::{EventStore, JetStreamBackend as StoreJetStreamBackend, PgnoBackend};
        use pardosa_nats::test_support::LiveNatsServer;
        use pardosa_nats::{JetStreamBackend, JetStreamConfig, RuntimeHandle};
        use std::sync::Arc;
        use std::time::{SystemTime, UNIX_EPOCH};
        use tokio::runtime::Runtime;
        fn tag() -> String {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            format!("{}_{}", std::process::id(), nanos)
        }
        fn config(tag: &str, rt: &Runtime, server: &LiveNatsServer) -> JetStreamConfig {
            JetStreamConfig::builder()
                .stream_name(format!("P3A_FRONTIER_{tag}"))
                .subject(format!("p3a.frontier.{tag}"))
                .durable_consumer(format!("p3a-frontier-{tag}"))
                .runtime_handle(RuntimeHandle::from_tokio(rt.handle().clone()))
                .nats_url(server.url().to_owned())
                .build()
                .expect("config valid")
        }
        async fn delete_stream(server: &LiveNatsServer, stream_name: &str) {
            let Ok(client) = async_nats::connect(server.url()).await else {
                return;
            };
            let js = async_nats::jetstream::new(client);
            let _ = js.delete_stream(stream_name).await;
        }
        let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
        let rt = Runtime::new().expect("tokio runtime");
        let tag = tag();
        let stream_name = format!("P3A_FRONTIER_{tag}");
        let backend =
            StoreJetStreamBackend::open(JetStreamBackend::open(config(&tag, &rt, &server)));
        let mut jetstream_store = EventStore::<P3aZeroSeedPayload>::create_with_backend(backend)
            .expect("create jetstream store");
        let mut pgno_path = std::env::temp_dir();
        pgno_path.push(format!("p3a-frontier-{tag}.pgno"));
        let mut pgno_store =
            EventStore::<P3aZeroSeedPayload>::create(&pgno_path).expect("create pgno store");
        for event in 0..7u64 {
            let _ = jetstream_store
                .writer()
                .begin(P3aZeroSeedPayload(event))
                .expect("begin jetstream event");
            let _ = pgno_store
                .writer()
                .begin(P3aZeroSeedPayload(event))
                .expect("begin pgno event");
        }
        let _ = jetstream_store.writer().sync().expect("sync jetstream");
        let _ = pgno_store.writer().sync().expect("sync pgno");
        drop(jetstream_store);
        drop(pgno_store);
        let reopened_jetstream = EventStore::<P3aZeroSeedPayload>::open_with_backend(
            StoreJetStreamBackend::open(JetStreamBackend::open(config(&tag, &rt, &server))),
        )
        .expect("reopen jetstream");
        let reopened_pgno =
            EventStore::<P3aZeroSeedPayload>::open_with_backend(PgnoBackend::open(&pgno_path))
                .expect("reopen pgno");
        assert_eq!(
            reopened_jetstream.reader().frontier().as_bytes(),
            reopened_pgno.reader().frontier().as_bytes(),
            "per-event frame replay frontier must be byte-identical to .pgno full-blob replay"
        );
        let _ = std::fs::remove_file(&pgno_path);
        rt.block_on(delete_stream(&server, &stream_name));
    }
    #[test]
    #[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission g3-jetstream-schema-gate-exec)"]
    fn event_frame_rehydrate_rejects_foreign_schema_tag() {
        use crate::store::{EventStore, JetStreamBackend as StoreJetStreamBackend};
        use pardosa_nats::test_support::LiveNatsServer;
        use pardosa_nats::{JetStreamBackend, JetStreamConfig, RuntimeHandle};
        use pardosa_wire::to_vec;
        use std::sync::Arc;
        use std::time::{SystemTime, UNIX_EPOCH};
        use tokio::runtime::Runtime;
        fn tag() -> String {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            format!("{}_{}", std::process::id(), nanos)
        }
        fn config(tag: &str, rt: &Runtime, server: &LiveNatsServer) -> JetStreamConfig {
            JetStreamConfig::builder()
                .stream_name(format!("P3A_SCHEMA_TAG_{tag}"))
                .subject(format!("p3a.schema_tag.{tag}"))
                .durable_consumer(format!("p3a-schema-tag-{tag}"))
                .runtime_handle(RuntimeHandle::from_tokio(rt.handle().clone()))
                .nats_url(server.url().to_owned())
                .stream_description_marker(format!(
                    "{:032x}",
                    Event::<P3aZeroSeedPayload>::ENVELOPE_HASH
                ))
                .build()
                .expect("config valid")
        }
        async fn delete_stream(server: &LiveNatsServer, stream_name: &str) {
            let Ok(client) = async_nats::connect(server.url()).await else {
                return;
            };
            let js = async_nats::jetstream::new(client);
            let _ = js.delete_stream(stream_name).await;
        }
        let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
        let rt = Runtime::new().expect("tokio runtime");
        let tag = tag();
        let stream_name = format!("P3A_SCHEMA_TAG_{tag}");
        let handle = JetStreamBackend::open(config(&tag, &rt, &server));
        let event = Event::new_unchecked(
            crate::EventId::from_decoded(0),
            crate::FiberId::from_decoded(0),
            false,
            crate::event::Precursor::Genesis,
            [0u8; 32],
            P3aZeroSeedPayload(7),
        );
        let bytes = to_vec(&event);
        let foreign_tag = "fedcba9876543210fedcba9876543210";
        let _ = handle
            .append_with_replay_tag(&bytes, foreign_tag)
            .expect("append foreign-tagged frame");
        let Err(err) = EventStore::<P3aZeroSeedPayload>::open_with_backend(
            StoreJetStreamBackend::open(JetStreamBackend::open(config(&tag, &rt, &server))),
        ) else {
            panic!("foreign schema tag must reject JetStream rehydrate")
        };
        match err {
            PardosaError::CursorRead { source } => match *source {
                persist::Error::SchemaHashMismatch { expected, found } => {
                    assert_eq!(expected, Event::<P3aZeroSeedPayload>::ENVELOPE_HASH);
                    assert_eq!(found, 0xfedc_ba98_7654_3210_fedc_ba98_7654_3210);
                }
                other => panic!("expected SchemaHashMismatch, got {other:?}"),
            },
            other => panic!("expected CursorRead, got {other:?}"),
        }
        rt.block_on(delete_stream(&server, &stream_name));
    }
    #[test]
    #[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission g3-jetstream-schema-gate-exec)"]
    fn create_with_backend_fresh_jetstream_emits_zero_seed_messages() {
        use crate::store::{EventStore, JetStreamBackend as StoreJetStreamBackend};
        use pardosa_nats::test_support::LiveNatsServer;
        use pardosa_nats::{JetStreamBackend, JetStreamConfig, RuntimeHandle};
        use std::sync::Arc;
        use std::time::{SystemTime, UNIX_EPOCH};
        use tokio::runtime::Runtime;
        fn tag() -> String {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            format!("{}_{}", std::process::id(), nanos)
        }
        fn config(tag: &str, rt: &Runtime, server: &LiveNatsServer) -> JetStreamConfig {
            JetStreamConfig::builder()
                .stream_name(format!("P3A_ZERO_SEED_{tag}"))
                .subject(format!("p3a.zero_seed.{tag}"))
                .durable_consumer(format!("p3a-zero-seed-{tag}"))
                .runtime_handle(RuntimeHandle::from_tokio(rt.handle().clone()))
                .nats_url(server.url().to_owned())
                .build()
                .expect("config valid")
        }
        async fn delete_stream(server: &LiveNatsServer, stream_name: &str) {
            let Ok(client) = async_nats::connect(server.url()).await else {
                return;
            };
            let js = async_nats::jetstream::new(client);
            let _ = js.delete_stream(stream_name).await;
        }
        let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
        let rt = Runtime::new().expect("tokio runtime");
        let tag = tag();
        let stream_name = format!("P3A_ZERO_SEED_{tag}");
        let backend =
            StoreJetStreamBackend::open(JetStreamBackend::open(config(&tag, &rt, &server)));
        let mut store = EventStore::<P3aZeroSeedPayload>::create_with_backend(backend)
            .expect("fresh create_with_backend succeeds without seed blob");
        let after_create = JetStreamBackend::open(config(&tag, &rt, &server))
            .replay_all()
            .expect("replay after create");
        assert_eq!(
            after_create.len(),
            0,
            "fresh create_with_backend must not publish an empty .pgno seed record"
        );
        for event in 0..3u64 {
            let _ = store
                .writer()
                .begin(P3aZeroSeedPayload(event))
                .expect("begin event");
        }
        let _ = store.writer().sync().expect("sync events");
        let after_sync = JetStreamBackend::open(config(&tag, &rt, &server))
            .replay_all()
            .expect("replay after sync");
        assert_eq!(
            after_sync.len(),
            3,
            "stored messages must equal folded event count after zero-message seed create"
        );
        rt.block_on(delete_stream(&server, &stream_name));
    }
}
