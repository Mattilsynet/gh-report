//! [`PardosaLogEventStore`] — file-per-aggregate persistent event store.
//!
//! Layout: `<root>/<aggregate_id>.log` per stream, `<root>/.lock` for
//! single-writer exclusion. Each log file is a sequence of length-prefixed
//! xxh64-trailered [`pardosa_encoding::to_vec(&EventEnvelope<E>)`] frames.
//!
//! Recovery on [`open`](PardosaLogEventStore::open): scan the root, for
//! each `<digits>.log` run [`frame::read_all_frames_valid`], decode each
//! body into `EventEnvelope<E>`, truncate any torn tail back to the last
//! valid frame boundary, and rebuild the in-memory slot. The store's
//! `next_id` is seeded as `max(seen_ids) + 1`.

use std::collections::HashSet;
use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use cherry_pit_core::{
    AggregateId, CorrelationContext, DomainEvent, EventEnvelope, EventStore, ListableEventStore,
    StoreCreateResult, StoreError,
};
use cherry_pit_storage::{DEFAULT_LOCK_TTL, RunLock, acquire};
use dashmap::DashMap;
use pardosa_encoding::Decode;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tracing::info;

use crate::error::OpenError;
use crate::frame::{read_all_frames_valid, write_frame};

/// Filename of the per-root `RunLock` lock file.
///
/// CHE-0043:R1 mandates `<store_dir>/.lock` for `MsgpackFileStore`; the
/// same pattern is adopted here for cross-substrate consistency.
const LOCK_FILENAME: &str = ".lock";

/// Suffix for per-aggregate log files. Filename is `<aggregate_id>.log`
/// where `<aggregate_id>` is a base-10 `u64` ≥ 1.
const LOG_SUFFIX: &str = ".log";

/// File-per-aggregate persistent [`cherry_pit_core::EventStore`].
///
/// Generic over the domain-event type `E`. The struct itself does not
/// require `E: Decode` — that bound is added at the impl sites that
/// deserialise (per CHE-0064 δ.3a-pre).
pub struct PardosaLogEventStore<E: DomainEvent> {
    root: PathBuf,
    /// RAII guard — drop releases the `.lock` file.
    _lock: RunLock,
    /// Per-aggregate writer slots. The mutex serialises append IO on a
    /// single stream while allowing disjoint aggregates to proceed in
    /// parallel.
    slots: DashMap<AggregateId, Arc<Mutex<AggregateSlot<E>>>>,
    /// Next aggregate id to assign on `create`. Seeded from boot scan.
    next_id: AtomicU64,
    _phantom: PhantomData<fn() -> E>,
}

impl<E: DomainEvent> std::fmt::Debug for PardosaLogEventStore<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PardosaLogEventStore")
            .field("root", &self.root)
            .field("streams", &self.slots.len())
            .field(
                "next_id",
                &self.next_id.load(std::sync::atomic::Ordering::SeqCst),
            )
            .finish_non_exhaustive()
    }
}

/// In-memory state of a single aggregate's stream.
///
/// Fields are populated by [`PardosaLogEventStore::open`] but only
/// consumed by `append` / `load` in Q.1.3; silence the read-side
/// dead-code lint until that wiring lands.
#[allow(dead_code)]
pub(crate) struct AggregateSlot<E: DomainEvent> {
    /// Append-mode handle to `<root>/<id>.log`. New frames go to the end.
    pub(crate) file: tokio::fs::File,
    /// Decoded envelope history, sequence-ordered (1..=N).
    pub(crate) events: Vec<EventEnvelope<E>>,
    /// Next sequence number to assign on append. Equals
    /// `events.last().sequence().get() + 1`, or 1 when empty.
    pub(crate) next_seq: u64,
}

