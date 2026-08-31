//! Ephemeral, in-process `EventStore` for the scheduler + sweep-timeout
//! streams.
//!
//! Per CHE-0099, these two streams are audit-only bookkeeping around
//! per-run sweep timeouts (CHE-0081:R11 opt-out from CHE-0072's durable
//! backend selector): they carry no restart-critical state, so gh-report
//! does not persist them to disk. This store mirrors the shape of
//! `cherry_pit_core::testing::InMemoryEventStore` but lives in gh-report
//! prod code rather than pulling `cherry-pit-core`'s `testing` feature
//! into the shipped binary.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::Mutex;

use cherry_pit_core::{
    AggregateId, CorrelationContext, DomainEvent, EventEnvelope, EventStore, StoreCreateResult,
    StoreError,
};

/// In-process, non-persistent `EventStore`. Always starts empty; state
/// does not survive process restart by design (CHE-0099).
pub struct EphemeralEventStore<E: DomainEvent> {
    state: Mutex<EphemeralState<E>>,
}

struct EphemeralState<E: DomainEvent> {
    streams: HashMap<AggregateId, Vec<EventEnvelope<E>>>,
    next_id: NonZeroU64,
}

impl<E: DomainEvent> EphemeralEventStore<E> {
    /// Create an empty ephemeral store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(EphemeralState {
                streams: HashMap::new(),
                next_id: NonZeroU64::MIN,
            }),
        }
    }
}

impl<E: DomainEvent> Default for EphemeralEventStore<E> {
    fn default() -> Self {
        Self::new()
    }
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

impl<E: DomainEvent> EventStore for EphemeralEventStore<E> {
    type Event = E;

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "the ephemeral store operates on a Mutex-guarded in-memory map with no I/O to await; the `async` keyword is dictated by the EventStore trait signature"
    )]
    async fn load(&self, id: AggregateId) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        let state = self
            .state
            .lock()
            .expect("EphemeralEventStore mutex poisoned");
        let stream = state.streams.get(&id).cloned().unwrap_or_default();
        EventEnvelope::validate_stream(id, &stream)
            .map_err(|e| StoreError::CorruptData(Box::new(e)))?;
        Ok(stream)
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "the ephemeral store operates on a Mutex-guarded in-memory map with no I/O to await; the `async` keyword is dictated by the EventStore trait signature"
    )]
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
        let mut state = self
            .state
            .lock()
            .expect("EphemeralEventStore mutex poisoned");
        let id = AggregateId::new(state.next_id);
        let bumped = state.next_id.get().checked_add(1).ok_or_else(|| {
            StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(
                "aggregate ID overflow",
            ))
        })?;
        state.next_id = NonZeroU64::new(bumped).expect("bumped non-zero u64 cannot wrap to zero");

        let envelopes = build_envelopes(id, 0, events, &context)?;
        state.streams.insert(id, envelopes.clone());
        Ok((id, envelopes))
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "the ephemeral store operates on a Mutex-guarded in-memory map with no I/O to await; the `async` keyword is dictated by the EventStore trait signature"
    )]
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
        let mut state = self
            .state
            .lock()
            .expect("EphemeralEventStore mutex poisoned");

        let Some(stream) = state.streams.get(&id) else {
            return Err(StoreError::Infrastructure(Box::<
                dyn std::error::Error + Send + Sync,
            >::from(format!(
                "cannot append to aggregate {id}: not created (use create() first)"
            ))));
        };

        let actual_sequence = stream.last().map_or(0, |e| e.sequence().get());
        if actual_sequence != expected_sequence.get() {
            return Err(StoreError::ConcurrencyConflict {
                aggregate_id: id,
                expected_sequence,
                actual_sequence,
            });
        }

        let new_envelopes = build_envelopes(id, expected_sequence.get(), events, &context)?;
        let stream_mut = state
            .streams
            .get_mut(&id)
            .expect("stream existence checked above under same lock");
        stream_mut.extend(new_envelopes.iter().cloned());
        Ok(new_envelopes)
    }
}
