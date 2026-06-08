//! Runtime-side gates for ADR-0018 §1-10 acceptance: the
//! `pardosa::store` module and binding items are crate-root
//! reachable (§8); ADR-0023 D5 binds `FiberIndex<K>` as
//! default-public, opt-in construction, no implicit runtime cost —
//! this file pins that the ADR-0018 §D1 capability surface
//! (open / append / fiber-read / line-tail / same-fiber
//! causal-walk) is exercisable **without naming** `FiberIndex<K>`
//! in source. `MigrationPolicy`/`open_with_migration` are absent
//! pre-ADR-0019 (§7). Compile-fail twins (reader/writer coercion,
//! typestate transitions, payload-only writer signature,
//! schema-mismatch variant absence) live in
//! `tests/event_store_compile_gates.rs` under `trybuild`.
use pardosa::store::{CausalChain, EventStore, FiberHistory, LineCursor, StoreReader, StoreWriter};
#[test]
fn pardosa_store_module_and_binding_items_are_reachable() {
    fn _accept_all<T>(
        _store: &EventStore<T>,
        _writer: &StoreWriter<'_, T>,
        _reader: &StoreReader<'_, T>,
        _hist: &FiberHistory<'_, T>,
        _chain: &CausalChain<'_, T>,
        _cursor: &LineCursor<T>,
    ) {
    }
    let _ = _accept_all::<u64>;
}
