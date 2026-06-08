#![allow(unreachable_code, unused)]
use pardosa::store::{EventId, EventStore, GenomeSafe, HasEventSchemaSource};
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Payload {
    v: u64,
}
impl HasEventSchemaSource for Payload {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
fn use_commit_offset_from_adopter_code() {
    let journal = std::path::Path::new("/tmp/no-pub-commit-offset.pgno");
    let sidecar = std::path::Path::new("/tmp/no-pub-commit-offset.ack");
    let store: EventStore<Payload> = EventStore::<Payload>::create(journal).unwrap();
    let reader = store.reader();
    let mut cur = reader.cursor(sidecar).unwrap();
    let id: EventId = unimplemented!();
    let _ = cur.commit_offset(id);
}
fn main() {}
