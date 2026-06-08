//! qf9h.25 — large-index reader memory profile.
//!
//! Exercises `Reader::open`'s speculative
//! `Vec::<IndexEntry>::with_capacity` (`reader.rs:262-288`):
//!
//! 1. **Bounded growth** — 1k/10k/100k entries; memory
//!    proportional to `message_count * INDEX_ENTRY_SIZE`.
//!    100k × 24 ≈ 2.4 MiB.
//! 2. **Hostile-file rejection** — `IndexTooLarge` /
//!    `IndexOverflow` reject files above
//!    [`ReaderOptions::max_message_count`] (default ≈ 44.7M)
//!    **before** alloc. Covered by `file_reader` tests.
//!
//! ## Memory recipe
//!
//! ```sh
//! /usr/bin/time -l cargo bench -p pardosa-file --bench reader_index
//! /usr/bin/time -v cargo bench -p pardosa-file --bench reader_index
//! ```
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use pardosa_file::{Reader, Writer};
use std::hint::black_box;
use std::io::Cursor;
const KNOWN_HASH: u128 = 0x0F0E_0D0C_0B0A_0908_0706_0504_0302_0100;
/// Build a well-formed `.pgno` byte buffer with `n` tiny messages.
/// Each message is a single-byte payload so the on-disk size is
/// dominated by the index region (24 bytes/entry) rather than the
/// payloads — what this bench is parameterising.
fn pgno_with_n_messages(n: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut w = Writer::new(&mut buf, KNOWN_HASH);
    let payload = [0u8; 1];
    for _ in 0..n {
        w.write_message(&payload).expect("write_message");
    }
    w.finish().expect("finish");
    buf
}
fn bench_reader_open_index(c: &mut Criterion) {
    let mut group = c.benchmark_group("reader_open_index");
    for &n in &[1_000usize, 10_000, 100_000] {
        let buf = pgno_with_n_messages(n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &buf, |b, buf| {
            b.iter(|| {
                let r = Reader::open(Cursor::new(black_box(buf.as_slice()))).expect("Reader::open");
                let _ = black_box(r.message_count());
            });
        });
    }
    group.finish();
}
criterion_group!(benches, bench_reader_open_index);
criterion_main!(benches);
