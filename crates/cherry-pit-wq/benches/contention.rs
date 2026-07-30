//! Layer 5 — comparative contention/throughput bench: CURRENT's
//! election-based `BudgetGate::acquire` vs increment B's lock-free
//! `TokenBucketRegulator` debit, under N-thread contention.
//!
//! Both gates are sized so neither ever blocks (limit/capacity far
//! exceeds total calls issued) — this isolates the CAS-contention cost
//! of each design's coordination shape rather than measuring
//! cooldown-sleep or refill-wait latency (already covered by Layer 2's
//! loom interleaving-count contrast and Layer 4's clock-seam tests).

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use cherry_pit_wq::{Admission, BudgetGate, Clock, Regulator, TokenBucketRegulator};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tokio_util::sync::CancellationToken;

/// Fixed-instant clock — throughput bench issues far fewer calls than
/// the bucket's capacity, so no refill is ever needed; a fixed `now`
/// keeps the bench measuring pure CAS contention, not clock overhead.
struct FixedClock(std::time::Instant);

impl Clock for FixedClock {
    fn now(&self) -> std::time::Instant {
        self.0
    }
}

fn multi_thread_rt(worker_threads: usize) -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .enable_all()
        .build()
        .expect("multi_thread runtime")
}

const CALLS_PER_THREAD: usize = 200;

async fn contended_acquire_current(gate: Arc<BudgetGate>, threads: usize) {
    let cancel = CancellationToken::new();
    let mut handles = Vec::with_capacity(threads);
    for _ in 0..threads {
        let g = Arc::clone(&gate);
        let c = cancel.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..CALLS_PER_THREAD {
                assert!(g.acquire(&c).await);
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
}

async fn contended_admit_b(bucket: Arc<TokenBucketRegulator>, threads: usize) {
    let cancel = CancellationToken::new();
    let mut handles = Vec::with_capacity(threads);
    for _ in 0..threads {
        let b = Arc::clone(&bucket);
        let c = cancel.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..CALLS_PER_THREAD {
                assert_eq!(b.admit(&c).await, Admission::Admitted);
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
}

fn bench_contention(c: &mut Criterion) {
    let mut group = c.benchmark_group("budget_gate_vs_token_bucket_contention");
    for &threads in &[1usize, 4, 16] {
        let total_calls = (CALLS_PER_THREAD * threads) as u64;
        group.throughput(Throughput::Elements(total_calls));

        group.bench_with_input(
            BenchmarkId::new("current_budget_gate_acquire", threads),
            &threads,
            |b, &threads| {
                let rt = multi_thread_rt(threads.max(1));
                b.iter(|| {
                    let gate = Arc::new(BudgetGate::new(
                        u64::try_from(CALLS_PER_THREAD * threads + 1).unwrap_or(u64::MAX),
                        Duration::from_hours(1),
                    ));
                    rt.block_on(contended_acquire_current(
                        black_box(Arc::clone(&gate)),
                        threads,
                    ));
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("b_token_bucket_admit", threads),
            &threads,
            |b, &threads| {
                let rt = multi_thread_rt(threads.max(1));
                b.iter(|| {
                    let clock: Arc<dyn Clock> = Arc::new(FixedClock(std::time::Instant::now()));
                    let bucket = Arc::new(TokenBucketRegulator::new(
                        clock,
                        u64::try_from(CALLS_PER_THREAD * threads + 1).unwrap_or(u64::MAX),
                        1,
                    ));
                    rt.block_on(contended_admit_b(black_box(Arc::clone(&bucket)), threads));
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_contention);
criterion_main!(benches);
