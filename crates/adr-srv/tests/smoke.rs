//! Smoke test: exercise AFM-0026:R1 surface from an external consumer.
//!
//! These tests prove the AFM-0026:R1 re-exports (`load_quiet`,
//! `resolve_corpus_root`, `Config`, `LoadError`) are callable from a
//! downstream crate. No pardosa bridge, no service surface — skeleton
//! stage per AFM-0027.

use std::path::PathBuf;

use adr_srv::LoadError;

/// Workspace root resolved at compile time from this crate's manifest
/// dir (`crates/adr-srv` → `../..`). The workspace root is where
/// `adr-fmt.toml` lives.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// `LoadError` does not yet derive `Display`/`Debug` (AFM-0026:R1
/// surface gap noted at skeleton stage); pattern-match its variants to
/// produce a panic message without depending on those traits.
fn load_err_msg(e: LoadError) -> String {
    match e {
        LoadError::Io(msg) => format!("Io: {msg}"),
        LoadError::Parse(msg) => format!("Parse: {msg}"),
    }
}

#[test]
fn surface_probe_resolves_corpus_root() {
    let root = workspace_root();
    let resolved =
        adr_srv::surface_probe(&root).unwrap_or_else(|e| panic!("surface_probe failed: {e}"));
    assert!(
        resolved.exists(),
        "resolved corpus root must exist on disk: {}",
        resolved.display()
    );
}

#[test]
fn load_quiet_is_callable_from_external_crate() {
    let root = workspace_root();
    let config = adr_srv::load_quiet(&root).unwrap_or_else(|e| panic!("{}", load_err_msg(e)));
    assert!(
        !config.domains.is_empty(),
        "workspace adr-fmt.toml declares at least one domain"
    );
}

#[test]
fn resolve_corpus_root_is_callable_from_external_crate() {
    let root = workspace_root();
    let config = adr_srv::load_quiet(&root).unwrap_or_else(|e| panic!("{}", load_err_msg(e)));
    let resolved = adr_srv::resolve_corpus_root(&root, &config.corpus)
        .unwrap_or_else(|e| panic!("resolve_corpus_root failed: {e}"));
    assert!(
        resolved.exists(),
        "corpus root must exist: {}",
        resolved.display()
    );
}
