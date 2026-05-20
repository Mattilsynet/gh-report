//! Integration tests for [`pardosa_eventstore::PardosaLogEventStore`].
//!
//! Covers the cross-cutting properties not exercised by the inline
//! unit tests: restart-survival, concurrent writers on disjoint
//! aggregates, and torn-tail recovery on next `open()`.

use std::num::NonZeroU64;
use std::sync::Arc;

use cherry_pit_core::{
    AggregateId, CorrelationContext, DomainEvent, EventStore, ListableEventStore,
};
use pardosa_encoding::{Decode, Decoder, Encode, EventError};
use pardosa_eventstore::PardosaLogEventStore;

#[derive(Debug, Clone, PartialEq)]
enum Ev {
    Tick(u32),
}

impl DomainEvent for Ev {
    fn event_type(&self) -> &'static str {
        "test.tick"
    }
}

impl Encode for Ev {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Ev::Tick(n) => {
                out.push(0u8);
                n.encode(out);
            }
        }
    }
}

impl Decode for Ev {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let tag = <u8 as Decode>::decode(d)?;
        match tag {
            0 => Ok(Ev::Tick(<u32 as Decode>::decode(d)?)),
            _ => Err(EventError::InvalidInput),
        }
    }
}

/// Persist three aggregates across two store lifetimes; verify the
/// second `open()` recovers every envelope and seeds `next_id` past
/// the highest persisted id.
#[tokio::test]
async fn restart_survives_create_and_append() {
    let dir = tempfile::tempdir().unwrap();

    // ─── First lifetime ───
    let ids_and_seqs = {
        let store = PardosaLogEventStore::<Ev>::open(dir.path()).await.unwrap();

        let (id_a, env_a) = store
            .create(vec![Ev::Tick(1), Ev::Tick(2)], CorrelationContext::none())
            .await
            .unwrap();
        let (id_b, env_b) = store
            .create(vec![Ev::Tick(10)], CorrelationContext::none())
            .await
            .unwrap();
        let last_a = env_a.last().unwrap().sequence();
        let _ = store
            .append(id_a, last_a, vec![Ev::Tick(3)], CorrelationContext::none())
            .await
            .unwrap();
        let (id_c, env_c) = store
            .create(
                vec![Ev::Tick(100), Ev::Tick(200), Ev::Tick(300)],
                CorrelationContext::none(),
            )
            .await
            .unwrap();
        (id_a, id_b, id_c, env_a.len() + 1, env_b.len(), env_c.len())
    };
    let (id_a, id_b, id_c, len_a, len_b, len_c) = ids_and_seqs;

    // ─── Second lifetime ───
    let store2 = PardosaLogEventStore::<Ev>::open(dir.path()).await.unwrap();
    let loaded_a = store2.load(id_a).await.unwrap();
    let loaded_b = store2.load(id_b).await.unwrap();
    let loaded_c = store2.load(id_c).await.unwrap();
    assert_eq!(loaded_a.len(), len_a, "aggregate A: 2 created + 1 appended");
    assert_eq!(loaded_b.len(), len_b);
    assert_eq!(loaded_c.len(), len_c);

    // Sequence integrity on the appended-to stream.
    for (i, env) in loaded_a.iter().enumerate() {
        assert_eq!(env.sequence().get(), (i + 1) as u64);
    }

    // `next_id` must be `max(seen) + 1`. Create one more and confirm.
    let max_seen = id_a.get().max(id_b.get()).max(id_c.get());
    let (id_next, _) = store2
        .create(vec![Ev::Tick(0)], CorrelationContext::none())
        .await
        .unwrap();
    assert_eq!(id_next.get(), max_seen + 1);
}

