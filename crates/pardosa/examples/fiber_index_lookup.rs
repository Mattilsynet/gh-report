//! `FiberIndex<K>` adopter example (ADR-0023 D1, D4, D5, D6).
//!
//! Two independent indices over the same journal, keyed on
//! different `K` families (a `u64` customer id and a
//! `&'static str` price-bucket tag) — illustrating D6's
//! separate-indices-per-key-family rule. Both extractors are
//! closure-first zero-to-many (D1): the customer-id one returns
//! one `K`; the bucket one returns 0–3. Lookups surface
//! `Empty` / `Unique(FiberId)` / `Diverged { fibers }` (D4).
//! Construction is opt-in (D5) via
//! [`StoreReader::fiber_index`](pardosa::store::StoreReader::fiber_index);
//! the index is in-memory only, log-derived (D2, D5).
use pardosa::prelude::*;
use pardosa::store::FiberLookup;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct OrderEvent {
    customer_id: u64,
    order_total_cents: u64,
}
impl HasEventSchemaSource for OrderEvent {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
impl Validate for OrderEvent {
    type Error = core::convert::Infallible;
    fn validate(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}
fn extract_customer(e: &Event<OrderEvent>) -> std::iter::Once<u64> {
    std::iter::once(e.domain_event().customer_id)
}
fn extract_price_buckets(e: &Event<OrderEvent>) -> Vec<&'static str> {
    let total = e.domain_event().order_total_cents;
    let mut buckets = Vec::new();
    if total >= 100 {
        buckets.push("ge_1usd");
    }
    if total >= 10_000 {
        buckets.push("ge_100usd");
    }
    if total >= 1_000_000 {
        buckets.push("ge_10000usd");
    }
    buckets
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "pardosa-fiber-index-example-{}.pgno",
        std::process::id()
    ));
    let mut store: EventStore<OrderEvent> = EventStore::create(&path)?;
    let _ = store.writer().begin(OrderEvent {
        customer_id: 42,
        order_total_cents: 50,
    })?;
    let _ = store.writer().begin(OrderEvent {
        customer_id: 42,
        order_total_cents: 250,
    })?;
    let _ = store.writer().begin(OrderEvent {
        customer_id: 999,
        order_total_cents: 2_000_000,
    })?;
    let _ = store.writer().sync()?;
    let by_customer = store.reader().fiber_index(extract_customer);
    let by_bucket = store.reader().fiber_index(extract_price_buckets);
    let report = |label: &str, look: FiberLookup<FiberId>| match look {
        FiberLookup::Empty => println!("  {label} -> Empty"),
        FiberLookup::Unique(fid) => println!("  {label} -> Unique({fid})"),
        FiberLookup::Diverged { fibers } => {
            println!(
                "  {label} -> Diverged({})",
                fibers
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        _ => println!("  {label} -> <unrecognised variant>"),
    };
    println!("by_customer index ({} keys):", by_customer.key_count());
    report("customer=42", by_customer.lookup(&42));
    report("customer=999", by_customer.lookup(&999));
    report("customer=7", by_customer.lookup(&7));
    println!("by_bucket index ({} keys):", by_bucket.key_count());
    report("ge_1usd", by_bucket.lookup(&"ge_1usd"));
    report("ge_100usd", by_bucket.lookup(&"ge_100usd"));
    report("ge_10000usd", by_bucket.lookup(&"ge_10000usd"));
    let _ = std::fs::remove_file(&path);
    Ok(())
}
