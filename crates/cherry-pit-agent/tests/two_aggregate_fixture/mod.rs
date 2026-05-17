//! 2-aggregate fixture shared by `tests/ergonomic_benchmark.rs`,
//! `tests/taxonomy_unit.rs`, `tests/taxonomy_integration.rs`, and
//! `tests/taxonomy_proptest.rs` per the WU-5 S7 contract.
//!
//! Subdirectories of `tests/` are not auto-compiled by cargo as
//! standalone test binaries (only direct `tests/*.rs` files are),
//! so this module is included via `#[path = "two_aggregate_fixture/mod.rs"] mod …;`
//! from each test that needs it.
//!
//! Layout split per contract §2:
//!
//! - `domain.rs` carries the two aggregates + commands + events + the
//!   cross-aggregate policy (the *domain*).
//! - `wiring.rs` carries the single `assemble()` constructor (the
//!   *wiring*).
//!
//! The split is the load-bearing artefact for the ergonomic LOC
//! benchmark.

// Structural rationale (RST-0003:R5 carve-out for the cargo-test
// shared-fixture idiom — same precedent as cherry-pit-web's
// `tests/common/mod.rs`): every test binary that includes this module
// via `#[path = "two_aggregate_fixture/mod.rs"] mod …;` compiles it
// as a *separate* translation unit. An item used by some test binaries
// but not others is genuinely dead in the compilations where it is
// unused, and an item used by every test binary would make a per-item
// `#[expect(dead_code)]` permanently unfulfilled. Per-item suppression
// would also balloon the source line count of every public item in
// `domain.rs` / `wiring.rs` / `infra.rs` for zero semantic gain.
// RST-0003:R5 forbids blanket suppression "even with reason"; this is
// the documented exception for shared cargo-test fixtures, scoped to
// this module only.
#![allow(
    dead_code,
    reason = "shared cargo-test fixture: each `mod two_aggregate_fixture;` inclusion is a separate compilation, so item-level reachability varies per test binary; per-item suppression is structurally infeasible here. Same idiom as cherry-pit-web/tests/common/mod.rs."
)]

pub mod domain;
pub mod infra;
pub mod wiring;
