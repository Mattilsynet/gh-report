//! Projection sidecar: at-least-once consumer with crash-safe resume.
//!
//! A single `LineCursor` walks the global event line; the adopter
//! folds each event into an in-memory projection and calls
//! `commit_consumed` after each successful side effect. On
//! restart, a fresh `LineCursor` against the same sidecar resumes
//! exclusively after the last committed event (ADR-0011 §D2/§D5).
//!
//! At-least-once is the substrate's upper bound; exactly-once
//! side effects need the adopter to pair the projection write and
//! the sidecar commit at a higher layer (ADR-0018).
//!
//! ```sh
//! cargo run --example projection_sidecar -p pardosa
//! ```
use pardosa::store::{EventId, EventStore, GenomeSafe, HasEventSchemaSource, Validate};
use pardosa_schema::Timestamp;
use std::collections::BTreeMap;
use std::path::PathBuf;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
#[allow(clippy::struct_field_names)]
struct Order {
    order_id: u64,
    cents: u64,
    when: Timestamp,
}
impl HasEventSchemaSource for Order {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = Some("examples/projection_sidecar");
}
impl Validate for Order {
    type Error = core::convert::Infallible;
    fn validate(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}
fn order(order_id: u64, cents: u64) -> Order {
    Order {
        order_id,
        cents,
        when: Timestamp::from_nanos(order_id.max(1)).expect("nonzero"),
    }
}
#[derive(Debug, Default)]
struct OrderTotals(BTreeMap<u64, u64>);
impl OrderTotals {
    fn fold(&mut self, o: &Order) {
        *self.0.entry(o.order_id).or_insert(0) += o.cents;
    }
}
fn fresh_paths() -> (PathBuf, PathBuf) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let mut journal = std::env::temp_dir();
    journal.push(format!("pardosa-projection-{nanos}.pgno"));
    let mut sidecar = std::env::temp_dir();
    sidecar.push(format!("pardosa-projection-{nanos}.sidecar"));
    (journal, sidecar)
}
fn seed_three_orders(path: &std::path::Path) {
    let mut store: EventStore<Order> = EventStore::create(path).expect("create");
    let _r0 = store.writer().begin(order(1, 100)).expect("begin");
    let _r1 = store.writer().begin(order(2, 250)).expect("begin");
    let _r2 = store.writer().begin(order(1, 50)).expect("begin");
    let _lsn = store.writer().sync().expect("sync");
}
fn append_one_more(path: &std::path::Path) {
    let mut store: EventStore<Order> = EventStore::open_validated(path).expect("reopen");
    let _r3 = store.writer().begin(order(3, 999)).expect("begin");
    let _lsn = store.writer().sync().expect("sync");
}
fn run_consumer_session(
    journal: &std::path::Path,
    sidecar: &std::path::Path,
    totals: &mut OrderTotals,
) -> Option<EventId> {
    let store: EventStore<Order> = EventStore::open_validated(journal).expect("open_validated");
    let reader = store.reader();
    let mut cursor = reader.cursor(sidecar).expect("cursor open");
    let starting = cursor.acked_offset();
    println!("  resumed from acked_offset = {starting:?}");
    let mut last = starting;
    let events: Vec<_> = cursor.tail().map(|r| r.expect("tail item")).collect();
    for ev in &events {
        totals.fold(ev.domain_event());
        cursor
            .commit_consumed(ev)
            .expect("sidecar commit one fsync");
        last = Some(ev.event_id());
    }
    last
}
fn main() {
    let (journal, sidecar) = fresh_paths();
    println!("journal: {}", journal.display());
    println!("sidecar: {}", sidecar.display());
    seed_three_orders(&journal);
    let mut totals = OrderTotals::default();
    println!("\n--- consumer session 1 (cold start, 3 events) ---");
    let last = run_consumer_session(&journal, &sidecar, &mut totals);
    println!("  totals after session 1: {:?}", totals.0);
    println!("  acked through: {last:?}");
    append_one_more(&journal);
    println!("\n--- writer appended 1 more event ---");
    println!("\n--- consumer session 2 (warm resume, 1 new event) ---");
    let last = run_consumer_session(&journal, &sidecar, &mut totals);
    println!("  totals after session 2: {:?}", totals.0);
    println!("  acked through: {last:?}");
    println!("\n--- consumer session 3 (no new events; tail is empty) ---");
    let last = run_consumer_session(&journal, &sidecar, &mut totals);
    println!("  totals after session 3 (unchanged): {:?}", totals.0);
    println!("  acked through (unchanged): {last:?}");
}
