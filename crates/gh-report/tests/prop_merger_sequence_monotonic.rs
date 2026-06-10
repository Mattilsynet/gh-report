//! Proptest pinning that the lifted [`cherry_pit_merger::Merger`]
//! assigns strictly +1-monotonic sequences across sequential
//! [`MergerHandle::dispatch`](cherry_pit_merger::MergerHandle::dispatch)
//! calls for the [`Repo`] aggregate.
//!
//! The test exercises a small set of routing keys (`alpha`/`beta`/`gamma`)
//! and asserts that per-aggregate streams have contiguous sequences
//! starting at 1 plus a count matching the per-key dispatch count.
//! Post-Mission-H (CHE-0069) the merger is consumed via
//! [`super::arms::RepoArm`] through
//! [`crate::app::services::merger::MergerHandles`].
//!
//! [`Repo`]: gh_report::domain::aggregates::repo::Repo
//! [`super::arms::RepoArm`]: gh_report::app::services::RepoArm

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_core::{
    AggregateId, BusError, CorrelationContext, EventBus, EventEnvelope, EventStore,
};
use gh_report::app::state::EventStoreImpl;
use proptest::collection::vec;
use proptest::prelude::*;
use tempfile::TempDir;

use gh_report::app::services::MergerHandles;
use gh_report::app::services::repo_service::RepoService;
use gh_report::domain::aggregates::repo::RecordEvaluation;
use gh_report::domain::events::DomainEvent;

const TS: &str = "2026-05-10T12:00:00Z";

#[derive(Default)]
struct NoopBus;

impl EventBus for NoopBus {
    type Event = DomainEvent;
    async fn publish(&self, _events: &[EventEnvelope<Self::Event>]) -> Result<(), BusError> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct Eval {
    name: &'static str,
}

fn name_strategy() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec!["alpha", "beta", "gamma"])
}

fn eval_strategy() -> impl Strategy<Value = Eval> {
    name_strategy().prop_map(|name| Eval { name })
}

fn build_record_eval(name: &'static str) -> RecordEvaluation {
    RecordEvaluation {
        domain_key: format!("id-{name}"),
        repo_name: name.into(),
        success: true,
        source: "scheduled_batch".into(),
        duration_ms: 0,
        timestamp: TS.into(),
        evidence: None,
    }
}

async fn drive_and_verify(evals: Vec<Eval>) -> Result<(), TestCaseError> {
    let dir = TempDir::new().expect("tempdir");
    let store = Arc::new(EventStoreImpl::create_pgno(&dir.path().join("events.pgno")).unwrap());
    let bus: Arc<NoopBus> = Arc::new(NoopBus);
    let repos_by_key: Arc<Mutex<HashMap<String, AggregateId>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let next_seq: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let (handles, _joins) = MergerHandles::<NoopBus>::with_bus_for_test(
        Arc::clone(&store),
        Arc::clone(&bus),
        Arc::clone(&repos_by_key),
        Arc::clone(&next_seq),
    );
    let svc = RepoService::with_handle(handles.repo);
    let ctx = CorrelationContext::none();

    let mut per_key_count: HashMap<&'static str, u64> = HashMap::new();
    for ev in &evals {
        let cmd = build_record_eval(ev.name);
        let dk = cmd.domain_key.clone();
        svc.record_evaluation(&dk, cmd, &ctx)
            .await
            .expect("RecordEvaluation succeeds");
        *per_key_count.entry(ev.name).or_insert(0) += 1;
    }

    let snapshot: Vec<(String, AggregateId)> = {
        let guard = repos_by_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.iter().map(|(k, v)| (k.clone(), *v)).collect()
    };

    let mut seen_ids: std::collections::BTreeSet<AggregateId> =
        std::collections::BTreeSet::default();
    for (key, agg_id) in &snapshot {
        prop_assert!(
            seen_ids.insert(*agg_id),
            "distinct routing keys must map to distinct AggregateId; collision on {}",
            key
        );

        let envelopes = store.load(*agg_id).await.expect("load aggregate stream");
        prop_assert!(
            !envelopes.is_empty(),
            "aggregate {:?} for key {} has empty stream after RecordEvaluation",
            agg_id,
            key
        );

        let seqs: Vec<u64> = envelopes.iter().map(|e| e.sequence().get()).collect();
        prop_assert_eq!(
            seqs[0],
            1,
            "first envelope sequence must be 1; got {:?}",
            seqs
        );
        for w in seqs.windows(2) {
            prop_assert_eq!(
                w[1],
                w[0] + 1,
                "sequences must be strictly +1 monotonic; got {:?}",
                seqs
            );
        }

        let key_name = key.strip_prefix("id-").unwrap_or(key.as_str());
        let expected_len = *per_key_count.get(key_name).unwrap_or(&0);
        prop_assert_eq!(
            envelopes.len() as u64,
            expected_len,
            "envelope count for key {} should equal command count {}",
            key,
            expected_len
        );
    }

    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    #[test]
    fn merger_assigns_strictly_monotonic_sequences_for_sequential_commands(
        evals in vec(eval_strategy(), 1..8),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(drive_and_verify(evals))?;
    }
}
