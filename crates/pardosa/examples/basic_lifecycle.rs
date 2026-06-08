//! Basic [`EventStore`] lifecycle: create → begin → sync → reopen → read.
//!
//! Demonstrates the adopter-facing path-backed workflow exposed by
//! [`pardosa::store::EventStore`]: open a fresh `.pgno`, start fibers,
//! sync durable bytes, then reopen the same path and iterate fiber
//! histories in commit order.
//!
//! Run with:
//!
//! ```sh
//! cargo run --example basic_lifecycle -p pardosa
//! ```
use pardosa::store::{EventStore, GenomeSafe, HasEventSchemaSource, LiveFiber, Validate};
use pardosa_schema::Timestamp;
use std::path::PathBuf;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Tick {
    when: Timestamp,
    seq: u64,
}
impl HasEventSchemaSource for Tick {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = Some("examples/basic_lifecycle");
}
impl Validate for Tick {
    type Error = core::convert::Infallible;
    fn validate(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}
fn tick(seq: u64) -> Tick {
    Tick {
        when: Timestamp::from_nanos(seq.max(1)).expect("nonzero"),
        seq,
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path: PathBuf = {
        let mut p = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        p.push(format!("pardosa-basic-lifecycle-{nanos}.pgno"));
        p
    };
    let mut store: EventStore<Tick> = EventStore::create(&path)?;
    let live_a: LiveFiber = store.writer().begin(tick(100))?.fiber();
    let live_b: LiveFiber = store.writer().begin(tick(200))?.fiber();
    let live_c: LiveFiber = store.writer().begin(tick(300))?.fiber();
    println!(
        "started fibers: {:?} {:?} {:?}",
        live_a.fiber_id(),
        live_b.fiber_id(),
        live_c.fiber_id(),
    );
    let lsn = store.writer().sync()?;
    println!("synced to {}: lsn={lsn:?}", path.display());
    drop(store);
    let store2: EventStore<Tick> = EventStore::open_validated(&path)?;
    let reader = store2.reader();
    for fid in [live_a.fiber_id(), live_b.fiber_id(), live_c.fiber_id()] {
        let history = reader.fiber(fid);
        for ev in history.iter()? {
            println!("  {:?}", ev.event_id());
        }
    }
    Ok(())
}
