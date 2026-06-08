use pardosa::store::{ExtractError, FiberIndex, FiberLookup};
use pardosa::store::{Event, FiberId};
fn _names_used() {
    let _: FiberIndex<u64> = FiberIndex::empty();
    let _: FiberLookup<FiberId> = FiberLookup::Empty;
    let _ = std::mem::size_of::<ExtractError>();
}
fn _extractor_shape_is_closure_first_zero_to_many<T>()
where
    T: 'static,
{
    let _build = |events: &[Event<T>]| -> FiberIndex<u64> {
        FiberIndex::build(events, |_e: &Event<T>| -> Vec<u64> { Vec::new() })
    };
    let _build_one = |events: &[Event<T>]| -> FiberIndex<u64> {
        FiberIndex::build(events, |_e: &Event<T>| std::iter::once(0u64))
    };
}
fn _prelude_reaches_index_types() {
    use pardosa::prelude::*;
    fn accept_index(_: FiberIndex<u64>) {}
    fn accept_lookup(_: FiberLookup<FiberId>) {}
    fn accept_error(_: ExtractError) {}
    let _ = accept_index;
    let _ = accept_lookup;
    let _ = accept_error;
}
fn _diverged_carries_fibers_in_log_order(look: FiberLookup<FiberId>) {
    match look {
        FiberLookup::Empty => {}
        FiberLookup::Unique(_) => {}
        FiberLookup::Diverged { fibers } => {
            let _: Vec<FiberId> = fibers;
        }
        _ => {}
    }
}
fn main() {
    let _ = _names_used;
    let _ = _extractor_shape_is_closure_first_zero_to_many::<u64>;
    let _ = _prelude_reaches_index_types;
    let _ = _diverged_carries_fibers_in_log_order;
}
