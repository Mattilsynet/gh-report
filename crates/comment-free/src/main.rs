#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![warn(clippy::missing_const_for_fn)]
use clap::Parser;
use comment_free::{
    CommentFreeError, DOC_LINT_DOCTRINE_MSG, DocBudget, FileOutcome, GitState, ProcessOptions,
    SKIP_DIRS, doc_lint_file, git_state, process_file, rustfmt_available, scan_doc_files,
};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use walkdir::WalkDir;
#[derive(Parser, Debug)]
#[command(
    name = "comment-free",
    about = "Doc-comment linter for Rust crates (default). \
             Use --rewrite to reformat .rs files via prettyplease + rustfmt; non-doc `//` and `/* */` comments are removed as a side-effect of the AST round-trip. \
             Use --rewrite --rustdoc-link-idioms for a byte-preserving doc-only pass that normalises a small set of Rust intra-doc link idioms without touching any non-doc bytes.",
    long_about = "Default mode is a read-only doc-comment budget linter: walks ROOT for .rs files \
                  and reports doc comments whose prose word count exceeds --doc-max-words. \
                  Fenced code blocks (` ``` ` or `~~~`) are excluded from the count; the \
                  doctrine allows 0-3 such fenced examples per doc comment and they do not \
                  consume the word budget. Examples are detected mechanically by fence \
                  delimiters only — there is no semantic example detection. Each finding is \
                  followed by a DOC_LINT_MSG line carrying the project doctrine. Doc comments \
                  are NEVER deleted by this tool.\n\
                  \n\
                  Two rewrite modes exist:\n\
                  \n\
                  1) `--rewrite` (legacy full pipeline): reformats .rs files via `syn` -> \
                  `prettyplease` -> `rustfmt --edition <EDITION>`. Non-doc `//` and `/* */` \
                  comments are removed as a side-effect of the AST round-trip. Doc-comment \
                  CONTENT is preserved, but surface syntax (`///` vs `#[doc = \"...\"]`) and \
                  whitespace may normalise; the entire file is reformatted to rustfmt's \
                  canonical style. Requires a clean git working tree under ROOT unless \
                  --force-dirty is passed.\n\
                  \n\
                  2) `--rewrite --rustdoc-link-idioms` (safe subpath): byte-preserving \
                  doc-only rewrite. Mutates ONLY `///`, `//!`, `#[doc = \"...\"]`, \
                  `#![doc = \"...\"]`, and `#[cfg_attr(_, doc = \"...\")]` payload text via \
                  the rustdoc-link idiom normaliser; every other byte in the source is \
                  preserved verbatim. Does NOT run `prettyplease` or `rustfmt`, does NOT \
                  strip non-doc `//` and `/* */` comments, does NOT touch block doc \
                  comments (`/** */`). This is the safe-to-dogfood subpath.\n\
                  \n\
                  --dry-run is always safe in both modes.\n\
                  \n\
                  Exit codes:\n\
                    0  clean (no findings, no errors)\n\
                    1  catastrophic / unmapped IO error\n\
                    2  invalid CLI arguments (clap rejection)\n\
                    3  git state error in rewrite mode (dirty / not-a-repo / git missing)\n\
                    4  doc-lint findings observed (default mode)\n\
                    5  per-file parse/IO errors observed during processing (both modes)\n\
                  \n\
                  Output streams: findings (DOC_LINT, REWRITE, WOULD_REWRITE, diffs) on \
                  stdout; metadata (SUMMARY, DOC_WARN, errors) on stderr."
)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "clap-derived CLI surface: each bool maps to one orthogonal user-facing flag (--rewrite, --dry-run, --force-dirty, --rustdoc-link-idioms); collapsing to a state-machine enum would obscure that mapping and complicate `requires=` constraints"
)]
struct Options {
    #[arg(default_value = ".", value_name = "ROOT")]
    root: PathBuf,
    /// Reformat .rs files to rustfmt's canonical style (whitespace,
    /// line-wrap, attribute placement may change). Pipeline is
    /// `syn` -> `prettyplease` -> `rustfmt --edition <EDITION>`. Non-doc
    /// `//` and `/* */` comments are discarded as a side-effect of the
    /// AST round-trip. Doc-comment content (`///`, `//!`, `#[doc=...]`,
    /// `#[doc(...)]`, and `doc` payloads inside `cfg_attr`) is preserved,
    /// though surface syntax may normalise.
    ///
    /// When combined with `--rustdoc-link-idioms`, dispatches to the
    /// byte-preserving safe subpath instead — see that flag's docs.
    #[arg(long)]
    rewrite: bool,
    /// Preview the rewrite as a unified diff without modifying files.
    /// Only meaningful with `--rewrite`. Default (lint) mode is
    /// already read-only; `--dry-run` is meaningful only with
    /// `--rewrite` (enforced by clap).
    #[arg(long, short = 'n', requires = "rewrite")]
    dry_run: bool,
    /// Bypass the clean-tree check when rewriting.
    /// Only meaningful with `--rewrite`.
    #[arg(long, requires = "rewrite")]
    force_dirty: bool,
    /// Unified-diff context line count (used with `--dry-run`).
    /// Only meaningful with `--rewrite`.
    #[arg(long, default_value_t = 3, value_name = "N", requires = "rewrite")]
    context: usize,
    /// Word budget for doc-comment prose. Fenced code blocks (` ``` `
    /// or `~~~`) are excluded from the count; the doctrine allows 0-3
    /// such fenced examples per doc comment and they do not consume
    /// the budget.
    #[arg(long, default_value_t = 80, value_name = "N")]
    doc_max_words: usize,
    /// Rust edition passed to `rustfmt --edition` during the post-process
    /// step. Default `2024` matches this workspace; override when
    /// rewriting code on an older edition. Has no effect when
    /// `--rustdoc-link-idioms` is also passed (the safe subpath does not
    /// invoke rustfmt).
    #[arg(
        long,
        default_value = "2024",
        value_name = "EDITION",
        requires = "rewrite"
    )]
    edition: String,
    /// Opt-in pass that rewrites mechanically-safe Rust intra-doc
    /// link idioms inside doc comments and doc attributes. Requires
    /// `--rewrite`.
    ///
    /// Dispatches to a BYTE-PRESERVING SAFE SUBPATH: only `///`,
    /// `//!`, `#[doc = "..."]`, `#![doc = "..."]`, and
    /// `#[cfg_attr(_, doc = "...")]` payloads are mutated.
    /// `prettyplease` and `rustfmt` are not run, non-doc comments are
    /// not stripped, block doc comments (`/** */`) are not touched.
    /// Line-count-preserving and idempotent.
    ///
    /// Examples (left -> right):
    ///
    /// ```text
    ///   [Type](Type)              -> [`Type`]
    ///   [foo::Bar](foo::Bar)      -> [`foo::Bar`]
    ///   [Type]                    -> [`Type`] (when code-ish)
    ///   [begin](Self::begin)      -> [`begin`](Self::begin)
    /// ```
    ///
    /// Skipped: URL targets, reference-style links, prose labels,
    /// targets with generics/disambiguators, fenced or inline code.
    #[arg(long, requires = "rewrite")]
    rustdoc_link_idioms: bool,
}
fn main() -> ExitCode {
    let opts = Options::parse();
    match run(&opts) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(5),
        Err(e) => {
            match &e {
                CommentFreeError::NotADirectory => {
                    eprintln!("error: {} is not a directory", opts.root.display());
                }
                CommentFreeError::GitDirty(summary) => {
                    eprintln!(
                        "error: refusing to rewrite files: git working tree is dirty\n\
                         {summary}\n\
                         pass --force-dirty to override, or --dry-run to preview without writing"
                    );
                }
                CommentFreeError::GitNotARepo => {
                    eprintln!(
                        "error: refusing to rewrite files: {} is not inside a git repository\n\
                         pass --force-dirty to override, or --dry-run to preview without writing",
                        opts.root.display()
                    );
                }
                CommentFreeError::GitUnavailable(msg) => {
                    eprintln!(
                        "error: refusing to rewrite files: could not query git state: {msg}\n\
                         pass --force-dirty to override, or --dry-run to preview without writing"
                    );
                }
                CommentFreeError::RustfmtUnavailable(msg) => {
                    eprintln!(
                        "error: refusing to rewrite files: {msg}\n\
                         install rustfmt (`rustup component add rustfmt`) or check that \
                         --edition is a value your rustfmt accepts"
                    );
                }
                other => {
                    eprintln!("error: {other}");
                }
            }
            ExitCode::from(&e)
        }
    }
}
fn run(opts: &Options) -> Result<u32, CommentFreeError> {
    if !opts.root.is_dir() {
        return Err(CommentFreeError::NotADirectory);
    }
    if opts.rewrite {
        run_strip(opts)
    } else {
        run_lint(opts)
    }
}
/// Allowlisted source-tree directory names. `comment-free` is a Rust-only
/// tool; only `.rs` files under one of these names anywhere in the path
/// are eligible for traversal.
const ALLOWED_ROOT_DIRS: &[&str] = &["crates", "src"];
/// Resolve `root` to the concrete directories `walk_rs_files` should descend.
///
/// If `root` itself sits inside (or is named) an allowlisted source dir, it
/// is returned verbatim — the caller already targeted a Rust subtree.
/// Otherwise `root` is treated as a project/workspace top: its immediate
/// `crates/` and `src/` children (whichever exist) are returned. An empty
/// result is valid and means "nothing to process".
fn resolve_walk_roots(root: &Path) -> Vec<PathBuf> {
    let in_scope = root
        .components()
        .any(|c| matches!(c.as_os_str().to_str(), Some(n) if ALLOWED_ROOT_DIRS.contains(& n)));
    if in_scope {
        return vec![root.to_path_buf()];
    }
    ALLOWED_ROOT_DIRS
        .iter()
        .map(|d| root.join(d))
        .filter(|p| p.is_dir())
        .collect()
}
/// Iterate every `.rs` file under `root`, ignoring traversal errors.
///
/// Restricts traversal to `.rs` under allowlisted Rust source roots
/// (`crates/`, `src/`) — `comment-free` is a Rust-only tool. Within those
/// roots, `SKIP_DIRS` (notably nested `target/`) still prune build output.
fn walk_rs_files(root: &Path) -> impl Iterator<Item = PathBuf> + use<'_> {
    resolve_walk_roots(root).into_iter().flat_map(|base| {
        WalkDir::new(base)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                if e.depth() == 0 {
                    return true;
                }
                let name = e.file_name().to_string_lossy();
                if e.file_type().is_dir()
                    && (name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()))
                {
                    return false;
                }
                true
            })
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .map(walkdir::DirEntry::into_path)
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("rs"))
    })
}
fn run_strip(opts: &Options) -> Result<u32, CommentFreeError> {
    rustfmt_available(&opts.edition).map_err(CommentFreeError::RustfmtUnavailable)?;
    if !opts.dry_run && !opts.force_dirty {
        match git_state(&opts.root) {
            GitState::Clean => {}
            GitState::Dirty(summary) => return Err(CommentFreeError::GitDirty(summary)),
            GitState::NotARepo => return Err(CommentFreeError::GitNotARepo),
            GitState::GitUnavailable(msg) => {
                return Err(CommentFreeError::GitUnavailable(msg));
            }
        }
    }
    let doc_warnings = scan_doc_files(&opts.root);
    for path in &doc_warnings {
        eprintln!("DOC_WARN\t{}", path.display());
    }
    if !doc_warnings.is_empty() {
        eprintln!(
            "warning: {} documentation file(s) found under {}; they will NOT be modified",
            doc_warnings.len(),
            opts.root.display()
        );
    }
    let process_opts = ProcessOptions {
        dry_run: opts.dry_run,
        context: opts.context,
        edition: opts.edition.clone(),
        rustdoc_link_idioms: opts.rustdoc_link_idioms,
    };
    let mut rewritten = 0u32;
    let mut unchanged = 0u32;
    let mut errors = 0u32;
    for path in walk_rs_files(&opts.root) {
        match process_file(&path, &process_opts) {
            FileOutcome::Rewritten { diff } => {
                rewritten += 1;
                if opts.dry_run {
                    println!("WOULD_REWRITE\t{}", path.display());
                    if let Some(d) = diff {
                        print!("{d}");
                    }
                } else {
                    println!("REWRITE\t{}", path.display());
                }
            }
            FileOutcome::Unchanged => {
                unchanged += 1;
            }
            FileOutcome::ParseError(msg) => {
                errors += 1;
                eprintln!("PARSE_ERROR\t{}\t{}", path.display(), msg);
            }
            FileOutcome::IoError(msg) => {
                errors += 1;
                eprintln!("IO_ERROR\t{}\t{}", path.display(), msg);
            }
        }
    }
    let mode = if opts.dry_run { "dry-run" } else { "write" };
    print_summary_strip(mode, rewritten, unchanged, errors);
    Ok(errors)
}
/// Strip-mode summary emitter. Emits to stderr (consistent with the
/// metadata-vs-data convention: findings/diffs/REWRITE lines are data on
/// stdout; the summary is metadata about the run on stderr).
fn print_summary_strip(mode: &str, rewritten: u32, unchanged: u32, errors: u32) {
    eprintln!(
        "SUMMARY\tmode={mode}\trewritten={rewritten}\tunchanged={unchanged}\terrors={errors}"
    );
}
/// Lint-mode summary emitter. Writes to stderr (consistent with metadata
/// convention).
fn print_summary_lint(files: u32, findings: u32, errors: u32) {
    eprintln!("SUMMARY\tmode=lint\tfiles={files}\tfindings={findings}\terrors={errors}");
}
fn run_lint(opts: &Options) -> Result<u32, CommentFreeError> {
    let budget = DocBudget {
        max_words: opts.doc_max_words,
    };
    let mut findings_total = 0u32;
    let mut errors = 0u32;
    let mut files_scanned = 0u32;
    for path in walk_rs_files(&opts.root) {
        files_scanned += 1;
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                errors += 1;
                eprintln!("IO_ERROR\t{}\t{e}", path.display());
                continue;
            }
        };
        let ast = match syn::parse_file(&source) {
            Ok(f) => f,
            Err(e) => {
                errors += 1;
                eprintln!("PARSE_ERROR\t{}\t{e}", path.display());
                continue;
            }
        };
        for finding in doc_lint_file(&ast, budget) {
            findings_total += 1;
            let suffix = if finding.fail_closed {
                "\tfail_closed=unbalanced_fence"
            } else {
                ""
            };
            println!(
                "DOC_LINT\t{}:{}\titem={}\twords={}\tbudget={}{suffix}",
                path.display(),
                finding.line,
                finding.item_label,
                finding.word_count,
                finding.budget
            );
            println!("DOC_LINT_MSG\t{DOC_LINT_DOCTRINE_MSG}");
        }
    }
    print_summary_lint(files_scanned, findings_total, errors);
    if errors > 0 {
        return Ok(errors);
    }
    if findings_total > 0 {
        return Err(CommentFreeError::DocLintFailure);
    }
    Ok(0)
}
