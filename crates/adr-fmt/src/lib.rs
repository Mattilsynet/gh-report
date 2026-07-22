//! ADR template and link-integrity validator — library surface.
//!
//! Ships as both a binary (`adr-fmt`) and a library (`adr_fmt`); the
//! binary is a thin wrapper over [`run`] so downstream consumers
//! (e.g. `adr-srv`) can reuse parsing, linting, and navigation
//! without spawning a subprocess.
//!
//! # Modes
//!
//! ```text
//! adr-fmt                     # default: print governance guidelines
//! adr-fmt --lint              # lint all ADRs
//! adr-fmt --refs <ADR_ID>     # ADRs that cite the target
//! adr-fmt --context <CRATE>   # decision rules for a crate
//! adr-fmt --tree [DOMAIN]     # domain tree overview
//! ```
//!
//! Corpus discovery walks up from CWD for an `adr-fmt.toml` with a
//! valid `[corpus]` table; no CLI override (SSOT per AFM-0001).
//!
//! Exit codes: `0` — analysis complete (warnings only, or clean);
//! `1` — infrastructure error or lint errors detected.
//!
//! CLI surface frozen for v0.1 per AFM-0001. Library API follows
//! AFM-0026 / CHE-0030: modules private, minimum re-export set for
//! `adr-srv` via a flat `pub use` block (oracle summary bd
//! `adr-fmt-d7ao`).

#![forbid(unsafe_code)]

mod config;
mod containment;
mod context;
mod guidelines;
mod model;
mod nav;
mod output;
mod parser;
mod refs;
mod report;
mod rules;

pub use config::{Config, LoadError, ResolveCorpusError, load_quiet, resolve_corpus_root};
pub use containment::{ContainmentError, contained_join, contained_join_optional};
pub use model::{AdrId, AdrRecord, DomainDir, RelVerb, Relationship, Status, Tier, parse_adr_id};
pub use parser::{ParseError, ParseOutcome, parse_domain, parse_stale};
pub use report::{Diagnostic, Severity};

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::Parser;

struct CorpusScan {
    records: Vec<model::AdrRecord>,
    diagnostics: Vec<report::Diagnostic>,
}

/// ADR template and link-integrity validator.
#[derive(Parser)]
#[command(name = "adr-fmt", version)]
struct Cli {
    /// Lint all ADRs, report diagnostics to stdout
    #[arg(long, group = "mode")]
    lint: bool,

    /// List ADRs that cite the target via References or Supersedes
    #[arg(long, value_name = "ADR_ID", group = "mode")]
    refs: Option<String>,

    /// Show decision rules applicable to a crate
    #[arg(long, value_name = "CRATE", group = "mode")]
    context: Option<String>,

    /// Print domain tree (optionally filtered by domain prefix)
    #[arg(long, value_name = "DOMAIN", num_args = 0..=1, default_missing_value = "", group = "mode")]
    tree: Option<String>,
}

