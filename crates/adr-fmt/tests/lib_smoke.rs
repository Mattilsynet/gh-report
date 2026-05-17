//! Smoke + API probe tests for the lib surface (Track 3.1).
//!
//! Two purposes:
//!   1. `run_default_mode_via_lib_api_returns_zero` — proves the
//!      top-level entry-point `adr_fmt::run` is callable from a
//!      library consumer (Phase 2 v2 C1 prerequisite for `adr-srv`).
//!   2. `lib_api_modules_resolve` — compile-time probe that each
//!      re-exported module path resolves and exposes the headline
//!      types future consumers will name.
//!
//! Binary regression coverage lives in `tests/integration.rs` (~84
//! tests). These tests exist solely to pin the lib API contract.

use std::ffi::OsString;
use std::path::PathBuf;

/// Type alias for `context_grouped`'s signature — kept here (not in
/// `adr_fmt::context`) because the probe's purpose is to name the
/// full external form from a library-consumer perspective, including
/// the `Result<Vec<…>>` layering. Mirrors what a real consumer would
/// `use` from the crate.
type ContextGroupedFn = fn(
    &str,
    &[adr_fmt::model::AdrRecord],
    &adr_fmt::config::Config,
) -> Result<Vec<adr_fmt::output::RootGroup>, String>;

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
    // Compile-time probes: name each re-exported module path and a
    // headline type from each. If any of these stop resolving, the
    // lib API contract has regressed.
    //
    // Variables are named without underscore prefix so each binding
    // is a deliberate use, not a no-op (silences
    // clippy::no_effect_underscore_binding under workspace pedantic).
    // `let _: T = expr` syntax is used for value probes; `let f: F = ...`
    // for fn-pointer probes (also exercises the type alias).

    // adr_fmt::report — Severity, Diagnostic
    let _: adr_fmt::report::Severity = adr_fmt::report::Severity::Warning;
    let _: adr_fmt::report::Diagnostic = adr_fmt::report::Diagnostic::warning(
        "T999",
        std::path::Path::new("probe.md"),
        1,
        String::from("probe"),
    );

    // adr_fmt::model — Tier, DomainDir, AdrId via parse_adr_id
    let _: adr_fmt::model::Tier = adr_fmt::model::Tier::A;
    let _: adr_fmt::model::DomainDir = adr_fmt::model::DomainDir {
        path: PathBuf::from("/tmp/probe"),
        prefix: String::from("PRB"),
        name: String::from("probe"),
    };
    let _: Option<adr_fmt::model::AdrId> = adr_fmt::model::parse_adr_id("PRB-0001");

    // adr_fmt::containment — ContainmentError, contained_join
    let _: Result<PathBuf, adr_fmt::containment::ContainmentError> =
        adr_fmt::containment::contained_join(std::path::Path::new("/tmp"), "x");

    // adr_fmt::nav — ChildEntry, compute_children
    let _: std::collections::HashMap<adr_fmt::model::AdrId, Vec<adr_fmt::nav::ChildEntry>> =
        adr_fmt::nav::compute_children(&[]);

    // adr_fmt::parser — parse_domain (fn-pointer probe; constructing
    // a full ParseOutcome requires fs access, out of scope for a probe).
    // The fn-pointer assignment alone enforces the signature at
    // compile time; calling through it isn't required.
    let parse_domain_fn: fn(
        &adr_fmt::model::DomainDir,
    ) -> Result<adr_fmt::parser::ParseOutcome, String> = adr_fmt::parser::parse_domain;
    // Use the binding so it isn't a dead binding under clippy.
    assert!(std::ptr::fn_addr_eq(
        parse_domain_fn,
        adr_fmt::parser::parse_domain as fn(_) -> _
    ));

    // adr_fmt::rules — run_all (fn-pointer probe).
    let rules_run_all_fn: fn(
        &[adr_fmt::model::AdrRecord],
        &adr_fmt::config::Config,
    ) -> Vec<adr_fmt::report::Diagnostic> = adr_fmt::rules::run_all;
    assert!(std::ptr::fn_addr_eq(
        rules_run_all_fn,
        adr_fmt::rules::run_all as fn(_, _) -> _,
    ));

    // adr_fmt::refs — find_refs (fn-pointer probe).
    let refs_find_fn: fn(
        &adr_fmt::model::AdrId,
        &[adr_fmt::model::AdrRecord],
    ) -> Result<adr_fmt::refs::RefsReport, String> = adr_fmt::refs::find_refs;
    assert!(std::ptr::fn_addr_eq(
        refs_find_fn,
        adr_fmt::refs::find_refs as fn(_, _) -> _,
    ));

    // adr_fmt::context — context_grouped (fn-pointer probe via alias).
    // The return type names `adr_fmt::output::RootGroup`, which is why
    // `output` is also re-exported (rendering helpers private would
    // make this signature unnameable externally).
    let context_grouped_fn: ContextGroupedFn = adr_fmt::context::context_grouped;
    assert!(std::ptr::fn_addr_eq(
        context_grouped_fn,
        adr_fmt::context::context_grouped as fn(_, _, _) -> _,
    ));
}
