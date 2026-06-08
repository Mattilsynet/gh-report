//! Harness-side coverage of the cfg-gated test-support
//! [`pardosa::store::EventStore::open`] symbol (ADR-0018 §D7).
//!
//! Mirrors the type-witness style of
//! `crates/pardosa/tests/store_surface_smoke.rs` for the
//! one signature that becomes `pub` only under
//! `feature = "test-support"`. Living here keeps the adopter-
//! facing smoke file (which still pins every default-feature
//! `pardosa::store::*` re-export) crate-local to `pardosa`
//! while exercising the wider `open` visibility under the
//! gate (mission `pardosa-test-matrix-split-20260606`).
#![allow(dead_code, unused_imports)]
use pardosa::store::{
    CausalChain, Decode, EventStore, FiberHistory, GenomeSafe, LineCursor, PardosaError,
};
fn type_witness<T>(_: T) {}
fn open_signature_under_test_support<T: Decode + GenomeSafe>() {
    fn fiber_history_nameable<U: Decode + GenomeSafe>(_h: &FiberHistory<'_, U>) {}
    fn causal_chain_nameable<U: Decode + GenomeSafe>(_c: &CausalChain<'_, U>) {}
    fn line_cursor_nameable<U: Decode + GenomeSafe>(_c: &LineCursor<U>) {}
    type_witness::<fn(&std::path::Path) -> Result<EventStore<T>, PardosaError>>(
        EventStore::<T>::open,
    );
    type_witness::<fn(&FiberHistory<'_, T>)>(fiber_history_nameable::<T>);
    type_witness::<fn(&CausalChain<'_, T>)>(causal_chain_nameable::<T>);
    type_witness::<fn(&LineCursor<T>)>(line_cursor_nameable::<T>);
}
#[test]
fn open_reader_bounds_compile_under_test_support_feature() {
    open_signature_under_test_support::<u64>();
}
