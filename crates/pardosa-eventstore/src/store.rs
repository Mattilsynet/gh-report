use std::collections::HashMap;
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

const LOCK_FILENAME: &str = ".lock";
const LOG_FILENAME: &str = "log";

/// Unified-log persistent [`cherry_pit_core::EventStore`].
///
/// All aggregates share one append-only file `<root>/log`; a single
/// `tokio::fs::File` (under a writer mutex) serves every append.
pub struct PardosaLogEventStore<E: DomainEvent> {
    root: PathBuf,
    _lock: RunLock,
    writer: Mutex<tokio::fs::File>,
    aggregates: DashMap<AggregateId, Arc<Mutex<AggregateSlot<E>>>>,
    next_id: AtomicU64,
    _phantom: PhantomData<fn() -> E>,
}

impl<E: DomainEvent> std::fmt::Debug for PardosaLogEventStore<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PardosaLogEventStore")
            .field("root", &self.root)
            .field("streams", &self.aggregates.len())
            .field(
                "next_id",
                &self.next_id.load(std::sync::atomic::Ordering::SeqCst),
            )
            .finish_non_exhaustive()
    }
}

pub(crate) struct AggregateSlot<E: DomainEvent> {
    pub(crate) events: Vec<EventEnvelope<E>>,
    pub(crate) next_seq: u64,
}

