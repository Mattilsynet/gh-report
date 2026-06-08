//! Adopter-facing 2-param reader/view/writer forms are nameable
//! without the `W` storage generic (ADR-0018 § Naming,
//! docs/adr/0018-public-event-store-api.md:1116-1190).
//!
//! Pins: `StoreReader<'_, T>`, `FiberHistory<'_, T>`,
//! `CausalChain<'_, T>`, `StoreWriter<'_, T>` — the `W = std::fs::File`
//! default makes the generic-`W` form invisible at the surface.
use pardosa::store::{
    CausalChain, EventStore, FiberHistory, GenomeSafe, HasEventSchemaSource, StoreReader,
    StoreWriter,
};
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Payload {
    v: u64,
}
impl HasEventSchemaSource for Payload {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
#[test]
fn two_param_reader_and_views_name_path_backed_handles() {
    fn accept_reader(_r: &StoreReader<'_, Payload>) {}
    fn accept_writer(_w: &StoreWriter<'_, Payload>) {}
    fn accept_history(_h: &FiberHistory<'_, Payload>) {}
    fn accept_chain(_c: &CausalChain<'_, Payload>) {}
    let mut store: EventStore<Payload> = EventStore::<Payload>::create(
        std::env::temp_dir()
            .join(format!("pardosa-2param-{}.pgno", std::process::id()))
            .as_path(),
    )
    .expect("create");
    let r0 = store.writer().begin(Payload { v: 1 }).expect("begin");
    let head = r0.event_id();
    let fid = r0.fiber().fiber_id();
    let reader: StoreReader<'_, Payload> = store.reader();
    accept_reader(&reader);
    let hist: FiberHistory<'_, Payload> = reader.fiber(fid);
    accept_history(&hist);
    let chain: CausalChain<'_, Payload> = reader.causal_chain(head);
    accept_chain(&chain);
    let writer: StoreWriter<'_, Payload> = store.writer();
    accept_writer(&writer);
}
