#![forbid(unsafe_code)]

//! adr-srv — service crate scraping the ADR corpus via the adr-fmt
//! library (AFM-0026:R1 surface) and projecting records into pardosa-
//! genome event envelopes via the `ApplicationService` pattern
//! (CHE-0054:R8/R10 carve-out: no agent / gateway / `App<...>`).
//!
//! See AFM-0027 for the adr-fmt ↔ adr-srv boundary and CHE-0054:R8/R10
//! for the bespoke-service rationale.
//!
//! ## Module layout (CHE-0030 + AFM-0026:R1)
//!
//! Internal modules are private; the public surface is the explicit
//! `pub use` block below. CHE-0030:R1 binds cherry-domain crates to
//! private `mod` + `pub use` re-exports so internal reorganisation
//! is non-breaking.
//!
//! ## M1.2 skeleton
//!
//! This mission lands the wire-byte-stable substrate for M1.3 (scrape
//! pipeline) and M1.4 (GraphQL schema). The `AdrIngested` payload
//! shape is frozen here; subsequent missions add behaviour, not
//! payload reshape (CHE-0022:R5 additive evolution only).

mod app;
mod domain;
mod graphql;
mod projection;
pub mod scrape;

use std::path::{Path, PathBuf};

// ── AFM-0026:R1 surface re-exports (preserved from M1.1) ──────────
pub use adr_fmt::{AdrRecord, Config, LoadError, load_quiet, resolve_corpus_root};

// ── adr-srv domain surface ────────────────────────────────────────
pub use domain::{
    AdrDate, AdrDateError, AdrDocument, AdrFrontmatter, AdrId, AdrIdError, AdrIngested, BodyHash,
    KNOWN_DOMAINS, Status, Tier,
};

// ── ApplicationService surface (CHE-0054:R8/R10) ──────────────────
pub use app::{AdrService, AppState, IngestOutcome};

// ── Read-side projection (CHE-0048:R5/R6) ─────────────────────────
pub use projection::AdrCorpus;

// ── GraphQL Query schema (M1.4) ───────────────────────────────────
pub use graphql::{AdrGql, AdrRef, AdrSchema, Query, build_schema};

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
    // LoadError implements Display per AFM-0028:R1; format via `{e}`
    // rather than variant-matching its public-field shape.
    let config: Config = load_quiet(marker_dir).map_err(|e| e.to_string())?;
    let root: PathBuf = resolve_corpus_root(marker_dir, &config.corpus)
        .map_err(|e| format!("resolve_corpus_root failed: {e}"))?;
    Ok(root)
}
