//! ADR template and link-integrity validator — library surface.
//!
//! This crate ships as both a binary (`adr-fmt`) and a library (`adr_fmt`).
//! The binary is a thin wrapper over [`run`] so future consumers
//! (e.g. `adr-srv`, Phase 2 v2 C1) can re-use the parsing, linting,
//! and navigation surface without spawning a subprocess.
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
//! The corpus location is discovered by walking up from the current
//! working directory until an `adr-fmt.toml` with a valid `[corpus]`
//! table is found. There is no CLI override — discovery is the SSOT
//! per AFM-0001.
//!
//! Exit codes:
//!   0 — Analysis complete (warnings only, or clean)
//!   1 — Infrastructure error or lint errors detected
//!
//! The CLI surface is frozen for v0.1 per AFM-0001. The library API
//! is exposed via `pub use` / `pub mod` re-exports per CHE-0030.

#![forbid(unsafe_code)]
// Track 3.1: pub-mod widening exposed pedantic-doc lints on pub fns
// in `parser.rs`, which AGENTS.md flags as "do not refactor during
// v0.1" (AFM-0006 regex parser is large + stable). Suppress these
// two pedantic lints crate-wide rather than touching parser.rs.
// Non-parser pub fns have `# Errors` docs added in-place; this
// allow exists for parser's three pub fns (`parse_domain`,
// `parse_stale`, `parse_adr_file`). Revisit when parser.rs is
// next opened for behavioural work.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    reason = "Track 3.1 lib-API widening exposed pedantic doc lints in parser.rs; AGENTS.md forbids parser.rs refactor during v0.1 per AFM-0006"
)]

// Public lib API (Track 3.1): re-export the modules that future
// consumers (e.g. `adr-srv`, Phase 2 v2 C1) need to drive ADR
// parsing, linting, navigation, and rendering without spawning
// the binary. `output` is re-exported because `context_grouped`
// returns `Vec<output::RootGroup>`; keeping `output` private
// would make the `context` return type unnameable externally.
// `guidelines` stays private — it is purely a stdout-rendering
// helper for the binary's default mode (AFM-0001).
pub mod config;
pub mod containment;
pub mod context;
pub mod model;
pub mod nav;
pub mod output;
pub mod parser;
pub mod refs;
pub mod report;
pub mod rules;

mod guidelines;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::Parser;

