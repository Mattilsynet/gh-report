//! Dragline operation benches for pardosa.
//!
//! Mission M4: harness only — no baselines, no regression budgets. Wave 3
//! perf work consumes these to record numbers.
//!
//! Run all: `cargo bench -p pardosa`
//! Run one: `cargo bench -p pardosa -- create_100_events`

// .unwrap() is the idiomatic shape inside criterion bench bodies — a panic
// fails the bench loudly rather than silently skewing numbers.
#![allow(clippy::missing_panics_doc)]

use criterion::{Criterion, criterion_group, criterion_main};
use pardosa::{DomainId, Dragline};
use std::hint::black_box;

const N: usize = 100;

fn bench_create(c: &mut Criterion) {
    c.bench_function("create_100_events", |b| {
        b.iter(|| {
            let mut d: Dragline<&'static str> = Dragline::new();
            for i in 0..N {
                d.create(black_box(i as i64), black_box("payload")).unwrap();
            }
            d
        });
    });
}

fn bench_read(c: &mut Criterion) {
    let mut d: Dragline<&'static str> = Dragline::new();
    let mut last: Option<DomainId> = None;
    for i in 0..N {
        let r = d.create(i as i64, "payload").unwrap();
        last = Some(r.domain_id);
    }
    let target = last.expect("at least one event created");
    c.bench_function("read_recent_after_100", |b| {
        b.iter(|| d.read(black_box(target)).unwrap());
    });
}

fn bench_verify(c: &mut Criterion) {
    let mut d: Dragline<&'static str> = Dragline::new();
    for i in 0..N {
        d.create(i as i64, "payload").unwrap();
    }
    c.bench_function("verify_invariants_100", |b| {
        b.iter(|| d.verify_invariants().unwrap());
    });
}

criterion_group!(benches, bench_create, bench_read, bench_verify);
criterion_main!(benches);
