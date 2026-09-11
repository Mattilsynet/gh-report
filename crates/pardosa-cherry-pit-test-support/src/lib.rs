#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, Ordering};

use cherry_pit_core::{
    AggregateId, CorrelationContext, DomainEvent, EventEnvelope, EventStore, StoreCreateResult,
    StoreError,
};
use pardosa::prelude::*;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

const SINGLE_EVENT_ONLY: &str = "PgnoEventStore accepts only single-event batches (create/append); \
     multi-event atomic commit has no primitive in the pardosa substrate today \
     (see bd ghr-00b572de option (a))";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PgnoEnvelope<Ev> {
    event_id: uuid::Uuid,
    aggregate_id: u64,
    sequence: u64,
    timestamp_nanos: i64,
    correlation_id: Option<uuid::Uuid>,
    causation_id: Option<uuid::Uuid>,
    payload: Ev,
}

fn default_claim(epoch: u64) -> OwnershipClaimRecord {
    OwnershipClaimRecord {
        epoch,
        machine_id: [0u8; 16],
        boot_id: [0u8; 16],
        process_id: u64::from(std::process::id()),
        process_start_time_ns: 0,
        claim_time_ns: 0,
        operator_label: "test-support".to_string(),
    }
}

/// Test-only `.pgno`-backed [`EventStore`] adapter over
/// `pardosa`'s facade — bridge crate per CHE-0084:R4-R6.
pub struct PgnoEventStore<Ev: DomainEvent + Serialize + DeserializeOwned + Clone + Send + 'static> {
    session: StdMutex<FileWriterSession>,
    next_id: AtomicU64,
    locks: StdMutex<HashMap<u64, Arc<StdMutex<()>>>>,
    _marker: std::marker::PhantomData<Ev>,
}

impl<Ev: DomainEvent + Serialize + DeserializeOwned + Clone + Send + 'static> PgnoEventStore<Ev> {
    /// Create a fresh `.pgno`-backed store, truncating any existing file.
    ///
    /// # Errors
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot create
    /// the backing container.
    pub fn create_pgno(path: &Path) -> Result<Self, StoreError> {
        let adapter = FileStorageAdapter::new(path);
        let _ = std::fs::remove_file(adapter.meta_path());
        let _ = std::fs::remove_file(adapter.pgno_path());
        let claim = default_claim(1);
        let session = adapter.create(&claim).map_err(to_store_error)?;
        Ok(Self::from_session(session))
    }

    /// Open an existing `.pgno`-backed store, rehydrating its fibers and
    /// seeding the `AggregateId` counter from the max id observed.
    ///
    /// # Errors
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot open
    /// or fold the backing container.
    pub fn open_pgno(path: &Path) -> Result<Self, StoreError> {
        let adapter = FileStorageAdapter::new(path);
        let epoch = adapter.current_epoch().map_err(to_store_error)?;
        let session = adapter.open_write(epoch).map_err(to_store_error)?;
        Ok(Self::from_session(session))
    }

    fn from_session(mut session: FileWriterSession) -> Self {
        let envelopes = session.read_all_envelopes().unwrap_or_default();
        let max_id = envelopes
            .iter()
            .filter_map(|env| {
                let record = serde_json::from_slice::<PgnoEnvelope<Ev>>(&env.payload).ok()?;
                Some(record.aggregate_id)
            })
            .max()
            .unwrap_or(0);
        Self {
            session: StdMutex::new(session),
            next_id: AtomicU64::new(max_id),
            locks: StdMutex::new(HashMap::new()),
            _marker: std::marker::PhantomData,
        }
    }

    fn aggregate_lock(&self, id: u64) -> Arc<StdMutex<()>> {
        let mut locks = self.locks.lock().expect("aggregate-lock map poisoned");
        Arc::clone(
            locks
                .entry(id)
                .or_insert_with(|| Arc::new(StdMutex::new(()))),
        )
    }

    fn ordered_stream(&self, id: AggregateId) -> Result<Vec<EventEnvelope<Ev>>, StoreError> {
        let mut session = self.session.lock().expect("session lock poisoned");
        let envelopes = session.read_all_envelopes().map_err(to_store_error)?;
        let mut result = Vec::new();
        for env in envelopes {
            if env.header.detached {
                continue;
            }
            let record = serde_json::from_slice::<PgnoEnvelope<Ev>>(&env.payload)
                .map_err(|e| StoreError::CorruptData(Box::new(e)))?;
            if record.aggregate_id == id.get() {
                let sequence = NonZeroU64::new(record.sequence).ok_or_else(|| {
                    StoreError::CorruptData(Box::<dyn std::error::Error + Send + Sync>::from(
                        "stored sequence must be non-zero",
                    ))
                })?;
                let timestamp =
                    jiff::Timestamp::from_nanosecond(i128::from(record.timestamp_nanos))
                        .map_err(|e| StoreError::CorruptData(Box::new(e)))?;
                result.push(
                    EventEnvelope::new(
                        record.event_id,
                        id,
                        sequence,
                        timestamp,
                        record.correlation_id,
                        record.causation_id,
                        record.payload,
                    )
                    .map_err(|e| StoreError::CorruptData(Box::new(e)))?,
                );
            }
        }
        result.sort_by_key(EventEnvelope::sequence);
        EventEnvelope::validate_stream(id, &result)
            .map_err(|e| StoreError::CorruptData(Box::new(e)))?;
        Ok(result)
    }

    fn record_single(
        &self,
        aggregate_id: u64,
        event_id: uuid::Uuid,
        sequence: u64,
        correlation_id: Option<uuid::Uuid>,
        causation_id: Option<uuid::Uuid>,
        payload: Ev,
    ) -> Result<(), StoreError> {
        let envelope = PgnoEnvelope {
            event_id,
            aggregate_id,
            sequence,
            timestamp_nanos: i64::try_from(jiff::Timestamp::now().as_nanosecond())
                .unwrap_or(i64::MAX),
            correlation_id,
            causation_id,
            payload,
        };
        let payload_bytes =
            serde_json::to_vec(&envelope).map_err(|e| StoreError::Infrastructure(Box::new(e)))?;
        let key = aggregate_id.to_string();
        let fiber_id = derive_fiber_id(&key);
        let event_id_bytes = *event_id.as_bytes();

        let mut session = self.session.lock().expect("session lock poisoned");
        let handle = session.fiber(fiber_id).map_err(to_store_error)?;
        if handle.is_detached() {
            session
                .rescue_fiber(fiber_id, event_id_bytes, payload_bytes)
                .map_err(to_store_error)?;
        } else {
            session
                .append_to_fiber(fiber_id, event_id_bytes, payload_bytes)
                .map_err(to_store_error)?;
        }
        session.sync().map_err(to_store_error)?;
        Ok(())
    }
}

