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

pub use adr_fmt::{
    AdrRecord, Config, LoadError, ResolveCorpusError, load_quiet, resolve_corpus_root,
};

pub use domain::{
    AdrDate, AdrDateError, AdrDocument, AdrFrontmatter, AdrId, AdrIdError, AdrIngested, BodyHash,
    KNOWN_DOMAINS, Status, Tier,
};

pub use app::{AdrService, AppState, IngestOutcome};

pub use projection::AdrCorpus;

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
/// Returns [`SurfaceProbeError::Load`] if `adr-fmt.toml` cannot be
/// loaded, or [`SurfaceProbeError::Resolve`] if the corpus root cannot
/// be resolved from the marker.
pub fn surface_probe(marker_dir: &Path) -> Result<PathBuf, SurfaceProbeError> {
    let config: Config = load_quiet(marker_dir)?;
    let root: PathBuf = resolve_corpus_root(marker_dir, &config.corpus)?;
    Ok(root)
}

/// Failure from [`surface_probe`]: either the config load or the
/// corpus-root resolve step of the AFM-0026:R1 surface probe failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum SurfaceProbeError {
    Load(LoadError),
    Resolve(ResolveCorpusError),
}

impl core::fmt::Display for SurfaceProbeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Load(e) => write!(f, "{e}"),
            Self::Resolve(e) => write!(f, "resolve_corpus_root failed: {e}"),
        }
    }
}

impl std::error::Error for SurfaceProbeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Load(e) => Some(e),
            Self::Resolve(e) => Some(e),
        }
    }
}

impl From<LoadError> for SurfaceProbeError {
    fn from(e: LoadError) -> Self {
        Self::Load(e)
    }
}

impl From<ResolveCorpusError> for SurfaceProbeError {
    fn from(e: ResolveCorpusError) -> Self {
        Self::Resolve(e)
    }
}
