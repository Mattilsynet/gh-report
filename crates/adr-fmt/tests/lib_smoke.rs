//! Smoke test for the lib API: confirms `adr_fmt::run` is callable
//! from a library consumer (Track 3.1 — Phase 2 v2 C1 prerequisite
//! for `adr-srv` consuming `adr-fmt` as a library).
//!
//! The binary surface is exercised by `tests/integration.rs` (~84
//! tests); this file exists solely to prove the lib entry-point
//! compiles and runs without spawning a subprocess.

use std::ffi::OsString;

#[test]
fn run_default_mode_via_lib_api_returns_zero() {
    // Default mode (no flags) prints guidelines/setup to stdout and
    // returns exit code 0. Calling `run` directly proves the lib
    // entry-point is reachable from a library consumer.
    //
    // `--help` and `--version` would invoke clap's internal
    // `process::exit` and kill the test process, so we exercise
    // default-mode instead.
    let argv: Vec<OsString> = vec![OsString::from("adr-fmt")];
    let exit = adr_fmt::run(argv);
    assert_eq!(exit, 0, "default-mode run should exit 0");
}