use config::Config;
use model::{DomainDir, parse_adr_id};

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
// CLI dispatch shape; splitting would scatter mode-selection logic
// without simplifying any branch.
#[expect(
    clippy::too_many_lines,
    reason = "CLI mode dispatch; each arm is a small linear sequence and splitting would scatter the mode-selection logic without simplifying any branch"
)]
pub fn run<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = Cli::parse_from(args);

    // Discover marker directory by walking up from CWD looking for an
    // `adr-fmt.toml` with a valid `[corpus]` table whose root resolves
    // to a directory containing at least one configured domain.
    let marker = discover_marker();

    // Default mode: guidelines
    let is_non_default_mode =
        cli.lint || cli.refs.is_some() || cli.context.is_some() || cli.tree.is_some();

    if !is_non_default_mode {
        // Guidelines mode — handles both setup and governance display
        if let Some((marker_dir, config)) = marker {
            // Resolve corpus root for the per-corpus governance display.
            // If unresolvable, fall back to setup guide rather than abort.
            match config::resolve_corpus_root(&marker_dir, &config.corpus) {
                Ok(_) => guidelines::print_governance(&config),
                Err(_) => guidelines::print_setup_guide(),
            }
        } else {
            guidelines::print_setup_guide();
        }
        return 0;
    }

    // Non-default modes require a discovered marker + valid corpus.
    let Some((marker_dir, config)) = marker else {
        eprintln!(
            "error: no adr-fmt.toml with a valid [corpus] table found in any parent directory"
        );
        eprintln!("       run from the workspace root, or create adr-fmt.toml there");
        return 1;
    };

    // Walk-up discovery uses `load_quiet` to suppress noise from skipped
    // markers; fire the legacy-rule deprecation warning once here, against
    // the selected marker only. Serves AFM-0003: the advisory channel must
    // remain credible — config users with legacy `[[rules]]` declarations
    // need to see exactly one deprecation note per run, not zero.
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

    let mut all_records = Vec::new();
    let mut parse_diagnostics = Vec::new();
    for dir in &domain_dirs {
        match parser::parse_domain(dir) {
            Ok(outcome) => {
                all_records.extend(outcome.records);
                parse_diagnostics.extend(outcome.diagnostics);
            }
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        }
    }

    // Parse stale directory (optional — may not exist in fresh repos)
    let stale_dir = match containment::contained_join_optional(&adr_root, &config.stale.directory) {
        Ok(opt) => opt,
        Err(e) => {
            eprintln!("error: stale directory in adr-fmt.toml: {e}");
            return 1;
        }
    };
    if let Some(stale_dir) = stale_dir
        && stale_dir.is_dir()
    {
        match parser::parse_stale(&stale_dir, &config) {
            Ok(outcome) => {
                all_records.extend(outcome.records);
                parse_diagnostics.extend(outcome.diagnostics);
            }
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        }
    }

    // Mode dispatch
    if let Some(ref adr_id_str) = cli.refs {
        // --refs mode
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
        // --context mode
        let groups = match context::context_grouped(crate_name, &all_records, &config) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        };
        print!("{}", output::render_root_groups(crate_name, &groups));
    } else if let Some(ref domain_filter) = cli.tree {
        // --tree mode
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
        // --lint mode: advisory-only per AFM-0003 R1/R2. All rule findings
        // are warnings; exit 0 always when lint completes. Exit 1 is reserved
        // for infrastructure errors (missing config, unreadable files,
        // invalid configuration) handled earlier in this function via
        // eprintln! + return 1.
        //
        // Parser-stage diagnostics (P### per AFM-0017) are merged with
        // rule-stage diagnostics so the user sees one unified list.
        let mut diagnostics = parse_diagnostics;
        diagnostics.extend(rules::run_all(&all_records, &config));
        print!(
            "{}",
            output::render_diagnostics(&diagnostics, all_records.len())
        );
    }

    0
}

/// Walk up from CWD looking for `adr-fmt.toml` with a structurally
/// valid `[corpus]` table.
///
/// Termination: returns the first ancestor directory whose
/// `adr-fmt.toml` parses, has `[corpus] root = "..."`, the resolved
/// corpus root exists as a directory, and at least one configured
/// domain directory either resolves cleanly to an existing path
/// (containment-clean and on disk).
///
/// **Skipping:** an ancestor with a malformed TOML, a missing
/// `[corpus]` table, no existing configured domain, or any
/// containment violation in `[corpus] root` / `[[domains]].directory`
/// is treated as a stray and skipped with a single-line `note:` to
/// stderr; walk-up continues so a stray cannot mask a valid parent.
///
/// **Hard errors during walk-up** (NOT skipped): an `adr-fmt.toml`
/// that exists but cannot be read (permission denied, IO error)
/// aborts discovery and surfaces the error, since silently skipping
/// a marker the user clearly intended would defeat the SSOT.
///
/// **CWD canonicalization:** the starting CWD is canonicalized once
/// before the loop so a CWD reached via symlinks (e.g. macOS
/// `/var → /private/var`) walks up through the resolved path. The
/// returned marker directory is also canonical.
///
/// **Platform notes:** walk-up uses `Path::parent()`, inheriting
/// Rust's path semantics. On Unix this terminates cleanly at `/`.
/// On Windows, `parent()` of a UNC root or verbatim prefix returns
/// `None`, also terminating. Symlinked CWDs and symlinked marker
/// files are accepted (file resolution follows symlinks).
///
/// Returns `None` if no valid marker is found before reaching the
/// filesystem root, or if `getcwd` fails. An unreadable marker is
/// reported to stderr and the process is exited with code 1 (same
/// behaviour as pre-Track-3.1; library callers that want different
/// behaviour should call lower-level modules directly).
fn discover_marker() -> Option<(PathBuf, Config)> {
    let cwd = std::env::current_dir().ok()?;
    // Canonicalize once so symlinked CWDs walk up through the
    // resolved path, not the lexical (unresolved) path.
    let canon_cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
    let mut dir = canon_cwd.as_path();
    loop {
        let candidate = dir.join("adr-fmt.toml");
        if candidate.is_file() {
            match try_marker(dir) {
                Ok(Some(pair)) => return Some(pair),
                Ok(None) => {
                    // Structurally invalid (parsed but unfit). Walk
                    // past so a stray cannot mask a valid parent.
                    eprintln!(
                        "note: skipping {}: marker is structurally invalid (no [corpus] table, \
                         missing corpus dir, no existing domain, or containment violation)",
                        candidate.display()
                    );
                }
                Err(TryMarkerError::Parse(msg)) => {
                    // Parse failure: skip-with-note, keep walking.
                    eprintln!("note: skipping {}: {msg}", candidate.display());
                }
                Err(TryMarkerError::Io(msg)) => {
                    // Unreadable marker the user clearly created:
                    // hard error, do not silently mask. Preserves
                    // pre-Track-3.1 behaviour; library callers that
                    // want to recover should use the lower-level
                    // config:: module directly.
                    eprintln!("error: {msg}");
                    std::process::exit(1);
                }
            }
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return None,
        }
    }
}

