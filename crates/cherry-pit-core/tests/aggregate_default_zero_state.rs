//! CHE-0012 R1: `Aggregate::default()` is zero state. The trybuild fixture
//! `tests/compile_fail/aggregate_requires_default.rs` pins the trait bound
//! (`impl Aggregate for X` requires `X: Default`). This test pins the
//! *public re-exported aggregate set* against a snapshot so that adding or
//! removing a concrete public aggregate is a conscious change that demands
//! both an ADR citation in the commit message and an explicit snapshot
//! update.
//!
//! The set is intentionally maintained by hand in `CURRENT_PUBLIC_AGGREGATES`
//! below. Divergence between this constant and `tests/fixtures/public_aggregates.txt`
//! fails the test with a diff and instructions.
//!
//! As of 1778750000: cherry-pit-core re-exports the `Aggregate` and
//! `HandleCommand` traits but no concrete aggregate types — those live in
//! downstream domain crates. Snapshot is therefore empty.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

/// Concrete public types re-exported from `cherry_pit_core` that implement
/// `Aggregate`. Maintained by hand. Empty as of 1778750000 (see module
/// doc). Add a name here AND in `tests/fixtures/public_aggregates.txt` when
/// re-exporting a new concrete aggregate. CHE-0012 R1 binds every entry.
const CURRENT_PUBLIC_AGGREGATES: &[&str] = &[];

#[test]
fn public_aggregate_set_matches_snapshot() {
    let actual: BTreeSet<&str> = CURRENT_PUBLIC_AGGREGATES.iter().copied().collect();

    let snapshot_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("public_aggregates.txt");
    let snapshot_raw = fs::read_to_string(&snapshot_path)
        .unwrap_or_else(|e| panic!("snapshot file {} unreadable: {e}", snapshot_path.display()));
    let expected: BTreeSet<String> = snapshot_raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(ToOwned::to_owned)
        .collect();

    let actual_owned: BTreeSet<String> = actual.iter().map(|s| (*s).to_owned()).collect();

    if actual_owned != expected {
        let added: Vec<&String> = actual_owned.difference(&expected).collect();
        let removed: Vec<&String> = expected.difference(&actual_owned).collect();
        panic!(
            "public aggregate set drifted from snapshot.\n\
             added (in CURRENT_PUBLIC_AGGREGATES, missing from snapshot): {added:?}\n\
             removed (in snapshot, missing from CURRENT_PUBLIC_AGGREGATES): {removed:?}\n\
             \n\
             To update: edit tests/fixtures/public_aggregates.txt to match the\n\
             current set AND cite the relevant ADR (typically CHE-0012) in your\n\
             commit message. Every entry in this set must implement Default\n\
             (CHE-0012 R1: aggregate default = zero state). The trybuild fixture\n\
             tests/compile_fail/aggregate_requires_default.rs enforces the bound\n\
             at compile time."
        );
    }
}
