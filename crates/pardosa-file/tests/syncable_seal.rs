//! Trybuild harness: `Syncable` is strong-sealed (ADR-0014
//! §F3).
//!
//! Downstream cannot `impl Syncable` — `sealed::Sealed`
//! supertrait is module-private. Fixture under
//! `tests/syncable_seal/` exercises the denial.
//!
//! Committed `.stderr` golden lists production impls
//! including the `test-support`-gated `FailureSink` (qf9h.8),
//! so the test is gated on the feature for deterministic
//! diagnostic ordering.
#![cfg(feature = "test-support")]
#[test]
fn syncable_is_sealed() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/syncable_seal/downstream_impl_denied.rs");
}
