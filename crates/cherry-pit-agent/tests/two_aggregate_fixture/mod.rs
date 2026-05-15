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

#![allow(
    dead_code,
    reason = "fixture shared by multiple test binaries; each binary uses a subset"
)]

pub mod domain;
pub mod infra;
pub mod wiring;
