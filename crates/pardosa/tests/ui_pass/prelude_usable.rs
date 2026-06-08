use pardosa::prelude::*;
use pardosa_schema::Timestamp;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Demo {
    n: u64,
    when: Timestamp,
}
impl HasEventSchemaSource for Demo {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = Some("ui_pass/prelude_usable");
}
impl Validate for Demo {
    type Error = core::convert::Infallible;
    fn validate(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}
fn must_compile() -> Result<(), PardosaError> {
    let path = std::path::PathBuf::from("/tmp/prelude_usable.pgno");
    let mut store: EventStore<Demo> = EventStore::create(&path)?;
    let receipt: AppendReceipt = store.writer().begin(Demo {
        n: 1,
        when: Timestamp::from_nanos(1).expect("nonzero"),
    })?;
    let live: LiveFiber = receipt.fiber();
    let _id: FiberId = live.fiber_id();
    let _: Lsn = store.writer().sync().expect("sync");
    Ok(())
}
fn opens_validated() -> Result<(), ValidatedReplayError<core::convert::Infallible>> {
    let _: EventStore<Demo> = EventStore::open_validated(std::path::Path::new("/dev/null"))?;
    Ok(())
}
fn touches_views(store: &EventStore<Demo>, fid: FiberId) {
    let reader: StoreReader<'_, Demo> = store.reader();
    let _ = reader.fiber(fid);
}
fn _detached_typestate(d: DetachedFiber) -> FiberId {
    d.fiber_id()
}
fn _detach_receipt(r: DetachReceipt) -> FiberId {
    r.fiber().fiber_id()
}
fn _event_accessors(ev: Event<Demo>) -> (EventId, Precursor, [u8; 32]) {
    (ev.event_id(), ev.precursor(), ev.precursor_hash())
}
fn _names_used(
    _: FiberState,
    _: Frontier,
    _: StoreMetadata,
    _: EnvelopeError,
    _: CausalChainError,
) {
}
fn _replay_surface() {
    fn _names<R: std::io::Read + std::io::Seek>() {
        let _ = replay::stream_checked::<R, Demo>;
        let _ = replay::stream_validated::<R, Demo>;
    }
}
fn _migrate_surface() {
    fn _refer<New: Decode + Encode + GenomeSafe + Validate + HasEventSchemaSource>() {
        let _ = migrate::migrate_keep::<
            Demo,
            New,
            core::convert::Infallible,
            fn(Event<Demo>) -> Result<New, core::convert::Infallible>,
        >;
    }
}
fn _publisher_surface(_: Box<dyn FrontierPublisher>, _: PublishError) {}
fn _iter_types(
    _: CausalChain<'_, Demo>,
    _: FiberHistory<'_, Demo>,
    _: LineCursor<Demo>,
    _: CausalChainIter<'_, Demo>,
    _: CausalChainStrictIter<'_, Demo>,
    _: FiberHistoryIter<'_, Demo>,
    _: HistoryStream<'_, Demo>,
    _: StoreWriter<'_, Demo>,
    _: Index,
) {
}
fn _codec_bounds<T: Decode + Encode + GenomeSafe + Validate>() {}
fn main() {
    let _ = must_compile;
    let _ = opens_validated;
    let _ = touches_views;
    let _ = _codec_bounds::<Demo>;
}
