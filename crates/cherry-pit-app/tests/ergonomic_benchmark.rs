//! Ergonomic LOC benchmark per S7 §2 + WU-5 brief lines 85–94.
//!
//! Counts non-blank, non-comment lines in `domain.rs` and `wiring.rs`
//! of the 2-aggregate fixture and asserts `wiring_loc <= domain_loc`.
//! If this fires, S7 `abort_if` #1 routes to the user (CHE-0051
//! ergonomic-claim falsifier).
//!
//! LOC methodology (mitigation #6): inline counter, no external
//! `tokei` / `scc` dep. Comment-line detection uses a leading `//`
//! after whitespace trim; doc comments (`///`, `//!`) and inner
//! attributes also start with `//` and so are counted as comments.
//! Blank lines are skipped. This matches the prose in the contract
//! verbatim.

const DOMAIN_PATH: &str = "tests/two_aggregate_fixture/domain.rs";
const WIRING_PATH: &str = "tests/two_aggregate_fixture/wiring.rs";

fn loc(path: &str) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {path}: {e}"))
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with("//")
        })
        .count()
}

#[test]
fn wiring_loc_does_not_exceed_domain_loc() {
    let domain = loc(DOMAIN_PATH);
    let wiring = loc(WIRING_PATH);
    assert!(
        wiring <= domain,
        "ergonomic benchmark failed: wiring_loc ({wiring}) > domain_loc ({domain}); \
         CHE-0051 ergonomic claim falsified — see WU-5 brief line 127"
    );
}
