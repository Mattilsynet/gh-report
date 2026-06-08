//! [`EventStore`] persist + read round-trip over a path-backed `.pgno`.
//!
//! Demonstrates a small end-to-end flow through the adopter-facing
//! `pardosa::store` surface: create a fresh log, append a handful of
//! payloads through [`StoreWriter::begin`], sync durable bytes, then
//! reopen the same path and walk each fiber history.
//!
//! Run:
//!
//! ```sh
//! cargo run --example persist_round_trip -p pardosa
//! ```
use pardosa::store::{EventStore, GenomeSafe, HasEventSchemaSource, LiveFiber, Validate};
use pardosa_schema::Timestamp;
use std::path::PathBuf;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Sample {
    when: Timestamp,
    v: u64,
}
impl HasEventSchemaSource for Sample {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = Some("examples/persist_round_trip");
}
impl Validate for Sample {
    type Error = core::convert::Infallible;
    fn validate(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}
fn sample(v: u64) -> Sample {
    Sample {
        when: Timestamp::from_nanos(v.max(1)).expect("nonzero"),
        v,
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path: PathBuf = {
        let mut p = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        p.push(format!("pardosa-persist-round-trip-{nanos}.pgno"));
        p
    };
    let mut store: EventStore<Sample> = EventStore::create(&path)?;
    let mut fibers: Vec<LiveFiber> = Vec::new();
    for n in [10u64, 11, 20] {
        let live = store.writer().begin(sample(n))?.fiber();
        println!("started {n} -> {:?}", live.fiber_id());
        fibers.push(live);
    }
    let _lsn = store.writer().sync()?;
    drop(store);
    let store2: EventStore<Sample> = EventStore::open_validated(&path)?;
    let reader = store2.reader();
    let mut count = 0usize;
    for live in &fibers {
        for _ev in reader.fiber(live.fiber_id()).iter()? {
            count += 1;
        }
    }
    println!("streamed {count} events from {}", path.display());
    assert_eq!(count, 3);
    Ok(())
}
