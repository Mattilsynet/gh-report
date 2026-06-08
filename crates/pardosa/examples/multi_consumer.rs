//! Multiple independent consumers tailing one journal.
//!
//! Each `LineCursor` is one exclusive-resume consumer
//! (ADR-0011 §D2). Two adopter-owned consumers — an audit log
//! summariser and a billing aggregator — each carry their own
//! sidecar path and commit independently. No consumer-group
//! coordinator; no partition assignment (ADR-0018).
//!
//! Pointing two cursors at the same sidecar is a deployment
//! error, not a supported split-the-work pattern. Splitting work
//! across consumers is an application-layer shard above Pardosa.
//!
//! ```sh
//! cargo run --example multi_consumer -p pardosa
//! ```
use pardosa::store::{EventStore, GenomeSafe, HasEventSchemaSource, Validate};
use pardosa_schema::Timestamp;
use std::path::PathBuf;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Charge {
    account_id: u64,
    cents: u64,
    when: Timestamp,
}
impl HasEventSchemaSource for Charge {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = Some("examples/multi_consumer");
}
impl Validate for Charge {
    type Error = core::convert::Infallible;
    fn validate(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}
fn charge(account_id: u64, cents: u64) -> Charge {
    Charge {
        account_id,
        cents,
        when: Timestamp::from_nanos((account_id + cents).max(1)).expect("nonzero"),
    }
}
fn fresh_paths() -> (PathBuf, PathBuf, PathBuf) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let mut tmp = std::env::temp_dir();
    tmp.push(format!("pardosa-multi-{nanos}"));
    let journal = tmp.with_extension("pgno");
    let sidecar_audit = tmp.with_extension("audit.sidecar");
    let sidecar_billing = tmp.with_extension("billing.sidecar");
    (journal, sidecar_audit, sidecar_billing)
}
fn seed(path: &std::path::Path) {
    let mut store: EventStore<Charge> = EventStore::create(path).expect("create");
    let _r0 = store.writer().begin(charge(1, 100)).expect("begin");
    let _r1 = store.writer().begin(charge(2, 200)).expect("begin");
    let _r2 = store.writer().begin(charge(1, 50)).expect("begin");
    let _r3 = store.writer().begin(charge(3, 800)).expect("begin");
    let _lsn = store.writer().sync().expect("sync");
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (journal, sidecar_audit, sidecar_billing) = fresh_paths();
    println!("journal:         {}", journal.display());
    println!("sidecar (audit): {}", sidecar_audit.display());
    println!("sidecar (bill):  {}", sidecar_billing.display());
    seed(&journal);
    let store: EventStore<Charge> = EventStore::open_validated(&journal)?;
    let reader = store.reader();
    let mut audit_cursor = reader.cursor(&sidecar_audit)?;
    let audit_events: Vec<_> = audit_cursor.tail().collect::<Result<_, _>>()?;
    for ev in &audit_events {
        audit_cursor.commit_consumed(ev)?;
    }
    println!("\naudit consumer observed {} events", audit_events.len());
    let mut billing_cursor = reader.cursor(&sidecar_billing)?;
    let billing_events: Vec<_> = billing_cursor.tail().collect::<Result<_, _>>()?;
    let mut total_cents: u64 = 0;
    for ev in &billing_events {
        total_cents += ev.domain_event().cents;
        billing_cursor.commit_consumed(ev)?;
    }
    println!("billing consumer summed {total_cents} cents across the line");
    let mut audit_warm = reader.cursor(&sidecar_audit)?;
    let mut billing_warm = reader.cursor(&sidecar_billing)?;
    println!(
        "\nwarm-restart positions: audit={:?}  billing={:?}",
        audit_warm.acked_offset(),
        billing_warm.acked_offset()
    );
    let audit_new: Vec<_> = audit_warm.tail().map(|r| r.expect("audit")).collect();
    let billing_new: Vec<_> = billing_warm.tail().map(|r| r.expect("billing")).collect();
    println!(
        "warm tail: audit yielded {} new event(s), billing yielded {} new event(s)",
        audit_new.len(),
        billing_new.len(),
    );
    Ok(())
}
