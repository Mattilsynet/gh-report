/// Trybuild compile-fail / compile-pass fixtures pin
/// **default-feature** public-API contracts (ADR-0018 §D7).
/// They are skipped when the `test-support` feature is active
/// because cargo's per-build feature unification widens the
/// public surface (e.g. `Event::try_new` becomes `pub(crate)`
/// rather than absent) and changes rustc's help-text
/// suggestion lists, producing legitimate stderr drift that
/// is not a regression of the gates being tested. The
/// `pardosa-test-support-harness` integration tests cover the
/// `test-support` half of the matrix (mission
/// `pardosa-test-matrix-split-20260606`).
#[cfg(not(feature = "test-support"))]
#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
    t.pass("tests/ui_pass/*.rs");
}