impl<E> PardosaLogEventStore<E>
where
    E: DomainEvent + Decode,
{
    /// Open or create the event store at `root`.
    ///
    /// 1. `create_dir_all(root)` so we can lock and write.
    /// 2. Acquire `<root>/.lock` via [`cherry_pit_storage::acquire`].
    /// 3. Scan `root` for `<digits>.log`; reject any other entry as
    ///    [`OpenError::UnknownFile`] except the lock file itself.
    /// 4. For each log: frame-scan, decode every body as
    ///    `EventEnvelope<E>`, truncate any torn tail, populate a slot.
    /// 5. Seed `next_id = max(seen_id) + 1` (or 1 if empty).
    ///
    /// # Errors
    ///
    /// Returns any [`OpenError`] variant — see the type for the
    /// catalogue. All failures are recoverable by operator action
    /// (clearing a stale lock, repairing or removing a malformed file).
    pub async fn open(root: &Path) -> Result<Self, OpenError> {
        // (1) Ensure the directory exists.
        tokio::fs::create_dir_all(root)
            .await
            .map_err(|source| OpenError::CreateDir {
                path: root.to_path_buf(),
                source,
            })?;

        // (2) Acquire the run lock. `acquire` is synchronous; the call
        // is bounded (a few sub-millisecond filesystem ops) so we run
        // it inline rather than offloading to `spawn_blocking`.
        let run_id = format!(
            "pardosa-eventstore-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        );
        let lock =
            acquire(root, &run_id, DEFAULT_LOCK_TTL, false, LOCK_FILENAME).map_err(|source| {
                OpenError::Lock {
                    path: root.join(LOCK_FILENAME),
                    source,
                }
            })?;

        // (3) Scan the directory. Collect filenames first so we can sort
        // for deterministic recovery order (helps the boot-log read like
        // a sequence of independent decisions).
        let mut entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(root)
            .await
            .map_err(|source| OpenError::Scan {
                path: root.to_path_buf(),
                source,
            })?;
        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|source| OpenError::Scan {
                path: root.to_path_buf(),
                source,
            })?
        {
            entries.push(entry.path());
        }
        entries.sort();

        let slots: DashMap<AggregateId, Arc<Mutex<AggregateSlot<E>>>> = DashMap::new();
        let mut seen_ids: HashSet<u64> = HashSet::new();
        let mut total_envelopes: u64 = 0;
        let mut truncated_tails: u64 = 0;

        for path in entries {
            let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if file_name == LOCK_FILENAME {
                continue;
            }
            let Some(stem) = file_name.strip_suffix(LOG_SUFFIX) else {
                return Err(OpenError::UnknownFile { path });
            };
            let aggregate_u64: u64 = stem
                .parse()
                .map_err(|_| OpenError::UnknownFile { path: path.clone() })?;
            let Some(nz) = NonZeroU64::new(aggregate_u64) else {
                return Err(OpenError::UnknownFile { path });
            };
            let aggregate_id = AggregateId::new(nz);

            let (envelopes, truncated, file) = recover_stream::<E>(&path, aggregate_id).await?;
            total_envelopes += envelopes.len() as u64;
            if truncated {
                truncated_tails += 1;
            }

            let next_seq = envelopes.last().map_or(1u64, |e| e.sequence().get() + 1);
            seen_ids.insert(aggregate_u64);
            slots.insert(
                aggregate_id,
                Arc::new(Mutex::new(AggregateSlot {
                    file,
                    events: envelopes,
                    next_seq,
                })),
            );
        }

        // (5) Seed next_id.
        let next_id = seen_ids
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
            .saturating_add(1);

        info!(
            root = %root.display(),
            streams = slots.len(),
            envelopes = total_envelopes,
            truncated_tails,
            next_id,
            "pardosa-eventstore opened"
        );

        Ok(Self {
            root: root.to_path_buf(),
            _lock: lock,
            slots,
            next_id: AtomicU64::new(next_id),
            _phantom: PhantomData,
        })
    }

    /// The root directory backing this store. Useful for diagnostics
    /// and tests; production code should not depend on it.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Recover one aggregate's stream from disk.
///
/// Reads every well-formed frame, decodes each as `EventEnvelope<E>`,
/// truncates the file to the last valid boundary if a torn tail was
/// detected, and returns an append-mode handle ready for new frames.
async fn recover_stream<E>(
    path: &Path,
    aggregate_id: AggregateId,
) -> Result<(Vec<EventEnvelope<E>>, bool, tokio::fs::File), OpenError>
where
    E: DomainEvent + Decode,
{
    // Read the file via std::fs (cheap; the recovery path is one-shot at
    // boot, not on the hot append path). This keeps `frame::read_all_frames_valid`
    // a pure synchronous helper.
    let bytes = std::fs::read(path).map_err(|source| OpenError::ReadLog {
        path: path.to_path_buf(),
        source,
    })?;
    let file_len = bytes.len() as u64;
    let mut cursor = std::io::Cursor::new(&bytes);
    let (bodies, valid_end) =
        read_all_frames_valid(&mut cursor).map_err(|source| OpenError::ReadLog {
            path: path.to_path_buf(),
            source,
        })?;

    let mut envelopes = Vec::with_capacity(bodies.len());
    for (frame_index, body) in bodies.iter().enumerate() {
        let envelope = pardosa_encoding::from_bytes::<EventEnvelope<E>>(body).map_err(|_| {
            OpenError::DecodeEnvelope {
                path: path.to_path_buf(),
                frame_index,
            }
        })?;
        // Cross-stream defence: a renamed file should not silently bind
        // its envelopes to a new id. `validate_stream` runs later (per
        // the trait contract on `load`); here we just reject the
        // simplest cross-stream mistake upfront so recovery fails loud.
        if envelope.aggregate_id() != aggregate_id {
            return Err(OpenError::DecodeEnvelope {
                path: path.to_path_buf(),
                frame_index,
            });
        }
        envelopes.push(envelope);
    }

    let truncated = valid_end < file_len;
    if truncated {
        // Truncate via std::fs::OpenOptions — we need write access
        // briefly, distinct from the append handle returned below.
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|source| OpenError::Truncate {
                path: path.to_path_buf(),
                source,
            })?;
        file.set_len(valid_end)
            .map_err(|source| OpenError::Truncate {
                path: path.to_path_buf(),
                source,
            })?;
    }

    // Open the append handle that the runtime store will hold.
    let file = OpenOptions::new()
        .append(true)
        .open(path)
        .await
        .map_err(|source| OpenError::OpenLog {
            path: path.to_path_buf(),
            source,
        })?;

    Ok((envelopes, truncated, file))
}