/// Internal error from [`try_marker`]: distinguishes parse failures
/// (skip-with-note in walk-up) from IO failures (hard error).
enum TryMarkerError {
    Parse(String),
    Io(String),
}

/// Try to load a marker directory's `adr-fmt.toml` and validate that
/// the resolved corpus root contains at least one configured domain.
///
/// Returns `Ok(Some)` on full structural validity. Returns `Ok(None)`
/// if the config is well-formed but unfit (no `[corpus]` table,
/// corpus root missing on disk, or no configured domain that is
/// either present-on-disk or *intentionally* present-with-violation).
/// Returns `Err(Parse)` if the TOML itself fails to parse; the
/// caller emits a note and continues. Returns `Err(Io)` if the file
/// exists but cannot be read; the caller aborts.
///
/// **TOCTOU note:** the caller checks `candidate.is_file()` before
/// invoking `try_marker`, but the file may be unlinked or chmod'd
/// between that check and `read_to_string`. A vanished marker maps
/// to `Err(Io)` and aborts walk-up — defensible since the file did
/// exist at the discovery moment, and silent skipping would mask
/// the user's clear intent.
///
/// **Marker-claim rule.** A marker is *claimed* (selected by walk-up)
/// when its corpus root resolves to an existing directory AND at
/// least one configured domain either:
///   1. resolves cleanly to an existing directory on disk, OR
///   2. raises a containment violation (absolute path, `..`,
///      symlink escape).
///
/// Case (2) is deliberate: it surfaces the violation to the user
/// downstream as an infrastructure error per AFM-0003 R1 rather
/// than silently walking past. The trade-off is a known
/// **masking footgun**: a stray `adr-fmt.toml` deeper in the tree,
/// whose corpus root happens to exist *and* whose only domain has
/// a violating directory, will mask a valid parent marker. Pinned
/// by `stray_marker_with_violating_domain_masks_parent` in tests.
/// The footgun is mitigated by the corpus-root-must-exist precheck
/// — a fully bogus stray is skipped without claiming.
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
    // Claim the marker if any configured domain is either (a) present
    // on disk OR (b) violating containment (downstream surfaces the
    // error). Skip only when ALL domains are clean-but-absent.
    let any_domain_intended = config.domains.iter().any(|d| {
        match containment::contained_join_optional(&corpus_root, &d.directory) {
            Err(_) => true, // violation → user clearly intended this marker
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
