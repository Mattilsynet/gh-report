//!Reachability pin (mission pardosa-architecture-spine-20260526, track spine-02-identifier-minting). On default features the raw mint entry points `EventId::new`, `FiberId::new`, `Index::new`, and the `From<u64> for EventId` conversion are absent from the public API; substrate-owned id construction goes through the crate-private `from_decoded` constructors. This crate has no `pardosa/test-support` activation in its dependency graph, so the harness sees the same surface as an external default-features consumer.
#[test]
fn raw_mint_entries_are_private_on_default_features() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/id_minting_compile_fail/*.rs");
}
