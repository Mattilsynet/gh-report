#![forbid(unsafe_code)]

//! adr-srv — service skeleton scraping the ADR corpus via the adr-fmt
//! library (AFM-0026:R1 surface). Skeleton stage: no pardosa bridge,
//! no service endpoints, no idempotency state.
//!
//! See AFM-0027 for the adr-fmt ↔ adr-srv boundary.

use std::path::{Path, PathBuf};

pub use adr_fmt::{AdrRecord, Config, LoadError, Tier, load_quiet, resolve_corpus_root};

/// Minimal probe that the AFM-0026:R1 surface compiles and is callable
/// from this crate. Loads the workspace `adr-fmt.toml` and resolves the
/// corpus root. Returns the resolved corpus path on success.
///
/// Used by the smoke test; not part of the eventual service API. The
/// `marker_dir` argument must contain (or be a walk-up ancestor of) an
/// `adr-fmt.toml`.
///
/// # Errors
/// Returns `Err` with a human-readable reason if either the config load
/// or corpus-root resolution fails. The error type is `String` because
/// the two underlying errors (`LoadError` and the resolve `String`) are
/// merged for caller convenience at the skeleton stage.
pub fn surface_probe(marker_dir: &Path) -> Result<PathBuf, String> {
    let config: Config = load_quiet(marker_dir).map_err(|e| match e {
        LoadError::Io(msg) => format!("load_quiet io error: {msg}"),
        LoadError::Parse(msg) => format!("load_quiet parse error: {msg}"),
    })?;
    let root: PathBuf = resolve_corpus_root(marker_dir, &config.corpus)
        .map_err(|e| format!("resolve_corpus_root failed: {e}"))?;
    Ok(root)
}