// `write_frame` is part of the frame module's public-within-crate API;
// keep the symbol live for Q.1.3 (`append` / `create` write frames).
#[allow(dead_code)]
fn _keep_write_frame_alive<W: std::io::Write>(w: &mut W, body: &[u8]) -> std::io::Result<()> {
    write_frame(w, body)
}

// ─── EventStore impl (Q.1.3) ────────────────────────────────────────
//
// Persist-then-publish discipline (CHE-0024:R2): we serialise the
// envelope, frame it, write it to the append-mode file handle, fsync
// (`sync_all`), and only then mutate in-memory state and return
// success. A process kill at any point before `sync_all` returns
// leaves the on-disk frame either fully present or absent (the xxh64
// trailer makes torn tails self-evident on recovery); after `sync_all`
// returns the frame is durable. In-memory state always trails durable
// state — a torn writer surfaces on the next `open()` and is recovered
// by truncating the torn tail (see `recover_stream`).
//
// The per-slot `tokio::sync::Mutex` is held across the entire
// write + fsync. This is the intended discipline: it serialises
// appenders on a single stream so the on-disk sequence and the
// in-memory `next_seq` never diverge under concurrent writers.
// `clippy::await_holding_lock` is *not* the right lint here — it
// targets `std::sync::Mutex` (a blocking primitive); the tokio mutex
// is async-aware and designed to be held across `.await`. We do not
// suppress the lint at module scope because clippy correctly does not
// fire on `tokio::sync::Mutex`; if it ever does, an `#[allow]` at the
// impl site with this rationale is the right fix.

/// Pre-encode an `EventEnvelope` to a single Vec for `write_frame`.
///
/// Serialisation runs outside the per-slot mutex (no shared state),
/// then the frame write happens under the mutex. This minimises the
/// critical section to file I/O only.
fn encode_envelope<E: DomainEvent + pardosa_encoding::Encode>(
    envelope: &EventEnvelope<E>,
) -> Vec<u8> {
    pardosa_encoding::to_vec(envelope)
}

/// Build one envelope batch — mirrors `cherry_pit_core::testing::build_envelopes`.
///
/// One shared timestamp per batch (atomic), `event_id` via `uuid::Uuid::now_v7`
/// (CHE-0033:R1). Sequence starts at `start_sequence + 1`.
fn build_envelopes<E: DomainEvent>(
    id: AggregateId,
    start_sequence: u64,
    events: Vec<E>,
    context: &CorrelationContext,
) -> Result<Vec<EventEnvelope<E>>, StoreError> {
    let timestamp = jiff::Timestamp::now();
    let mut envelopes = Vec::with_capacity(events.len());
    for (i, payload) in events.into_iter().enumerate() {
        let i_u64 = u64::try_from(i).unwrap_or(u64::MAX);
        let raw = start_sequence
            .checked_add(i_u64)
            .and_then(|s| s.checked_add(1))
            .ok_or_else(|| {
                StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(
                    "sequence overflow",
                ))
            })?;
        let sequence = NonZeroU64::new(raw).ok_or_else(|| {
            StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(
                "sequence must be non-zero",
            ))
        })?;
        let envelope = EventEnvelope::new(
            uuid::Uuid::now_v7(),
            id,
            sequence,
            timestamp,
            context.correlation_id(),
            context.causation_id(),
            payload,
        )
        .map_err(|e| StoreError::Infrastructure(Box::new(e)))?;
        envelopes.push(envelope);
    }
    Ok(envelopes)
}

