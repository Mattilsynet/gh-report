//! Smoke + API probe tests pinning the lib surface.
//!
//! `run_default_mode_via_lib_api_returns_zero` proves `adr_fmt::run` is
//! callable from a library consumer. `lib_api_modules_resolve` is a
//! compile-time probe that every item in the Q2 public-API set (see bd
//! adr-fmt-d7ao) resolves under its re-exported crate-root path.
//!
//! Modules `context`, `nav`, `output`, `refs`, `rules`, `guidelines` are
//! private per CHE-0030 (Flat Public API via Private Modules); external
//! consumers MUST NOT name those paths, so no probes exist for them.
//!
//! Binary regression coverage lives in `tests/integration.rs`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[test]
fn run_default_mode_via_lib_api_returns_zero() {
    let argv: Vec<OsString> = vec![OsString::from("adr-fmt")];
    let exit = adr_fmt::run(argv);
    assert_eq!(exit, 0, "default-mode run should exit 0");
}

#[test]
fn lib_api_modules_resolve() {
    let _: adr_fmt::Severity = adr_fmt::Severity::Warning;
    let _: adr_fmt::Diagnostic =
        adr_fmt::Diagnostic::warning("T999", Path::new("probe.md"), 1, String::from("probe"));

    let _: adr_fmt::Tier = adr_fmt::Tier::A;
    let _: adr_fmt::DomainDir = adr_fmt::DomainDir {
        path: PathBuf::from("/tmp/probe"),
        prefix: String::from("PRB"),
        name: String::from("probe"),
    };
    let _: Option<adr_fmt::AdrId> = adr_fmt::parse_adr_id("PRB-0001");
    let _: adr_fmt::Status = adr_fmt::Status::Accepted;
    let _: adr_fmt::RelVerb = adr_fmt::RelVerb::References;
    let _: fn() -> Vec<adr_fmt::Relationship> = || Vec::new();

    let _: Result<PathBuf, adr_fmt::ContainmentError> =
        adr_fmt::contained_join(Path::new("/tmp"), "x");
    let _: Result<Option<PathBuf>, adr_fmt::ContainmentError> =
        adr_fmt::contained_join_optional(Path::new("/tmp"), "x");

    let parse_domain_fn: fn(
        &adr_fmt::DomainDir,
    ) -> Result<adr_fmt::ParseOutcome, adr_fmt::ParseError> = adr_fmt::parse_domain;
    assert!(std::ptr::fn_addr_eq(
        parse_domain_fn,
        adr_fmt::parse_domain as fn(_) -> _
    ));
    let parse_stale_fn: fn(
        &Path,
        &adr_fmt::Config,
    ) -> Result<adr_fmt::ParseOutcome, adr_fmt::ParseError> = adr_fmt::parse_stale;
    assert!(std::ptr::fn_addr_eq(
        parse_stale_fn,
        adr_fmt::parse_stale as fn(_, _) -> _,
    ));

    let load_quiet_fn: fn(&Path) -> Result<adr_fmt::Config, adr_fmt::LoadError> =
        adr_fmt::load_quiet;
    assert!(std::ptr::fn_addr_eq(
        load_quiet_fn,
        adr_fmt::load_quiet as fn(_) -> _,
    ));
    let _ = adr_fmt::resolve_corpus_root;

    let _: fn() -> Vec<adr_fmt::AdrRecord> = || Vec::new();
}