/// Library entry-point: parse `args` as the CLI, dispatch, return exit code.
///
/// The binary [`main`] is a thin wrapper around this function. Future
/// library consumers (e.g. `adr-srv`) call lower-level modules directly
/// (`parser`, `rules`, `nav`); `run` exists primarily to keep the binary
/// surface a one-liner and to provide a top-level smoke-testable entry.
///
/// Errors are reported by writing to stderr and returning a non-zero
/// exit code; this preserves AFM-0001 CLI behaviour bit-for-bit.
/// `--help` and `--version` are handled inside `Cli::parse_from`, which
/// calls `process::exit` itself per clap's contract.
///
/// # Panics
///
/// Panics only through clap's built-in `--help`/`--version` termination
/// path, which exits the process before returning to the caller.
pub fn run<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = Cli::parse_from(args);

    let marker = match discover_marker() {
        Ok(opt) => opt,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 1;
        }
    };

    let is_non_default_mode =
        cli.lint || cli.refs.is_some() || cli.context.is_some() || cli.tree.is_some();

    if !is_non_default_mode {
        return run_default_mode(marker);
    }

    let Some((marker_dir, config)) = marker else {
        eprintln!(
            "error: no adr-fmt.toml with a valid [corpus] table found in any parent directory"
        );
        eprintln!("       run from the workspace root, or create adr-fmt.toml there");
        return 1;
    };

    config::emit_legacy_rule_warnings(&config);

    let adr_root = match config::resolve_corpus_root(&marker_dir, &config.corpus) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    let domain_dirs = match discover_domains(&adr_root, &config) {
        Ok(dirs) => dirs,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    if domain_dirs.is_empty() {
        eprintln!(
            "error: no domain directories found in {}",
            adr_root.display()
        );
        return 1;
    }

    let CorpusScan {
        records: all_records,
        diagnostics: parse_diagnostics,
    } = match scan_corpus(&adr_root, &config, &domain_dirs) {
        Ok(scan) => scan,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    if let Some(ref adr_id_str) = cli.refs {
        let Some(target_id) = parse_adr_id(adr_id_str) else {
            eprintln!(
                "error: {} is not a valid ADR ID (expected PREFIX-NNNN)",
                adr_id_str.escape_debug()
            );
            return 1;
        };
        let report = match refs::find_refs(&target_id, &all_records) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        };
        print!("{}", output::render_refs(&report));
    } else if let Some(ref crate_name) = cli.context {
        let groups = match context::context_grouped(crate_name, &all_records, &config) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        };
        print!("{}", output::render_root_groups(crate_name, &groups));
    } else if let Some(ref domain_filter) = cli.tree {
        let filter = if domain_filter.is_empty() {
            None
        } else {
            Some(domain_filter.as_str())
        };
        print!(
            "{}",
            output::render_tree(&all_records, &domain_dirs, &config, filter)
        );
    } else if cli.lint {
        let mut diagnostics = parse_diagnostics;
        diagnostics.extend(rules::run_all(&all_records, &config));
        print!(
            "{}",
            output::render_diagnostics(&diagnostics, all_records.len())
        );
    }

    0
}

fn run_default_mode(marker: Option<(PathBuf, Config)>) -> i32 {
    if let Some((marker_dir, config)) = marker {
        match config::resolve_corpus_root(&marker_dir, &config.corpus) {
            Ok(_) => guidelines::print_governance(&config),
            Err(_) => guidelines::print_setup_guide(),
        }
    } else {
        guidelines::print_setup_guide();
    }
    0
}

fn scan_corpus(
    adr_root: &Path,
    config: &Config,
    domain_dirs: &[DomainDir],
) -> Result<CorpusScan, String> {
    let mut records = Vec::new();
    let mut diagnostics = Vec::new();

    for dir in domain_dirs {
        let outcome = parser::parse_domain(dir).map_err(|e| e.to_string())?;
        records.extend(outcome.records);
        diagnostics.extend(outcome.diagnostics);
    }

    let stale_dir = containment::contained_join_optional(adr_root, &config.stale.directory)
        .map_err(|e| format!("stale directory in adr-fmt.toml: {e}"))?;
    if let Some(stale_dir) = stale_dir
        && stale_dir.is_dir()
    {
        let outcome = parser::parse_stale(&stale_dir, config).map_err(|e| e.to_string())?;
        records.extend(outcome.records);
        diagnostics.extend(outcome.diagnostics);
    }

    Ok(CorpusScan {
        records,
        diagnostics,
    })
}

/// Walk up from CWD for `adr-fmt.toml` with a structurally valid
/// `[corpus]` table. See [`try_marker`] for per-marker validation.
///
/// A malformed TOML, missing `[corpus]` table, or containment
/// violation is skipped with a `note:` to stderr; walk-up continues
/// so one stray marker cannot mask a valid parent. An unreadable
/// existing `adr-fmt.toml` is a hard error (`Err(msg)`) — skipping
/// it would defeat the SSOT intent.
///
/// CWD is canonicalized once before the loop (handles symlinked
/// CWDs, e.g. macOS `/var` → `/private/var`); the returned marker
/// directory is also canonical.
///
/// `Ok(None)` if no valid marker is found, or `getcwd` fails.
/// Callers at the binary edge map `Err` to `eprintln! + return 1`
/// (lift per oracle bd `adr-fmt-d7ao` T2; AFM-0001:R1 governs the
/// binary's contract, not the library).
fn discover_marker() -> Result<Option<(PathBuf, Config)>, String> {
    let Ok(cwd) = std::env::current_dir() else {
        return Ok(None);
    };
    let canon_cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
    let mut dir = canon_cwd.as_path();
    loop {
        let candidate = dir.join("adr-fmt.toml");
        if candidate.is_file() {
            match try_marker(dir) {
                Ok(Some(pair)) => return Ok(Some(pair)),
                Ok(None) => {
                    eprintln!(
                        "note: skipping {}: marker is structurally invalid (no [corpus] table, \
                         missing corpus dir, no existing domain, or containment violation)",
                        candidate.display()
                    );
                }
                Err(TryMarkerError::Parse(msg)) => {
                    eprintln!("note: skipping {}: {msg}", candidate.display());
                }
                Err(TryMarkerError::Io(msg)) => {
                    return Err(msg);
                }
            }
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return Ok(None),
        }
    }
}

