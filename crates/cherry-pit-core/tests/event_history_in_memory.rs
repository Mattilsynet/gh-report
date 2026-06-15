use std::future::Future;
use std::num::NonZeroU64;
use std::task::{Context, Poll, Waker};

use cherry_pit_core::testing::InMemoryEventStore;
use cherry_pit_core::{
    AggregateId, CorrelationContext, DomainEvent, EventEnvelope, EventHistoryEventStore,
    EventStore, StoreError,
};
use serde::{Deserialize, Serialize};

fn block_on<F: Future>(fut: F) -> F::Output {
    let mut cx = Context::from_waker(Waker::noop());
    let mut fut = std::pin::pin!(fut);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum HistoryEvent {
    Recorded(u8),
}

impl DomainEvent for HistoryEvent {
    fn event_type(&self) -> &'static str {
        "history.recorded"
    }
}

fn seeded_store() -> (
    InMemoryEventStore<HistoryEvent>,
    AggregateId,
    Vec<EventEnvelope<HistoryEvent>>,
    uuid::Uuid,
) {
    let store = InMemoryEventStore::new();
    let correlation_id = uuid::Uuid::now_v7();
    let (id, mut envelopes) = block_on(store.create(
        vec![HistoryEvent::Recorded(1)],
        CorrelationContext::correlated(correlation_id),
    ))
    .unwrap();

    let second = block_on(store.append(
        id,
        NonZeroU64::new(1).unwrap(),
        vec![HistoryEvent::Recorded(2)],
        CorrelationContext::new(correlation_id, envelopes[0].event_id()),
    ))
    .unwrap();
    let second_id = second[0].event_id();
    envelopes.extend(second);

    let third = block_on(store.append(
        id,
        NonZeroU64::new(2).unwrap(),
        vec![HistoryEvent::Recorded(3)],
        CorrelationContext::new(correlation_id, second_id),
    ))
    .unwrap();
    envelopes.extend(third);

    (store, id, envelopes, correlation_id)
}

fn event_ids(envelopes: &[EventEnvelope<HistoryEvent>]) -> Vec<uuid::Uuid> {
    envelopes.iter().map(EventEnvelope::event_id).collect()
}

#[test]
fn event_history_methods_return_stored_envelope_futures() {
    fn assert_stored_envelope_future<F, E>(_future: F)
    where
        F: Future<Output = Result<Vec<EventEnvelope<E>>, StoreError>> + Send,
        E: DomainEvent,
    {
    }

    let (store, id, envelopes, _) = seeded_store();
    assert_stored_envelope_future(store.history(id));
    assert_stored_envelope_future(store.replay_until(id, NonZeroU64::new(2).unwrap()));
    assert_stored_envelope_future(store.causal_chain(id, envelopes[2].event_id()));
}

#[test]
fn history_and_causal_chain_use_stored_metadata() {
    let (store, id, envelopes, correlation_id) = seeded_store();

    let history = block_on(store.history(id)).unwrap();
    assert_eq!(event_ids(&history), event_ids(&envelopes));
    assert_eq!(
        history
            .iter()
            .map(EventEnvelope::sequence)
            .map(NonZeroU64::get)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );

    let chain = block_on(store.causal_chain(id, envelopes[2].event_id())).unwrap();
    assert_eq!(event_ids(&chain), event_ids(&envelopes));
    assert_eq!(
        chain
            .iter()
            .map(EventEnvelope::correlation_id)
            .collect::<Vec<_>>(),
        vec![
            Some(correlation_id),
            Some(correlation_id),
            Some(correlation_id)
        ]
    );
    assert_eq!(
        chain
            .iter()
            .map(EventEnvelope::causation_id)
            .collect::<Vec<_>>(),
        vec![
            None,
            Some(envelopes[0].event_id()),
            Some(envelopes[1].event_id()),
        ]
    );
}

#[test]
fn empty_history_for_unknown_aggregate() {
    let store = InMemoryEventStore::<HistoryEvent>::new();
    let unknown = AggregateId::new(NonZeroU64::new(999).unwrap());

    let history = block_on(store.history(unknown)).unwrap();

    assert!(history.is_empty());
}

#[test]
fn replay_until_returns_load_order_prefix() {
    let (store, id, envelopes, _) = seeded_store();

    let prefix = block_on(store.replay_until(id, NonZeroU64::new(2).unwrap())).unwrap();

    assert_eq!(event_ids(&prefix), event_ids(&envelopes[..2]));
}