/// Reusable event fixtures for external consumers of [`PgnoEventStore`].
pub mod fixture {
    use cherry_pit_core::DomainEvent;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[repr(u8)]
    pub enum RecordedEvent {
        Recorded { value: u32 } = 0,
    }

    impl DomainEvent for RecordedEvent {
        fn event_type(&self) -> &'static str {
            "pgno-fixture.recorded"
        }
    }
}

pub mod scheduler_store;
pub use scheduler_store::{
    PgnoSchedulerStore, SchedulerEventConversionError, SchedulerEventDto, from_dto, to_dto,
};

pub mod serde_bridge;
pub use serde_bridge::{PgnoSerdeStore, SerdeBridgeError, SerdeEnvelopeDto};

fn to_store_error(error: impl std::error::Error + Send + Sync + 'static) -> StoreError {
    StoreError::Infrastructure(Box::new(error))
}

fn single_event_error() -> StoreError {
    StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(
        SINGLE_EVENT_ONLY,
    ))
}

impl<Ev: DomainEvent + Serialize + DeserializeOwned + Clone + Send + 'static> EventStore
    for PgnoEventStore<Ev>
{
    type Event = Ev;

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "test-support store operates on sync pardosa state with no I/O to await; the `async` keyword is dictated by the trait signature it implements"
    )]
    async fn load(&self, id: AggregateId) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        self.ordered_stream(id)
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "test-support store operates on sync pardosa state with no I/O to await; the `async` keyword is dictated by the trait signature it implements"
    )]
    async fn create(
        &self,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> StoreCreateResult<Self::Event> {
        if events.len() > 1 {
            return Err(single_event_error());
        }
        let Some(payload) = events.into_iter().next() else {
            return Err(StoreError::Infrastructure(Box::<
                dyn std::error::Error + Send + Sync,
            >::from(
                "cannot create aggregate with zero events",
            )));
        };

        let raw_id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let id = AggregateId::new(NonZeroU64::new(raw_id).ok_or_else(|| {
            StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(
                "aggregate ID overflow",
            ))
        })?);
        let lock = self.aggregate_lock(raw_id);
        let _guard = lock.lock().expect("aggregate lock poisoned");

        let event_id = uuid::Uuid::now_v7();
        self.record_single(
            raw_id,
            event_id,
            1,
            context.correlation_id(),
            context.causation_id(),
            payload,
        )?;
        let envelopes = self.ordered_stream(id)?;
        Ok((id, envelopes))
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "test-support store operates on sync pardosa state with no I/O to await; the `async` keyword is dictated by the trait signature it implements"
    )]
    async fn append(
        &self,
        id: AggregateId,
        expected_sequence: NonZeroU64,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        if events.len() > 1 {
            return Err(single_event_error());
        }
        let Some(payload) = events.into_iter().next() else {
            return Ok(Vec::new());
        };

        let lock = self.aggregate_lock(id.get());
        let _guard = lock.lock().expect("aggregate lock poisoned");

        let existing = self.ordered_stream(id)?;
        if existing.is_empty() {
            return Err(StoreError::Infrastructure(Box::<
                dyn std::error::Error + Send + Sync,
            >::from(format!(
                "append to aggregate {id:?} that was never created"
            ))));
        }
        let actual_sequence = existing.last().map_or(0, |e| e.sequence().get());
        if actual_sequence != expected_sequence.get() {
            return Err(StoreError::ConcurrencyConflict {
                aggregate_id: id,
                expected_sequence,
                actual_sequence,
            });
        }

        let event_id = uuid::Uuid::now_v7();
        self.record_single(
            id.get(),
            event_id,
            expected_sequence.get() + 1,
            context.correlation_id(),
            context.causation_id(),
            payload,
        )?;
        let full_stream = self.ordered_stream(id)?;
        let new_envelope = full_stream
            .into_iter()
            .find(|envelope| envelope.event_id() == event_id)
            .expect("just-recorded envelope must be present in the reloaded stream");
        Ok(vec![new_envelope])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc as StdArc;
    use std::thread;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    enum TestEvent {
        Happened { value: String },
    }

    impl DomainEvent for TestEvent {
        fn event_type(&self) -> &'static str {
            "test.happened"
        }
    }

    fn event(value: &str) -> TestEvent {
        TestEvent::Happened {
            value: value.to_string(),
        }
    }

    fn temp_pgno_path() -> tempfile::TempPath {
        let file = tempfile::NamedTempFile::new().expect("create temp file");
        let path = file.into_temp_path();
        std::fs::remove_file(&path).expect("clear placeholder so create_pgno starts fresh");
        path
    }

    #[tokio::test]
    async fn create_then_load_roundtrip() {
        let path = temp_pgno_path();
        let store = PgnoEventStore::<TestEvent>::create_pgno(&path).expect("create store");

        let (id, created) = store
            .create(vec![event("a")], CorrelationContext::none())
            .await
            .expect("create succeeds");
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].sequence().get(), 1);

        let loaded = store.load(id).await.expect("load succeeds");
        assert_eq!(loaded.len(), created.len());
        assert_eq!(loaded[0].event_id(), created[0].event_id());
        assert_eq!(loaded[0].payload(), created[0].payload());
    }

    #[tokio::test]
    async fn append_single_event_extends_stream() {
        let path = temp_pgno_path();
        let store = PgnoEventStore::<TestEvent>::create_pgno(&path).expect("create store");
        let (id, created) = store
            .create(vec![event("a")], CorrelationContext::none())
            .await
            .expect("create succeeds");
        let expected_sequence = created[0].sequence();

        let appended = store
            .append(
                id,
                expected_sequence,
                vec![event("b")],
                CorrelationContext::none(),
            )
            .await
            .expect("append succeeds");
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].sequence().get(), 2);

        let loaded = store.load(id).await.expect("load succeeds");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[1].event_id(), appended[0].event_id());
        assert_eq!(loaded[1].payload(), appended[0].payload());
    }

    #[tokio::test]
    async fn append_empty_is_noop() {
        let path = temp_pgno_path();
        let store = PgnoEventStore::<TestEvent>::create_pgno(&path).expect("create store");
        let (id, created) = store
            .create(vec![event("a")], CorrelationContext::none())
            .await
            .expect("create succeeds");

        let appended = store
            .append(
                id,
                created[0].sequence(),
                Vec::new(),
                CorrelationContext::none(),
            )
            .await
            .expect("empty append succeeds");
        assert!(appended.is_empty());

        let loaded = store.load(id).await.expect("load succeeds");
        assert_eq!(loaded.len(), 1);
    }

    #[tokio::test]
    async fn append_rejects_wrong_expected_sequence() {
        let path = temp_pgno_path();
        let store = PgnoEventStore::<TestEvent>::create_pgno(&path).expect("create store");
        let (id, _created) = store
            .create(vec![event("a")], CorrelationContext::none())
            .await
            .expect("create succeeds");

        let wrong = NonZeroU64::new(99).unwrap();
        let result = store
            .append(id, wrong, vec![event("b")], CorrelationContext::none())
            .await;

        assert!(matches!(
            result,
            Err(StoreError::ConcurrencyConflict {
                expected_sequence,
                actual_sequence: 1,
                ..
            }) if expected_sequence == wrong
        ));
    }

    #[tokio::test]
    async fn create_rejects_multi_event_batch() {
        let path = temp_pgno_path();
        let store = PgnoEventStore::<TestEvent>::create_pgno(&path).expect("create store");

        let result = store
            .create(vec![event("a"), event("b")], CorrelationContext::none())
            .await;

        assert!(
            matches!(result, Err(StoreError::Infrastructure(e)) if e.to_string().contains("single-event"))
        );
    }

    #[tokio::test]
    async fn append_rejects_multi_event_batch() {
        let path = temp_pgno_path();
        let store = PgnoEventStore::<TestEvent>::create_pgno(&path).expect("create store");
        let (id, created) = store
            .create(vec![event("a")], CorrelationContext::none())
            .await
            .expect("create succeeds");

        let result = store
            .append(
                id,
                created[0].sequence(),
                vec![event("b"), event("c")],
                CorrelationContext::none(),
            )
            .await;

        assert!(
            matches!(result, Err(StoreError::Infrastructure(e)) if e.to_string().contains("single-event"))
        );
    }

    #[tokio::test]
    async fn concurrent_single_appends_one_wins_one_conflicts() {
        let path = temp_pgno_path();
        let store =
            StdArc::new(PgnoEventStore::<TestEvent>::create_pgno(&path).expect("create store"));
        let (id, created) = store
            .create(vec![event("a")], CorrelationContext::none())
            .await
            .expect("create succeeds");
        let expected_sequence = created[0].sequence();

        let store_a = StdArc::clone(&store);
        let store_b = StdArc::clone(&store);
        let handle_a = thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .expect("runtime")
                .block_on(store_a.append(
                    id,
                    expected_sequence,
                    vec![event("racer-a")],
                    CorrelationContext::none(),
                ))
        });
        let handle_b = thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .expect("runtime")
                .block_on(store_b.append(
                    id,
                    expected_sequence,
                    vec![event("racer-b")],
                    CorrelationContext::none(),
                ))
        });

        let result_a = handle_a.join().expect("thread a joins");
        let result_b = handle_b.join().expect("thread b joins");

        let successes = [&result_a, &result_b]
            .into_iter()
            .filter(|r| r.is_ok())
            .count();
        let conflicts = [&result_a, &result_b]
            .into_iter()
            .filter(|r| matches!(r, Err(StoreError::ConcurrencyConflict { .. })))
            .count();
        assert_eq!(successes, 1, "exactly one racer must win the append");
        assert_eq!(conflicts, 1, "exactly one racer must observe the conflict");

        let loaded = store.load(id).await.expect("load succeeds");
        assert_eq!(loaded.len(), 2, "only the winning append landed");
    }

    #[tokio::test]
    async fn restart_recovery_survives_reopen() {
        let path = temp_pgno_path();
        let id;
        {
            let store = PgnoEventStore::<TestEvent>::create_pgno(&path).expect("create store");
            let (created_id, created) = store
                .create(vec![event("a")], CorrelationContext::none())
                .await
                .expect("create succeeds");
            id = created_id;
            store
                .append(
                    id,
                    created[0].sequence(),
                    vec![event("b")],
                    CorrelationContext::none(),
                )
                .await
                .expect("append succeeds");
        }

        let reopened =
            PgnoEventStore::<TestEvent>::open_pgno(&path).expect("reopen existing store");
        let loaded = reopened.load(id).await.expect("load succeeds");
        assert_eq!(loaded.len(), 2, "both events survive restart");
        assert_eq!(loaded[0].sequence().get(), 1);
        assert_eq!(loaded[1].sequence().get(), 2);

        let (new_id, _) = reopened
            .create(vec![event("c")], CorrelationContext::none())
            .await
            .expect("create after reopen assigns a fresh id above the seeded max");
        assert!(
            new_id.get() > id.get(),
            "AggregateId counter must be seeded from the max id observed on open"
        );
    }
}
