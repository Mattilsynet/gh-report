use crate::payload::EnvelopePayload;
use cherry_pit_core::{
    AggregateId, CorrelationContext, DomainEvent, EventEnvelope, EventStore, ListableEventStore,
    SingleWriterEventStore, StoreCreateResult, StoreError,
};
use pardosa::store::{Event, EventStore as PardosaStore, JetStreamBackend, PgnoBackend};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::path::Path;
use std::sync::Mutex;

pub struct PardosaEventStore<E: DomainEvent> {
    inner: Mutex<InnerStore<E>>,
}

struct InnerStore<E: DomainEvent> {
    store: PardosaStore<EnvelopePayload>,
    index: BTreeMap<AggregateId, Vec<EventEnvelope<E>>>,
    _event: PhantomData<fn() -> E>,
}

#[derive(Clone)]
struct CapturedEnvelope {
    envelope_bytes: Vec<u8>,
}

impl<E: DomainEvent> PardosaEventStore<E> {
    /// Create a `.pgno`-backed adapter over a new pardosa store.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] if pardosa cannot create
    /// or fold the backing store. Returns [`StoreError::CorruptData`] if
    /// re-reading the newly-created store yields invalid payloads.
    pub fn create_pgno(path: &Path) -> Result<Self, StoreError> {
        let store = PardosaStore::<EnvelopePayload>::create(path).map_err(infrastructure_error)?;
        Self::from_pardosa_store(store)
    }

    /// Open a `.pgno`-backed adapter and capture its logical stream index.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] if pardosa cannot open or
    /// fold the backing store. Returns [`StoreError::CorruptData`] if any
    /// captured payload cannot be decoded into `EventEnvelope<E>` or fails
    /// stream validation.
    pub fn open_pgno(path: &Path) -> Result<Self, StoreError> {
        let store = PardosaStore::<EnvelopePayload>::open_with_backend(PgnoBackend::open(path))
            .map_err(infrastructure_error)?;
        Self::from_pardosa_store(store)
    }

    /// Open a JetStream-backed adapter and capture its logical stream index.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] if pardosa cannot fetch or
    /// rehydrate the JetStream-authoritative blob. Returns
    /// [`StoreError::CorruptData`] if any captured payload cannot be
    /// decoded into `EventEnvelope<E>` or fails stream validation.
    pub fn open_jetstream(backend: JetStreamBackend) -> Result<Self, StoreError> {
        let store = PardosaStore::<EnvelopePayload>::open_with_backend(backend)
            .map_err(infrastructure_error)?;
        Self::from_pardosa_store(store)
    }

    fn from_pardosa_store(store: PardosaStore<EnvelopePayload>) -> Result<Self, StoreError> {
        let index = capture_index::<E>(&store)?;
        Ok(Self {
            inner: Mutex::new(InnerStore {
                store,
                index,
                _event: PhantomData,
            }),
        })
    }

    /// Return aggregate ids captured during adapter open.
    ///
    /// # Errors
    ///
    /// This method returns `Ok` for a successfully-opened adapter. The
    /// `Result` shape mirrors the async `ListableEventStore` contract.
    pub fn list_indexed_aggregates(&self) -> Result<Vec<AggregateId>, StoreError> {
        let inner = self.lock_inner()?;
        Ok(inner.index.keys().copied().collect())
    }

    /// Load an aggregate stream from the captured open-time index.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::CorruptData`] when captured envelope bytes
    /// fail to decode or when the gathered stream is not gap-free for
    /// `id`.
    pub fn load_indexed(&self, id: AggregateId) -> Result<Vec<EventEnvelope<E>>, StoreError> {
        let inner = self.lock_inner()?;
        Ok(inner.index.get(&id).cloned().unwrap_or_default())
    }

    fn lock_inner(&self) -> Result<std::sync::MutexGuard<'_, InnerStore<E>>, StoreError> {
        self.inner
            .lock()
            .map_err(|_| infrastructure_error("pardosa event store mutex poisoned"))
    }
}

fn capture_index<E: DomainEvent>(
    store: &PardosaStore<EnvelopePayload>,
) -> Result<BTreeMap<AggregateId, Vec<EventEnvelope<E>>>, StoreError> {
    let captured = RefCell::new(Vec::<(u64, Vec<u8>)>::new());
    let extractor = |event: &Event<EnvelopePayload>| -> std::iter::Once<u64> {
        let payload = event.domain_event();
        captured
            .borrow_mut()
            .push((payload.aggregate_id, payload.envelope_bytes().to_vec()));
        std::iter::once(payload.aggregate_id)
    };
    let _ = store.reader().fiber_index::<u64, _, _>(extractor);

    let mut captured_index = BTreeMap::<AggregateId, Vec<CapturedEnvelope>>::new();
    let mut aggregate_ids = BTreeSet::<AggregateId>::new();
    for (raw_id, envelope_bytes) in captured.into_inner() {
        let aggregate_id = aggregate_id(raw_id)?;
        aggregate_ids.insert(aggregate_id);
        captured_index
            .entry(aggregate_id)
            .or_default()
            .push(CapturedEnvelope { envelope_bytes });
    }
    let mut index = BTreeMap::<AggregateId, Vec<EventEnvelope<E>>>::new();
    for id in aggregate_ids {
        let envelopes = decode_stream::<E, _>(id, captured_index.get(&id).into_iter().flatten())?;
        index.insert(id, envelopes);
    }
    Ok(index)
}

