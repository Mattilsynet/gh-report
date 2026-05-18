//! Encoding round-trip benches for pardosa-encoding.
//!
//! Mission M4: harness only — no baselines, no regression budgets. Wave 3
//! perf work consumes these to record numbers.
//!
//! Run all: `cargo bench -p pardosa-encoding`
//! Run one: `cargo bench -p pardosa-encoding -- encode_u64`

// .unwrap() is the idiomatic shape inside criterion bench bodies — a panic
// fails the bench loudly rather than silently skewing numbers.
#![allow(clippy::missing_panics_doc)]

use criterion::{Criterion, criterion_group, criterion_main};
use pardosa_encoding::{from_bytes, to_vec};
use std::hint::black_box;

fn bench_u64(c: &mut Criterion) {
    let value: u64 = 0xDEAD_BEEF_CAFE_F00D;
    c.bench_function("encode_u64", |b| {
        b.iter(|| to_vec(black_box(&value)));
    });
    let bytes = to_vec(&value);
    c.bench_function("decode_u64", |b| {
        b.iter(|| from_bytes::<u64>(black_box(&bytes)).unwrap());
    });
}

fn bench_vec_u8(c: &mut Criterion) {
    let value: Vec<u8> = (0..=255).collect();
    c.bench_function("encode_vec_u8_256", |b| {
        b.iter(|| to_vec(black_box(&value)));
    });
    let bytes = to_vec(&value);
    c.bench_function("decode_vec_u8_256", |b| {
        b.iter(|| from_bytes::<Vec<u8>>(black_box(&bytes)).unwrap());
    });
}

fn bench_tuple(c: &mut Criterion) {
    let value: (u32, u64, bool) = (42, 0xCAFE_BABE_DEAD_BEEF, true);
    c.bench_function("encode_tuple_u32_u64_bool", |b| {
        b.iter(|| to_vec(black_box(&value)));
    });
    let bytes = to_vec(&value);
    c.bench_function("decode_tuple_u32_u64_bool", |b| {
        b.iter(|| from_bytes::<(u32, u64, bool)>(black_box(&bytes)).unwrap());
    });
}

criterion_group!(benches, bench_u64, bench_vec_u8, bench_tuple);
criterion_main!(benches);
