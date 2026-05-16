//! Smoke tests for `PardosaEventStore` — exercises the `EventStore` contract
//! and the three CHE-0057 extension trait impls. These are minimum-
//! viable assertions; SM-6 will surface deeper conformance gaps via the
//! shared harness from SM-3/SM-4.

use std::num::NonZeroU64;

use cherry_pit_core::{
    AggregateId, CorrelationContext, DomainEvent, EventStore, HashChainedEventStore,
    PurgeableEventStore, StoreError,
};
use cherry_pit_pardosa::PardosaEventStore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TestEvent {
    payload: String,
}

impl DomainEvent for TestEvent {
    fn event_type(&self) -> &'static str {
        "test.event"
    }
}

fn ctx() -> CorrelationContext {
    CorrelationContext::none()
}

#[tokio::test]
async fn create_assigns_aggregate_id_one_and_returns_envelopes() {
    let store: PardosaEventStore<TestEvent> = PardosaEventStore::new();
    let (id, envs) = store
        .create(
            vec![TestEvent {
                payload: "e1".into(),
            }],
            ctx(),
        )
        .await
        .expect("create succeeds");
    assert_eq!(id.get(), 1, "first AggregateId starts at 1 (CHE-0020:R2)");
    assert_eq!(envs.len(), 1);
    assert_eq!(envs[0].sequence().get(), 1);
    assert_eq!(envs[0].payload().payload, "e1");
}

#[tokio::test]
async fn load_unknown_aggregate_returns_empty_vec() {
    let store: PardosaEventStore<TestEvent> = PardosaEventStore::new();
    let id = AggregateId::new(NonZeroU64::new(999).unwrap());
    let result = store
        .load(id)
        .await
        .expect("load is infallible for unknown");
    assert!(result.is_empty(), "CHE-0019:R1 — empty vec for unknown");
}

#[tokio::test]
async fn create_then_load_round_trips_envelopes() {
    let store: PardosaEventStore<TestEvent> = PardosaEventStore::new();
    let initial = vec![
        TestEvent {
            payload: "a".into(),
        },
        TestEvent {
            payload: "b".into(),
        },
        TestEvent {
            payload: "c".into(),
        },
    ];
    let (id, created) = store.create(initial.clone(), ctx()).await.unwrap();
    let loaded = store.load(id).await.unwrap();
    assert_eq!(loaded.len(), 3);
    for (i, env) in loaded.iter().enumerate() {
        assert_eq!(
            env.sequence().get(),
            u64::try_from(i).unwrap() + 1,
            "sequences 1..N contiguous"
        );
        assert_eq!(env.payload(), &initial[i]);
    }
    // Envelope identity preserved across substrate boundary
    // (CHE-0042:R1 — event_ids assigned at store, returned to caller,
    // observed unchanged on load).
    for (a, b) in created.iter().zip(loaded.iter()) {
        assert_eq!(a.event_id(), b.event_id());
        assert_eq!(a.sequence(), b.sequence());
    }
}

#[tokio::test]
async fn append_extends_stream_with_correct_sequences() {
    let store: PardosaEventStore<TestEvent> = PardosaEventStore::new();
    let (id, initial) = store
        .create(
            vec![TestEvent {
                payload: "1".into(),
            }],
            ctx(),
        )
        .await
        .unwrap();
    let last = initial.last().unwrap().sequence();
    let appended = store
        .append(
            id,
            last,
            vec![
                TestEvent {
                    payload: "2".into(),
                },
                TestEvent {
                    payload: "3".into(),
                },
            ],
            ctx(),
        )
        .await
        .unwrap();
    assert_eq!(appended.len(), 2);
    assert_eq!(appended[0].sequence().get(), 2);
    assert_eq!(appended[1].sequence().get(), 3);

    let loaded = store.load(id).await.unwrap();
    assert_eq!(loaded.len(), 3);
}