fn decode_stream<'a, E, I>(
    id: AggregateId,
    captures: I,
) -> Result<Vec<EventEnvelope<E>>, StoreError>
where
    E: DomainEvent,
    I: IntoIterator<Item = &'a CapturedEnvelope>,
{
    let mut envelopes = captures
        .into_iter()
        .map(|capture| {
            rmp_serde::from_slice::<EventEnvelope<E>>(&capture.envelope_bytes)
                .map_err(corrupt_data)
        })
        .collect::<Result<Vec<_>, _>>()?;
    envelopes.sort_by_key(EventEnvelope::sequence);
    EventEnvelope::validate_stream(id, &envelopes).map_err(corrupt_data)?;
    Ok(envelopes)
}

fn aggregate_id(raw_id: u64) -> Result<AggregateId, StoreError> {
    NonZeroU64::new(raw_id)
        .map(AggregateId::new)
        .ok_or_else(|| corrupt_data("aggregate_id must be non-zero"))
}

impl<E: DomainEvent> EventStore for PardosaEventStore<E> {
    type Event = E;

    async fn load(&self, id: AggregateId) -> Result<Vec<EventEnvelope<E>>, StoreError> {
        self.load_indexed(id)
    }

    async fn create(
        &self,
        events: Vec<E>,
        context: CorrelationContext,
    ) -> StoreCreateResult<E> {
        if events.is_empty() {
            return Err(infrastructure_error(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot create aggregate with zero events",
            )));
        }
        let mut inner = self.lock_inner()?;
        let id = next_aggregate_id(&inner.index)?;
        let envelopes = build_envelopes(id, 0, events, &context)?;
        persist_envelopes(&mut inner.store, id, &envelopes)?;
        inner.index.insert(id, envelopes.clone());
        Ok((id, envelopes))
    }

    async fn append(
        &self,
        id: AggregateId,
        expected_sequence: NonZeroU64,
        events: Vec<E>,
        context: CorrelationContext,
    ) -> Result<Vec<EventEnvelope<E>>, StoreError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let mut inner = self.lock_inner()?;
        let existing = inner.index.get(&id).ok_or_else(|| {
            infrastructure_error(format!("cannot append to aggregate {id}: not created"))
        })?;
        let actual_sequence = existing.last().map_or(0, |envelope| envelope.sequence().get());
        if actual_sequence != expected_sequence.get() {
            return Err(StoreError::ConcurrencyConflict {
                aggregate_id: id,
                expected_sequence,
                actual_sequence,
            });
        }
        let envelopes = build_envelopes(id, expected_sequence.get(), events, &context)?;
        persist_envelopes(&mut inner.store, id, &envelopes)?;
        inner.index.entry(id).or_default().extend(envelopes.clone());
        Ok(envelopes)
    }
}

impl<E: DomainEvent> ListableEventStore for PardosaEventStore<E> {
    async fn list_aggregates(&self) -> Result<Vec<AggregateId>, StoreError> {
        self.list_indexed_aggregates()
    }
}

impl<E: DomainEvent> SingleWriterEventStore for PardosaEventStore<E> {}

fn next_aggregate_id<E: DomainEvent>(
    index: &BTreeMap<AggregateId, Vec<EventEnvelope<E>>>,
) -> Result<AggregateId, StoreError> {
    let next = index
        .keys()
        .map(|id| id.get())
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| infrastructure_error(io::Error::other("aggregate ID overflow")))?;
    aggregate_id(next)
}

fn build_envelopes<E: DomainEvent>(
    id: AggregateId,
    start_sequence: u64,
    events: Vec<E>,
    context: &CorrelationContext,
) -> Result<Vec<EventEnvelope<E>>, StoreError> {
    let timestamp = jiff::Timestamp::now();
    events
        .into_iter()
        .enumerate()
        .map(|(offset, payload)| {
            let offset = u64::try_from(offset).map_err(infrastructure_error)?;
            let raw_sequence = start_sequence
                .checked_add(offset)
                .and_then(|sequence| sequence.checked_add(1))
                .ok_or_else(|| infrastructure_error(io::Error::other("sequence overflow")))?;
            let sequence = NonZeroU64::new(raw_sequence).ok_or_else(|| {
                infrastructure_error(io::Error::other("sequence must be non-zero"))
            })?;
            EventEnvelope::new(
                uuid::Uuid::now_v7(),
                id,
                sequence,
                timestamp,
                context.correlation_id(),
                context.causation_id(),
                payload,
            )
            .map_err(infrastructure_error)
        })
        .collect()
}

fn persist_envelopes<E: DomainEvent>(
    store: &mut PardosaStore<EnvelopePayload>,
    id: AggregateId,
    envelopes: &[EventEnvelope<E>],
) -> Result<(), StoreError> {
    for envelope in envelopes {
        let encoded = rmp_serde::to_vec_named(envelope).map_err(infrastructure_error)?;
        let domain_key = id.to_string();
        let payload = EnvelopePayload::new(encoded, id.get(), domain_key).map_err(corrupt_data)?;
        let _ = store.writer().begin(payload).map_err(infrastructure_error)?;
    }
    let _ = store.writer().sync().map_err(infrastructure_error)?;
    Ok(())
}

fn infrastructure_error(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> StoreError {
    StoreError::Infrastructure(source.into())
}

fn corrupt_data(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> StoreError {
    StoreError::CorruptData(source.into())
}