/// Persist a batch of envelopes to an open append handle: one framed
/// write per envelope, followed by a single `sync_all`.
///
/// Holds the caller's mutex guard for the entire duration — this is the
/// intentional persist-then-publish boundary.
async fn persist_batch<E: DomainEvent + pardosa_encoding::Encode>(
    file: &mut tokio::fs::File,
    envelopes: &[EventEnvelope<E>],
) -> Result<(), StoreError> {
    for envelope in envelopes {
        let body = encode_envelope(envelope);
        // Build the full frame in memory then issue one write_all — this
        // matches `write_frame`'s contract but avoids the sync-only
        // signature mismatch (the helper is `std::io::Write`-based).
        let mut frame_buf = Vec::with_capacity(body.len() + 12);
        write_frame(&mut frame_buf, &body).map_err(|e| {
            StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(format!(
                "frame encode: {e}"
            )))
        })?;
        file.write_all(&frame_buf).await.map_err(|e| {
            StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(format!(
                "write frame: {e}"
            )))
        })?;
    }
    file.sync_all().await.map_err(|e| {
        StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(format!(
            "fsync: {e}"
        )))
    })?;
    Ok(())
}

impl<E> EventStore for PardosaLogEventStore<E>
where
    E: DomainEvent + Decode,
{
    type Event = E;

    async fn load(&self, id: AggregateId) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        let Some(slot_arc) = self.slots.get(&id).map(|r| Arc::clone(r.value())) else {
            // CHE-0019:R1 — unknown aggregate returns empty vec, not error.
            return Ok(Vec::new());
        };
        let guard = slot_arc.lock().await;
        let events = guard.events.clone();
        drop(guard);
        // CHE-0042:R4 — honour the conformance shape even though in-process
        // construction makes corruption structurally impossible.
        EventEnvelope::validate_stream(id, &events)
            .map_err(|e| StoreError::CorruptData(Box::new(e)))?;
        Ok(events)
    }

    async fn create(
        &self,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> StoreCreateResult<Self::Event> {
        if events.is_empty() {
            return Err(StoreError::Infrastructure(Box::<
                dyn std::error::Error + Send + Sync,
            >::from(
                "cannot create aggregate with zero events",
            )));
        }

        // Allocate a fresh id. The boot scan seeded `next_id` past the
        // max persisted id; `fetch_add` then advances monotonically.
        // Concurrent creators race on `fetch_add` and each gets a
        // distinct id; the `create_new(true)` O_EXCL open below is the
        // final defence against any aliasing.
        let raw_id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let nz = NonZeroU64::new(raw_id).ok_or_else(|| {
            StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(
                "aggregate id allocator yielded zero",
            ))
        })?;
        let id = AggregateId::new(nz);

        // O_EXCL create — if the file exists, the allocator is wrong
        // (would only happen on a stale `next_id`); surface loudly.
        let path = self.root.join(format!("{raw_id}.log"));
        let mut file = OpenOptions::new()
            .write(true)
            .append(true)
            .create_new(true)
            .open(&path)
            .await
            .map_err(|e| {
                StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(
                    format!("create {}: {e}", path.display()),
                ))
            })?;

        let envelopes = build_envelopes(id, 0, events, &context)?;
        persist_batch(&mut file, &envelopes).await?;

        let next_seq = envelopes.last().map_or(1u64, |e| e.sequence().get() + 1);
        let slot = Arc::new(Mutex::new(AggregateSlot {
            file,
            events: envelopes.clone(),
            next_seq,
        }));
        self.slots.insert(id, slot);

        Ok((id, envelopes))
    }

    async fn append(
        &self,
        id: AggregateId,
        expected_sequence: NonZeroU64,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        if events.is_empty() {
            // CHE-0005 / store.rs:242 — empty append is a no-op.
            return Ok(Vec::new());
        }

        let Some(slot_arc) = self.slots.get(&id).map(|r| Arc::clone(r.value())) else {
            return Err(StoreError::Infrastructure(Box::<
                dyn std::error::Error + Send + Sync,
            >::from(format!(
                "cannot append to aggregate {id}: not created (use create() first)"
            ))));
        };

        // Hold the per-slot mutex across the optimistic check, the
        // write, and the fsync. This is the persist-then-publish
        // boundary: in-memory `next_seq` only advances after `sync_all`
        // returns. `tokio::sync::Mutex` is async-aware; holding it
        // across `.await` is correct and supported.
        let mut guard = slot_arc.lock().await;

        let actual_sequence = guard.next_seq.saturating_sub(1);
        if actual_sequence != expected_sequence.get() {
            return Err(StoreError::ConcurrencyConflict {
                aggregate_id: id,
                expected_sequence,
                actual_sequence,
            });
        }

        let envelopes = build_envelopes(id, expected_sequence.get(), events, &context)?;
        persist_batch(&mut guard.file, &envelopes).await?;

        guard.events.extend(envelopes.iter().cloned());
        if let Some(last) = guard.events.last() {
            guard.next_seq = last.sequence().get() + 1;
        }
        Ok(envelopes)
    }
}

