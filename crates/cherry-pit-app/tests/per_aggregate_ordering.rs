//! Per-aggregate event ordering proptest for F2 (mission
//! adr-fmt-cq7vb.2).
//!
//! Property: under the F2 / Approach A2 design — synchronous bus
//! fan-out (CHE-0024:§7) feeding a single bounded `tokio::sync::mpsc`
//! channel that drains into a single sequential consumer task —
//! envelopes arrive at the consumer in **publish order**, and
//! therefore per-aggregate event order is preserved through dispatch.
//!
//! Sequential dispatch makes the per-aggregate ordering claim
//! trivially true: if envelope `(agg=a, seq=n)` is published before
//! `(agg=a, seq=n+1)`, then `n` enters the channel first (synchronous
//! bus fan-out) and the consumer pulls `n` before `n+1` (`mpsc::Receiver`
//! is strictly FIFO). The proptest's contribution is to **observe**
//! the property under a random publish schedule across multiple
//! aggregates — confirming that the orphan-`handle.spawn` design
//! (pre-F2) which violated this property is genuinely gone.
//!
//! Scope deliberately narrow:
//!
//! - We exercise the bus → `enqueue_or_log` → channel → consumer
//!   pipeline directly. The `run_dispatch_consumer` policy-execution
//!   path is unit-tested in `app.rs::tests`; here we focus on the
//!   ordering property that the pipeline preserves.
//! - Generous channel capacity (`8 * N`) so back-pressure does NOT
//!   drop envelopes — the back-pressure surface is a separate property
//!   tested in `app.rs::tests::full_dispatch_channel_drops_overflow_…`.

use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_app::{InProcessEventBus, enqueue_or_log};
use cherry_pit_core::{AggregateId, DomainEvent, EventBus, EventEnvelope};
use proptest::prelude::*;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum E {
    Tick { aggregate: u64, sequence: u64 },
}

impl DomainEvent for E {
    fn event_type(&self) -> &'static str {
        "test.tick"
    }
}

fn envelope(aggregate: u64, sequence: u64) -> EventEnvelope<E> {
    EventEnvelope::new(
        uuid::Uuid::now_v7(),
        AggregateId::new(NonZeroU64::new(aggregate).unwrap()),
        NonZeroU64::new(sequence).unwrap(),
        jiff::Timestamp::now(),
        None,
        None,
        E::Tick {
            aggregate,
            sequence,
        },
    )
    .unwrap()
}

/// Generate a publish schedule: M aggregates, each with K envelopes
/// at sequences 1..=K, then shuffled into a random global publish
/// order. Returns the shuffled publish order; the per-aggregate
/// expected dispatch order is the filtered projection of that vector
/// for each aggregate.
fn schedule_strategy() -> impl Strategy<Value = Vec<(u64, u64)>> {
    (1usize..=4, 1u64..=8, any::<u64>()).prop_map(|(num_aggregates, k_per, seed)| {
        let mut all: Vec<(u64, u64)> = Vec::new();
        for agg_idx in 1..=(num_aggregates as u64) {
            for s in 1..=k_per {
                all.push((agg_idx, s));
            }
        }
        let mut rng_state = seed | 1;
        for i in (1..all.len()).rev() {
            rng_state = rng_state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let modulus = u64::try_from(i + 1).unwrap_or(u64::MAX);
            let j = usize::try_from(rng_state % modulus).unwrap_or(0);
            all.swap(i, j);
        }
        all
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        ..ProptestConfig::default()
    })]

    /// The consumer pulls envelopes from the channel in publish order.
    /// Per-aggregate ordering follows trivially: filtering a sequence
    /// that preserves global order also preserves per-aggregate order.
    #[test]
    fn per_aggregate_dispatch_order_matches_publish_order(shuffled in schedule_strategy()) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async move {
            let bus = Arc::new(InProcessEventBus::<E>::new());
            let capacity = (shuffled.len() * 8).max(16);
            let (tx, mut rx) = mpsc::channel::<EventEnvelope<E>>(capacity);

            bus.register(move |envelope: &EventEnvelope<E>| {
                enqueue_or_log(&tx, envelope);
            });

            let observed: Arc<Mutex<Vec<(u64, u64)>>> = Arc::new(Mutex::new(Vec::new()));
            let observed_for_consumer = Arc::clone(&observed);
            let consumer = tokio::spawn(async move {
                while let Some(envelope) = rx.recv().await {
                    let E::Tick { aggregate, sequence } = envelope.payload();
                    observed_for_consumer
                        .lock()
                        .unwrap()
                        .push((*aggregate, *sequence));
                }
            });

            for (aggregate, sequence) in &shuffled {
                bus.publish(&[envelope(*aggregate, *sequence)])
                    .await
                    .unwrap();
            }

            drop(bus);
            tokio::time::timeout(std::time::Duration::from_secs(5), consumer)
                .await
                .expect("consumer must terminate after bus drop")
                .expect("consumer task must not panic");

            let observed_list = observed.lock().unwrap().clone();

            prop_assert_eq!(
                observed_list.len(),
                shuffled.len(),
                "all published envelopes must reach the consumer (no drops at this capacity)",
            );

            prop_assert_eq!(
                observed_list.clone(),
                shuffled.clone(),
                "global dispatch order must equal global publish order under sequential consumer",
            );

            let aggregates_seen: std::collections::BTreeSet<u64> =
                shuffled.iter().map(|(a, _)| *a).collect();
            for aggregate in aggregates_seen {
                let expected: Vec<(u64, u64)> = shuffled
                    .iter()
                    .filter(|(a, _)| *a == aggregate)
                    .copied()
                    .collect();
                let observed_for_agg: Vec<(u64, u64)> = observed_list
                    .iter()
                    .filter(|(a, _)| *a == aggregate)
                    .copied()
                    .collect();
                prop_assert_eq!(
                    observed_for_agg,
                    expected,
                    "per-aggregate dispatch order for aggregate {} must match its publish order",
                    aggregate,
                );
            }

            Ok(())
        })?;
    }
}
