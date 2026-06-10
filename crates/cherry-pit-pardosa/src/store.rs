use crate::payload::EnvelopePayload;
use cherry_pit_core::{AggregateId, DomainEvent, EventEnvelope, StoreError};
use pardosa::store::{Event, EventStore as PardosaStore, PgnoBackend};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::path::Path;

pub struct PardosaEventStore<E: DomainEvent> {
    _store: PardosaStore<EnvelopePayload>,
    index: BTreeMap<AggregateId, Vec<CapturedEnvelope>>,
    _event: PhantomData<fn() -> E>,
}

#[derive(Clone)]
struct CapturedEnvelope {
    envelope_bytes: Vec<u8>,
}

impl<E: DomainEvent> PardosaEventStore<E> {
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

    fn from_pardosa_store(store: PardosaStore<EnvelopePayload>) -> Result<Self, StoreError> {
        let index = capture_index::<E>(&store)?;
        Ok(Self {
            _store: store,
            index,
            _event: PhantomData,
        })
    }

    /// Return aggregate ids captured during adapter open.
    ///
    /// # Errors
    ///
    /// This method returns `Ok` for a successfully-opened adapter. The
    /// `Result` shape mirrors the async `ListableEventStore` contract.
    pub fn list_indexed_aggregates(&self) -> Result<Vec<AggregateId>, StoreError> {
        Ok(self.index.keys().copied().collect())
    }

    /// Load an aggregate stream from the captured open-time index.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::CorruptData`] when captured envelope bytes
    /// fail to decode or when the gathered stream is not gap-free for
    /// `id`.
    pub fn load_indexed(&self, id: AggregateId) -> Result<Vec<EventEnvelope<E>>, StoreError> {
        decode_stream(id, self.index.get(&id).into_iter().flatten())
    }
}

fn capture_index<E: DomainEvent>(
    store: &PardosaStore<EnvelopePayload>,
) -> Result<BTreeMap<AggregateId, Vec<CapturedEnvelope>>, StoreError> {
    let captured = RefCell::new(Vec::<(u64, Vec<u8>)>::new());
    let extractor = |event: &Event<EnvelopePayload>| -> std::iter::Once<u64> {
        let payload = event.domain_event();
        captured
            .borrow_mut()
            .push((payload.aggregate_id, payload.envelope_bytes().to_vec()));
        std::iter::once(payload.aggregate_id)
    };
    let _ = store.reader().fiber_index::<u64, _, _>(extractor);

    let mut index = BTreeMap::<AggregateId, Vec<CapturedEnvelope>>::new();
    let mut aggregate_ids = BTreeSet::<AggregateId>::new();
    for (raw_id, envelope_bytes) in captured.into_inner() {
        let aggregate_id = aggregate_id(raw_id)?;
        aggregate_ids.insert(aggregate_id);
        index
            .entry(aggregate_id)
            .or_default()
            .push(CapturedEnvelope { envelope_bytes });
    }
    for id in aggregate_ids {
        let _ = decode_stream::<E, _>(id, index.get(&id).into_iter().flatten())?;
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

fn infrastructure_error(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> StoreError {
    StoreError::Infrastructure(source.into())
}

fn corrupt_data(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> StoreError {
    StoreError::CorruptData(source.into())
}