#[tokio::test]
async fn append_rejects_stale_expected_sequence_with_conflict() {
    let store: PardosaEventStore<TestEvent> = PardosaEventStore::new();
    let (id, _) = store
        .create(
            vec![TestEvent {
                payload: "1".into(),
            }],
            ctx(),
        )
        .await
        .unwrap();
    // Caller incorrectly thinks last sequence is still 1, but has
    // staged an append after observing a stale view.
    let stale = NonZeroU64::new(99).unwrap();
    let err = store
        .append(
            id,
            stale,
            vec![TestEvent {
                payload: "x".into(),
            }],
            ctx(),
        )
        .await
        .unwrap_err();
    match err {
        StoreError::ConcurrencyConflict {
            aggregate_id,
            expected_sequence,
            actual_sequence,
        } => {
            assert_eq!(aggregate_id, id);
            assert_eq!(expected_sequence, stale);
            assert_eq!(actual_sequence, 1);
        }
        other => panic!("expected ConcurrencyConflict, got {other:?}"),
    }
}

#[tokio::test]
async fn create_rejects_empty_events() {
    let store: PardosaEventStore<TestEvent> = PardosaEventStore::new();
    let err = store.create(vec![], ctx()).await.unwrap_err();
    assert!(matches!(err, StoreError::Infrastructure(_)));
}

// ── PurgeableEventStore ───────────────────────────────────────────

#[tokio::test]
async fn load_history_for_unknown_aggregate_is_empty() {
    let store: PardosaEventStore<TestEvent> = PardosaEventStore::new();
    let id = AggregateId::new(NonZeroU64::new(42).unwrap());
    let history = store.load_history(id).await.unwrap();
    assert!(history.is_empty(), "CHE-0019:R1 + CHE-0059:R5");
}

#[tokio::test]
async fn recreate_severs_continuity_and_restarts_sequence() {
    let store: PardosaEventStore<TestEvent> = PardosaEventStore::new();
    let (id, _) = store
        .create(
            vec![
                TestEvent {
                    payload: "v1-a".into(),
                },
                TestEvent {
                    payload: "v1-b".into(),
                },
            ],
            ctx(),
        )
        .await
        .unwrap();

    // After recreate, the fresh incarnation's stream starts at seq 1.
    let recreated = store
        .recreate(
            id,
            TestEvent {
                payload: "tombstone".into(),
            },
            vec![TestEvent {
                payload: "v2-a".into(),
            }],
            ctx(),
        )
        .await
        .unwrap();
    assert_eq!(recreated.len(), 1);
    assert_eq!(
        recreated[0].sequence().get(),
        1,
        "recreate severs continuity per CHE-0059:R4 / PAR-0001",
    );

    // `load` returns the current incarnation only.
    let loaded = store.load(id).await.unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].payload().payload, "v2-a");

    // `load_history` returns both incarnations concatenated, plus
    // the tombstone event recorded against the prior incarnation
    // during recreate (substrate audit trail per CHE-0059:R2/R4 +
    // PAR-0001 Defined→Detached transition).
    let history = store.load_history(id).await.unwrap();
    assert_eq!(
        history.len(),
        4,
        "pre-purge 2 + tombstone 1 + post-recreate 1 = 4"
    );
    assert_eq!(history[2].payload().payload, "tombstone");
}

// ── HashChainedEventStore (rollout stub per CHE-0060:R3) ──────────

#[tokio::test]
async fn frontier_hash_returns_sentinel_for_rollout_stub() {
    let store: PardosaEventStore<TestEvent> = PardosaEventStore::new();
    let hash = store.frontier_hash();
    assert_eq!(
        hash, [0u8; 32],
        "rollout-stub sentinel hash per CHE-0060:R3",
    );
}

#[tokio::test]
async fn verify_chain_rollout_stub_always_fails() {
    let store: PardosaEventStore<TestEvent> = PardosaEventStore::new();
    let err = store.verify_chain().await.unwrap_err();
    match err {
        StoreError::Infrastructure(inner) => {
            let msg = inner.to_string();
            assert!(
                msg.contains("CHE-0060:R3"),
                "stub error must cite the carve-out ADR rule: {msg}"
            );
        }
        other => panic!("expected Infrastructure error, got {other:?}"),
    }
}

// ── SingleWriterEventStore (marker — type-system signal) ──────────

#[tokio::test]
async fn single_writer_marker_compiles() {
    // Compile-time evidence: PardosaEventStore implements the marker
    // trait. The type-check here is the test.
    fn assert_marker<S: cherry_pit_core::SingleWriterEventStore>(_: &S) {}
    let store: PardosaEventStore<TestEvent> = PardosaEventStore::new();
    assert_marker(&store);
}