/// Internal error from [`try_marker`]: distinguishes parse failures
/// (skip-with-note in walk-up) from IO failures (hard error).
enum TryMarkerError {
    Parse(String),
    Io(String),
}

/// Load `marker_dir`'s `adr-fmt.toml`; validate the corpus root
/// has at least one configured domain.
///
/// `Ok(Some)` on full validity; `Ok(None)` if well-formed but unfit
/// (no `[corpus]` table, missing corpus root, or no domain resolves
/// or intentionally violates containment — see **Marker-claim
/// rule**). `Err(Parse)` on TOML parse failure (caller notes and
/// continues); `Err(Io)` if unreadable, including a TOCTOU race
/// where the file vanishes between the caller's check and this read.
///
/// **Marker-claim rule.** Claimed when the corpus root exists and
/// a domain resolves to an existing directory, or raises a
/// containment violation (surfaced downstream per AFM-0003:R1). A
/// stray marker whose root exists and whose only domain violates
/// containment can mask a
/// valid parent — mitigated by the corpus-root-must-exist precheck.
/// Pinned by
/// `stray_marker_with_violating_domain_masks_parent`.
fn try_marker(marker_dir: &Path) -> Result<Option<(PathBuf, Config)>, TryMarkerError> {
    let config = config::load_quiet(marker_dir).map_err(|e| match e {
        config::LoadError::Io(m) => TryMarkerError::Io(m),
        config::LoadError::Parse(m) => TryMarkerError::Parse(m),
    })?;
    let Ok(corpus_root) = config::resolve_corpus_root(marker_dir, &config.corpus) else {
        return Ok(None);
    };
    if !corpus_root.is_dir() {
        return Ok(None);
    }
    let any_domain_intended = config.domains.iter().any(|d| {
        match containment::contained_join_optional(&corpus_root, &d.directory) {
            Err(_) => true,
            Ok(Some(p)) => p.is_dir(),
            Ok(None) => false,
        }
    });
    if !any_domain_intended {
        return Ok(None);
    }
    let canon_marker = std::fs::canonicalize(marker_dir).map_err(|e| {
        TryMarkerError::Io(format!("cannot canonicalize {}: {e}", marker_dir.display()))
    })?;
    Ok(Some((canon_marker, config)))
}

/// Build domain directories from config, applying strict containment.
///
/// Each `domain.directory` from `adr-fmt.toml` is joined to `root`
/// via [`containment::contained_join_optional`]: absolute paths and
/// `..` components are rejected, and the canonical target must be a
/// descendant of the canonical ADR root. Containment failures abort
/// the run as infrastructure errors per AFM-0003 R1.
///
/// A configured directory that does not exist on disk is silently
/// skipped (returns `None` from the optional join); the caller emits
/// a diagnostic when zero domains resolve.
fn discover_domains(root: &Path, config: &Config) -> Result<Vec<DomainDir>, String> {
    let mut dirs = Vec::new();
    for domain in &config.domains {
        let resolved = containment::contained_join_optional(root, &domain.directory)
            .map_err(|e| format!("domain '{}' directory: {e}", domain.prefix))?;
        if let Some(path) = resolved
            && path.is_dir()
        {
            dirs.push(DomainDir {
                path,
                prefix: domain.prefix.clone(),
                name: domain.name.clone(),
            });
        }
    }
    Ok(dirs)
}
