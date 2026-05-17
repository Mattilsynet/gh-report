//! Smoke + API probe tests for the lib surface (Track 3.1).
//!
//! Two purposes:
//!   1. `run_default_mode_via_lib_api_returns_zero` — proves the
//!      top-level entry-point `adr_fmt::run` is callable from a
//!      library consumer (Phase 2 v2 C1 prerequisite for `adr-srv`).
//!   2. `lib_api_modules_resolve` — compile-time probe that every
//!      item in the Q2 public-API set resolves under its re-exported
//!      path at the crate root. If any of these stop resolving, the
//!      lib API contract has regressed.
//!
//! Q2 minimum set — see oracle bd adr-fmt-d7ao:
//!   - `Config`, `LoadError`, `load_quiet`, `resolve_corpus_root`
//!   - `ContainmentError`, `contained_join`, `contained_join_optional`
//!   - `AdrId`, `AdrRecord`, `DomainDir`, `Tier`, `parse_adr_id`
//!   - `ParseOutcome`, `parse_domain`, `parse_stale`
//!   - `Diagnostic`, `Severity`
//!
//! Modules `context`, `nav`, `output`, `refs`, `rules`, `guidelines`
//! are private per CHE-0030 R1; their probes were dropped in commit
//! "trim adr-fmt lib surface to CHE-0030 Q2 set" because external
//! consumers MUST NOT name those paths.
//!
//! Binary regression coverage lives in `tests/integration.rs` (~84
//! tests). These tests exist solely to pin the lib API contract.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

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

#[test]
fn lib_api_modules_resolve() {
    // Compile-time probes for the Q2 set. Each binding names a path
    // re-exported at the crate root via `pub use`; if any path
    // shifts or is removed, this test fails to compile.

    // report — Severity, Diagnostic
    let _: adr_fmt::Severity = adr_fmt::Severity::Warning;
    let _: adr_fmt::Diagnostic =
        adr_fmt::Diagnostic::warning("T999", Path::new("probe.md"), 1, String::from("probe"));

    // model — Tier, DomainDir, AdrId, AdrRecord (via parse_adr_id),
    // parse_adr_id
    let _: adr_fmt::Tier = adr_fmt::Tier::A;
    let _: adr_fmt::DomainDir = adr_fmt::DomainDir {
        path: PathBuf::from("/tmp/probe"),
        prefix: String::from("PRB"),
        name: String::from("probe"),
    };
    let _: Option<adr_fmt::AdrId> = adr_fmt::parse_adr_id("PRB-0001");
    // AdrRecord named via fn-pointer signature below.

    // containment — ContainmentError, contained_join,
    // contained_join_optional
    let _: Result<PathBuf, adr_fmt::ContainmentError> =
        adr_fmt::contained_join(Path::new("/tmp"), "x");
    let _: Result<Option<PathBuf>, adr_fmt::ContainmentError> =
        adr_fmt::contained_join_optional(Path::new("/tmp"), "x");

    // parser — parse_domain, parse_stale, ParseOutcome (fn-pointer
    // probes; constructing a full ParseOutcome requires fs access).
    let parse_domain_fn: fn(&adr_fmt::DomainDir) -> Result<adr_fmt::ParseOutcome, String> =
        adr_fmt::parse_domain;
    assert!(std::ptr::fn_addr_eq(
        parse_domain_fn,
        adr_fmt::parse_domain as fn(_) -> _
    ));
    let parse_stale_fn: fn(&Path, &adr_fmt::Config) -> Result<adr_fmt::ParseOutcome, String> =
        adr_fmt::parse_stale;
    assert!(std::ptr::fn_addr_eq(
        parse_stale_fn,
        adr_fmt::parse_stale as fn(_, _) -> _,
    ));

    // config — Config, LoadError, load_quiet, resolve_corpus_root
    // (fn-pointer probes name AdrRecord-free signatures from
    // config.rs). `Config` and `LoadError` are named via the
    // load_quiet signature below.
    let load_quiet_fn: fn(&Path) -> Result<adr_fmt::Config, adr_fmt::LoadError> =
        adr_fmt::load_quiet;
    assert!(std::ptr::fn_addr_eq(
        load_quiet_fn,
        adr_fmt::load_quiet as fn(_) -> _,
    ));
    // resolve_corpus_root takes a `CorpusConfig` which is private
    // (sub-struct of Config); name it through a fn-pointer-coerced
    // assertion only via `&Config`-projecting code paths in real
    // consumers. For a probe, the path-resolution alone is enough:
    let _ = adr_fmt::resolve_corpus_root;

    // AdrRecord — named explicitly to pin the type's public path.
    // Real consumers see `Vec<AdrRecord>` from parsers; here a
    // type-position probe suffices.
    let _: fn() -> Vec<adr_fmt::AdrRecord> = || Vec::new();
}