/// Two creators racing against the same store must each receive a
/// distinct `AggregateId` and persist their own log file.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_create_assigns_distinct_ids() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(PardosaLogEventStore::<Ev>::open(dir.path()).await.unwrap());

    let mut handles = Vec::new();
    for i in 0..16u32 {
        let s = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            s.create(vec![Ev::Tick(i)], CorrelationContext::none())
                .await
                .unwrap()
                .0
        }));
    }
    let mut ids: Vec<AggregateId> = Vec::new();
    for h in handles {
        ids.push(h.await.unwrap());
    }
    ids.sort_by_key(|a| a.get());
    ids.dedup_by_key(|a| a.get());
    assert_eq!(ids.len(), 16, "all ids must be distinct");

    // list_aggregates surfaces all 16.
    let mut listed = store.list_aggregates().unwrap();
    listed.sort_by_key(|a| a.get());
    assert_eq!(listed, ids);
}

/// Two appenders on the *same* aggregate must serialise: one succeeds,
/// the other observes the resulting sequence advance and is rejected
/// with `ConcurrencyConflict` (unless it happens to load-then-append
/// in the gap, which the per-slot mutex prevents).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_append_on_same_aggregate_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(PardosaLogEventStore::<Ev>::open(dir.path()).await.unwrap());
    let (id, env) = store
        .create(vec![Ev::Tick(0)], CorrelationContext::none())
        .await
        .unwrap();
    let last_seq = env.last().unwrap().sequence();

    // Both appenders supply the *same* expected_sequence — the one
    // that wins the mutex first persists; the second observes
    // next_seq advanced and is rejected.
    let s1 = Arc::clone(&store);
    let s2 = Arc::clone(&store);
    let h1 = tokio::spawn(async move {
        s1.append(id, last_seq, vec![Ev::Tick(1)], CorrelationContext::none())
            .await
    });
    let h2 = tokio::spawn(async move {
        s2.append(id, last_seq, vec![Ev::Tick(2)], CorrelationContext::none())
            .await
    });
    let r1 = h1.await.unwrap();
    let r2 = h2.await.unwrap();

    // Exactly one Ok, exactly one ConcurrencyConflict.
    let oks = [&r1, &r2].iter().filter(|r| r.is_ok()).count();
    let conflicts = [&r1, &r2]
        .iter()
        .filter(|r| {
            matches!(
                r,
                Err(cherry_pit_core::StoreError::ConcurrencyConflict { .. })
            )
        })
        .count();
    assert_eq!(oks, 1, "exactly one append must succeed");
    assert_eq!(conflicts, 1, "the other must conflict");

    // Final stream length: 1 (create) + 1 (winner) = 2.
    let final_stream = store.load(id).await.unwrap();
    assert_eq!(final_stream.len(), 2);
}

/// Recover from a torn tail: write a complete log, append junk bytes
/// (simulating a partial mid-frame crash), re-open, confirm the
/// well-formed prefix is intact and `append` advances from there.
#[tokio::test]
async fn torn_tail_is_truncated_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let id;
    {
        let store = PardosaLogEventStore::<Ev>::open(dir.path()).await.unwrap();
        let (created_id, _) = store
            .create(vec![Ev::Tick(1), Ev::Tick(2)], CorrelationContext::none())
            .await
            .unwrap();
        id = created_id;
    } // drop releases lock

    // Corrupt the tail.
    let log_path = dir.path().join(format!("{}.log", id.get()));
    let mut bytes = std::fs::read(&log_path).unwrap();
    let pre_corrupt_len = bytes.len();
    bytes.extend_from_slice(b"\xff\xff\xff\xff\x00\x00\x00\x00garbage");
    std::fs::write(&log_path, &bytes).unwrap();

    // Re-open: the recovery path must truncate back to pre_corrupt_len
    // and surface the original two envelopes.
    let store = PardosaLogEventStore::<Ev>::open(dir.path()).await.unwrap();
    let loaded = store.load(id).await.unwrap();
    assert_eq!(loaded.len(), 2);
    let post_open_len = std::fs::metadata(&log_path).unwrap().len();
    assert_eq!(usize::try_from(post_open_len).unwrap(), pre_corrupt_len);

    // Appending after recovery still works (file handle is fresh).
    let _ = store
        .append(
            id,
            NonZeroU64::new(2).unwrap(),
            vec![Ev::Tick(3)],
            CorrelationContext::none(),
        )
        .await
        .expect("append after torn-tail recovery");
    let final_stream = store.load(id).await.unwrap();
    assert_eq!(final_stream.len(), 3);
}