impl<E> ListableEventStore for PardosaLogEventStore<E>
where
    E: DomainEvent + Decode,
{
    fn list_aggregates(&self) -> Result<Vec<AggregateId>, StoreError> {
        Ok(self.slots.iter().map(|entry| *entry.key()).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trivial `DomainEvent + Encode + Decode` fixture for the
    /// open-on-empty-dir test. Mirrors the pattern in
    /// `cherry-pit-core::event::tests::TestEvent` but lives here so the
    /// test crate has no extra dep on cherry-pit-core's test-only
    /// surface.
    #[derive(Debug, Clone, PartialEq)]
    enum TestEvent {
        Tick,
    }

    impl DomainEvent for TestEvent {
        fn event_type(&self) -> &'static str {
            "test.tick"
        }
    }

    impl pardosa_encoding::Encode for TestEvent {
        fn encode(&self, out: &mut Vec<u8>) {
            match self {
                TestEvent::Tick => out.push(0u8),
            }
        }
    }

    impl pardosa_encoding::Decode for TestEvent {
        fn decode(
            d: &mut pardosa_encoding::Decoder<'_>,
        ) -> Result<Self, pardosa_encoding::EventError> {
            let tag = <u8 as pardosa_encoding::Decode>::decode(d)?;
            match tag {
                0 => Ok(TestEvent::Tick),
                _ => Err(pardosa_encoding::EventError::InvalidInput),
            }
        }
    }

    #[tokio::test]
    async fn open_on_empty_dir_succeeds_with_next_id_1() {
        let dir = tempfile::tempdir().unwrap();
        let store = PardosaLogEventStore::<TestEvent>::open(dir.path())
            .await
            .expect("open empty dir");
        assert!(store.slots.is_empty());
        assert_eq!(store.next_id.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(dir.path().join(".lock").exists());
    }

    #[tokio::test]
    async fn open_rejects_unknown_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("not-a-log.txt"), b"garbage").unwrap();
        let err = PardosaLogEventStore::<TestEvent>::open(dir.path())
            .await
            .expect_err("unknown file must reject");
        assert!(matches!(err, OpenError::UnknownFile { .. }));
    }

    #[tokio::test]
    async fn second_open_on_same_root_blocks_on_lock() {
        let dir = tempfile::tempdir().unwrap();
        let _first = PardosaLogEventStore::<TestEvent>::open(dir.path())
            .await
            .expect("first open");
        let err = PardosaLogEventStore::<TestEvent>::open(dir.path())
            .await
            .expect_err("second open must fail while first holds lock");
        assert!(matches!(err, OpenError::Lock { .. }));
    }

    #[tokio::test]
    async fn create_then_load_returns_envelopes() {
        let dir = tempfile::tempdir().unwrap();
        let store = PardosaLogEventStore::<TestEvent>::open(dir.path())
            .await
            .unwrap();
        let (id, envelopes) = store
            .create(
                vec![TestEvent::Tick, TestEvent::Tick],
                CorrelationContext::none(),
            )
            .await
            .expect("create");
        assert_eq!(envelopes.len(), 2);
        assert_eq!(envelopes[0].sequence().get(), 1);
        assert_eq!(envelopes[1].sequence().get(), 2);
        assert_eq!(id.get(), 1);

        let loaded = store.load(id).await.expect("load");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].event_id(), envelopes[0].event_id());
    }

    #[tokio::test]
    async fn load_unknown_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = PardosaLogEventStore::<TestEvent>::open(dir.path())
            .await
            .unwrap();
        let id = AggregateId::new(NonZeroU64::new(99).unwrap());
        let loaded = store.load(id).await.expect("load");
        assert!(loaded.is_empty());
    }

    #[tokio::test]
    async fn create_rejects_empty_events() {
        let dir = tempfile::tempdir().unwrap();
        let store = PardosaLogEventStore::<TestEvent>::open(dir.path())
            .await
            .unwrap();
        let err = store
            .create(vec![], CorrelationContext::none())
            .await
            .expect_err("empty create must fail");
        assert!(matches!(err, StoreError::Infrastructure(_)));
    }

    #[tokio::test]
    async fn append_advances_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let store = PardosaLogEventStore::<TestEvent>::open(dir.path())
            .await
            .unwrap();
        let (id, created) = store
            .create(vec![TestEvent::Tick], CorrelationContext::none())
            .await
            .unwrap();
        let last_seq = created.last().unwrap().sequence();

        let appended = store
            .append(
                id,
                last_seq,
                vec![TestEvent::Tick, TestEvent::Tick],
                CorrelationContext::none(),
            )
            .await
            .expect("append");
        assert_eq!(appended.len(), 2);
        assert_eq!(appended[0].sequence().get(), 2);
        assert_eq!(appended[1].sequence().get(), 3);

        let loaded = store.load(id).await.unwrap();
        assert_eq!(loaded.len(), 3);
    }

    #[tokio::test]
    async fn append_with_wrong_expected_sequence_returns_concurrency_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let store = PardosaLogEventStore::<TestEvent>::open(dir.path())
            .await
            .unwrap();
        let (id, _) = store
            .create(vec![TestEvent::Tick], CorrelationContext::none())
            .await
            .unwrap();
        // Last seq is 1; supplying 5 should conflict.
        let err = store
            .append(
                id,
                NonZeroU64::new(5).unwrap(),
                vec![TestEvent::Tick],
                CorrelationContext::none(),
            )
            .await
            .expect_err("wrong expected_sequence must conflict");
        match err {
            StoreError::ConcurrencyConflict {
                aggregate_id,
                expected_sequence,
                actual_sequence,
            } => {
                assert_eq!(aggregate_id, id);
                assert_eq!(expected_sequence.get(), 5);
                assert_eq!(actual_sequence, 1);
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn append_empty_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let store = PardosaLogEventStore::<TestEvent>::open(dir.path())
            .await
            .unwrap();
        let (id, _) = store
            .create(vec![TestEvent::Tick], CorrelationContext::none())
            .await
            .unwrap();
        let appended = store
            .append(
                id,
                NonZeroU64::new(1).unwrap(),
                vec![],
                CorrelationContext::none(),
            )
            .await
            .expect("empty append is no-op");
        assert!(appended.is_empty());
    }

    #[tokio::test]
    async fn append_to_unknown_aggregate_errors() {
        let dir = tempfile::tempdir().unwrap();
        let store = PardosaLogEventStore::<TestEvent>::open(dir.path())
            .await
            .unwrap();
        let id = AggregateId::new(NonZeroU64::new(42).unwrap());
        let err = store
            .append(
                id,
                NonZeroU64::new(1).unwrap(),
                vec![TestEvent::Tick],
                CorrelationContext::none(),
            )
            .await
            .expect_err("append to never-created aggregate must error");
        assert!(matches!(err, StoreError::Infrastructure(_)));
    }

    #[tokio::test]
    async fn list_aggregates_returns_all_created_ids() {
        let dir = tempfile::tempdir().unwrap();
        let store = PardosaLogEventStore::<TestEvent>::open(dir.path())
            .await
            .unwrap();
        let (id1, _) = store
            .create(vec![TestEvent::Tick], CorrelationContext::none())
            .await
            .unwrap();
        let (id2, _) = store
            .create(vec![TestEvent::Tick], CorrelationContext::none())
            .await
            .unwrap();
        let mut listed = store.list_aggregates().expect("list");
        listed.sort_by_key(|a| a.get());
        assert_eq!(listed, vec![id1, id2]);
    }
}