impl<E> PardosaLogEventStore<E>
where
    E: DomainEvent + Decode,
{
    /// Open or create the unified-log event store at `root`.
    ///
    /// # Errors
    ///
    /// Returns any [`OpenError`] variant.
    pub async fn open(root: &Path) -> Result<Self, OpenError> {
        tokio::fs::create_dir_all(root)
            .await
            .map_err(|source| OpenError::CreateDir {
                path: root.to_path_buf(),
                source,
            })?;

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

        let log_path = root.join(LOG_FILENAME);
        let recovered = recover_log::<E>(&log_path)?;

        let writer = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .await
            .map_err(|source| OpenError::OpenLog {
                path: log_path.clone(),
                source,
            })?;

        let aggregates: DashMap<AggregateId, Arc<Mutex<AggregateSlot<E>>>> = DashMap::new();
        for (id, slot) in recovered.slots {
            aggregates.insert(id, Arc::new(Mutex::new(slot)));
        }

        let next_id = recovered.max_seen_id.saturating_add(1);

        info!(
            root = %root.display(),
            streams = aggregates.len(),
            envelopes = recovered.envelopes,
            torn_tail_truncated = recovered.truncated_tail,
            next_id,
            "pardosa-eventstore opened"
        );

        Ok(Self {
            root: root.to_path_buf(),
            _lock: lock,
            writer: Mutex::new(writer),
            aggregates,
            next_id: AtomicU64::new(next_id),
            _phantom: PhantomData,
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

struct RecoveredLog<E: DomainEvent> {
    slots: HashMap<AggregateId, AggregateSlot<E>>,
    truncated_tail: bool,
    envelopes: u64,
    max_seen_id: u64,
}

fn recover_log<E>(log_path: &Path) -> Result<RecoveredLog<E>, OpenError>
where
    E: DomainEvent + Decode,
{
    let bytes = match std::fs::read(log_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(source) => {
            return Err(OpenError::ReadLog {
                path: log_path.to_path_buf(),
                source,
            });
        }
    };
    let file_len = bytes.len() as u64;
    let mut cursor = std::io::Cursor::new(&bytes);
    let (bodies, valid_end) =
        read_all_frames_valid(&mut cursor).map_err(|source| OpenError::ReadLog {
            path: log_path.to_path_buf(),
            source,
        })?;

    let mut slots: HashMap<AggregateId, AggregateSlot<E>> = HashMap::new();
    let mut max_seen: u64 = 0;
    for (frame_index, body) in bodies.iter().enumerate() {
        let envelope = pardosa_encoding::from_bytes::<EventEnvelope<E>>(body).map_err(|_| {
            OpenError::DecodeEnvelope {
                path: log_path.to_path_buf(),
                frame_index,
            }
        })?;
        let id = envelope.aggregate_id();
        max_seen = max_seen.max(id.get());
        let slot = slots.entry(id).or_insert_with(|| AggregateSlot {
            events: Vec::new(),
            next_seq: 1,
        });
        let seq = envelope.sequence().get();
        slot.events.push(envelope);
        slot.next_seq = seq + 1;
    }
    let total_envelopes = bodies.len() as u64;

    let truncated = valid_end < file_len;
    if truncated {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(log_path)
            .map_err(|source| OpenError::Truncate {
                path: log_path.to_path_buf(),
                source,
            })?;
        file.set_len(valid_end)
            .map_err(|source| OpenError::Truncate {
                path: log_path.to_path_buf(),
                source,
            })?;
        file.sync_all().map_err(|source| OpenError::Truncate {
            path: log_path.to_path_buf(),
            source,
        })?;
    }

    Ok(RecoveredLog {
        slots,
        truncated_tail: truncated,
        envelopes: total_envelopes,
        max_seen_id: max_seen,
    })
}

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

fn encode_envelope<E: DomainEvent + pardosa_encoding::Encode>(
    envelope: &EventEnvelope<E>,
) -> Vec<u8> {
    pardosa_encoding::to_vec(envelope)
}

/// Persist a batch under the shared writer mutex: `write_all` + `fsync`,
/// then return. In-memory state mutates only after this returns Ok.
async fn persist_batch<E: DomainEvent + pardosa_encoding::Encode>(
    writer: &mut tokio::fs::File,
    envelopes: &[EventEnvelope<E>],
) -> Result<(), StoreError> {
    for envelope in envelopes {
        let body = encode_envelope(envelope);
        let mut frame_buf = Vec::with_capacity(body.len() + 12);
        write_frame(&mut frame_buf, &body).map_err(|e| {
            StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(format!(
                "frame encode: {e}"
            )))
        })?;
        writer.write_all(&frame_buf).await.map_err(|e| {
            StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(format!(
                "write frame: {e}"
            )))
        })?;
    }
    writer.sync_all().await.map_err(|e| {
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
        let Some(slot_arc) = self.aggregates.get(&id).map(|r| Arc::clone(r.value())) else {
            return Ok(Vec::new());
        };
        let guard = slot_arc.lock().await;
        let events = guard.events.clone();
        drop(guard);
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

        let raw_id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let nz = NonZeroU64::new(raw_id).ok_or_else(|| {
            StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(
                "aggregate id allocator yielded zero",
            ))
        })?;
        let id = AggregateId::new(nz);

        let envelopes = build_envelopes(id, 0, events, &context)?;

        let mut writer = self.writer.lock().await;
        persist_batch(&mut writer, &envelopes).await?;
        drop(writer);

        let next_seq = envelopes.last().map_or(1u64, |e| e.sequence().get() + 1);
        let slot = Arc::new(Mutex::new(AggregateSlot {
            events: envelopes.clone(),
            next_seq,
        }));
        self.aggregates.insert(id, slot);

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
            return Ok(Vec::new());
        }

        let Some(slot_arc) = self.aggregates.get(&id).map(|r| Arc::clone(r.value())) else {
            return Err(StoreError::Infrastructure(Box::<
                dyn std::error::Error + Send + Sync,
            >::from(format!(
                "cannot append to aggregate {id}: not created (use create() first)"
            ))));
        };

        // Slot mutex serialises per-aggregate sequence assignment; the
        // writer mutex serialises the shared file. Lock order is
        // slot-first then writer to keep the optimistic check coherent
        // with the persisted prefix.
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

        let mut writer = self.writer.lock().await;
        persist_batch(&mut writer, &envelopes).await?;
        drop(writer);

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
        Ok(self.aggregates.iter().map(|entry| *entry.key()).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(store.aggregates.is_empty());
        assert_eq!(store.next_id.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(dir.path().join(".lock").exists());
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
