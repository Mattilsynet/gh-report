use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use cherry_pit_core::{CorrelationContext, DomainKey, JobOutcome, JobSource};
use cherry_pit_wq::{
    BudgetGate, EnqueueResult, JobExecutor, JobSpec, RateLimitState, WorkQueue, WorkerPoolConfig,
    run_worker_pool,
};
use proptest::prelude::*;
use tokio::sync::mpsc;

fn current_thread_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current_thread runtime")
}

fn multi_thread_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("multi_thread runtime")
}

fn key_strategy() -> impl Strategy<Value = String> {
    "[a-z]{1,4}"
}

fn source_strategy() -> impl Strategy<Value = JobSource> {
    prop_oneof![
        Just(JobSource::ScheduledBatch),
        Just(JobSource::InitialLoad),
        ("[a-z]{1,4}", "[a-z]{1,4}").prop_map(|(id, kind)| JobSource::External { id, kind }),
    ]
}

fn job(key: &str, source: JobSource) -> JobSpec<String> {
    JobSpec::new(
        key.to_string(),
        format!("ctx-{key}"),
        source,
        CorrelationContext::none(),
    )
}

fn distinct_preserving<T: Eq + std::hash::Hash + Clone>(v: Vec<T>) -> Vec<T> {
    let mut seen = HashSet::new();
    v.into_iter().filter(|x| seen.insert(x.clone())).collect()
}

struct ParityExecutor;

impl JobExecutor for ParityExecutor {
    type Context = String;
    type Result = String;

