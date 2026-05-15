//! Compile-fail harness — locks `dyn`-incompat invariants of the public
//! port surface per CHE-0049:R12 + CHE-0005:R1. Pattern mirrors WU-3
//! SM-3.2 (`crates/cherry-pit-projection/tests/compile_tests.rs`).

#[cfg(feature = "projection")]
#[test]
fn compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/*.rs");
}