    async fn execute<'a>(
        &'a self,
        key: &'a DomainKey,
        _ctx: &'a Self::Context,
    ) -> Result<String, String> {
        if key.len().is_multiple_of(2) {
            Ok(key.clone())
        } else {
            Err(format!("fail-{key}"))
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, .. ProptestConfig::default() })]

    #[test]
    fn p1_dedup_count_matches_distinct(
        keys in prop::collection::vec(key_strategy(), 0..32)
    ) {
        let capacity = (keys.len() + 1).max(1);
        let q: WorkQueue<String> = WorkQueue::new(capacity);
        let mut accepted = 0usize;
        let mut dedup = 0usize;
        let mut full = 0usize;
        for k in &keys {
            match q.enqueue(job(k, JobSource::ScheduledBatch)) {
                EnqueueResult::Accepted => accepted += 1,
                EnqueueResult::Deduplicated => dedup += 1,
                EnqueueResult::QueueFull => full += 1,
                _ => unreachable!("non_exhaustive future variant"),
            }
        }
        let distinct = distinct_preserving(keys.clone()).len();
        prop_assert_eq!(full, 0);
        prop_assert_eq!(accepted, distinct);
        prop_assert_eq!(dedup, keys.len() - distinct);
        prop_assert_eq!(q.len(), distinct);
        prop_assert_eq!(q.is_empty(), distinct == 0);
    }

    #[test]
    fn p2_reenqueue_after_dequeue(k in key_strategy()) {
        let rt = current_thread_rt();
        let expected_key = k.clone();
        let result = rt.block_on(async move {
            let q: WorkQueue<String> = WorkQueue::new(4);
            Ok::<_, TestCaseError>((
                q.enqueue(job(&k, JobSource::ScheduledBatch)),
                q.enqueue(job(&k, JobSource::ScheduledBatch)),
                q.dequeue().await.expect("queued").domain_key,
                q.enqueue(job(&k, JobSource::ScheduledBatch)),
            ))
        });
        let (first, duplicate, dequeued, reenqueued) = result?;
        prop_assert_eq!(first, EnqueueResult::Accepted);
        prop_assert_eq!(duplicate, EnqueueResult::Deduplicated);
        prop_assert_eq!(dequeued, expected_key);
        prop_assert_eq!(reenqueued, EnqueueResult::Accepted);
    }

    #[test]
    fn p3_fifo_distinct(keys in prop::collection::vec(key_strategy(), 0..16)) {
        let rt = current_thread_rt();
        let distinct = distinct_preserving(keys);
        let expected = distinct.clone();
        let actual_input = distinct.clone();
        let got = rt.block_on(async move {
            let q: WorkQueue<String> = WorkQueue::new(actual_input.len().max(1));
            for k in &actual_input {
                let _ = q.enqueue(job(k, JobSource::ScheduledBatch));
            }
            let mut got = Vec::new();
            for k in &actual_input {
                let j = q.dequeue().await.expect("queued");
                got.push((j.domain_key, k.clone()));
            }
            got
        });
        prop_assert_eq!(got.len(), expected.len());
        for (actual, expected_key) in got {
            prop_assert_eq!(actual, expected_key);
        }
    }

    #[test]
    fn p4_source_neutrality(
        items in prop::collection::vec((key_strategy(), source_strategy()), 0..16)
    ) {
        let rt = current_thread_rt();
        let (order_a, order_b) = rt.block_on(async move {
            let distinct: Vec<(String, JobSource)> = {
                let mut seen = HashSet::new();
                items.into_iter().filter(|(k, _)| seen.insert(k.clone())).collect()
            };
            let with_sources: Vec<JobSpec<String>> = distinct
                .iter()
                .map(|(k, s)| job(k, s.clone()))
                .collect();
            let baseline: Vec<JobSpec<String>> = distinct
                .iter()
                .map(|(k, _)| job(k, JobSource::ScheduledBatch))
                .collect();

            let drain = |specs: Vec<JobSpec<String>>| async move {
                let q: WorkQueue<String> = WorkQueue::new(specs.len().max(1));
                for s in &specs {
                    let _ = q.enqueue(s.clone());
                }
                let mut got = Vec::new();
                for _ in 0..specs.len() {
                    got.push(q.dequeue().await.expect("queued").domain_key);
                }
                got
            };

            let order_a = drain(with_sources).await;
            let order_b = drain(baseline).await;
            (order_a, order_b)
        });
        prop_assert_eq!(order_a, order_b);
    }

    #[test]
    fn p5p6_outcomes_complete_and_correlation_propagates(
        keys in prop::collection::vec(key_strategy(), 1..8)
    ) {
        let rt = multi_thread_rt();
        let distinct = distinct_preserving(keys);
        let expected = distinct.clone();
        let result = rt.block_on(async move {
            let capacity = distinct.len();
            let queue = Arc::new(WorkQueue::new(capacity));
            let corrs: Vec<CorrelationContext> = distinct
                .iter()
                .map(|_| CorrelationContext::correlated(uuid::Uuid::now_v7()))
                .collect();
            for (k, c) in distinct.iter().zip(corrs.iter()) {
                let spec = JobSpec::new(
                    k.clone(),
                    format!("ctx-{k}"),
                    JobSource::ScheduledBatch,
                    c.clone(),
                );
                let _ = queue.enqueue(spec);
            }
            queue.close();

            let (tx, mut rx) = mpsc::channel(capacity + 4);
            let workers = 2usize.min(capacity).max(1);
            let mut cfg = WorkerPoolConfig::default();
            cfg.worker_count = workers;
            run_worker_pool(
                Arc::clone(&queue),
                Arc::new(ParityExecutor),
                Arc::new(BudgetGate::new(10_000, Duration::from_mins(1))),
                Arc::new(RateLimitState::default()),
                cfg,
                tokio_util::sync::CancellationToken::new(),
                tx,
            )
            .await;

            let mut got: HashMap<String, (CorrelationContext, bool)> = HashMap::new();
            while let Some(outcome) = rx.recv().await {
                match outcome {
                    JobOutcome::Success { domain_key, correlation, .. } => {
                        got.insert(domain_key, (correlation, true));
                    }
                    JobOutcome::Failure { domain_key, correlation, .. } => {
                        got.insert(domain_key, (correlation, false));
                    }
                    _ => return Err("unexpected JobOutcome variant"),
                }
            }
            Ok((got, corrs))
        });
        let (got, corrs) = result.map_err(TestCaseError::fail)?;
        prop_assert_eq!(got.len(), expected.len());
        for (k, expected_corr) in expected.iter().zip(corrs.iter()) {
            let (corr, succeeded) = got
                .get(k)
                .ok_or_else(|| TestCaseError::fail(format!("missing outcome for key {k}")))?;
            prop_assert_eq!(corr, expected_corr);
            prop_assert_eq!(*succeeded, k.len().is_multiple_of(2));
        }
    }
}
