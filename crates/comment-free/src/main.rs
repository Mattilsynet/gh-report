#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![warn(clippy::missing_const_for_fn)]
use clap::Parser;
use comment_free::{
    CommentFreeError, DOC_LINT_DOCTRINE_MSG, DocBudget, FileOutcome, ProcessOptions, SKIP_DIRS,
    doc_lint_file, process_file, scan_doc_files,
};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use walkdir::WalkDir;
#[derive(Parser, Debug)]
#[command(
    name = "comment-free",
    about = "Doc-comment linter for Rust crates (default). \
             Use --rewrite for a byte-preserving rewrite of doc-link idioms in `///`, `//!`, `#[doc=...]`, and `#[cfg_attr(_, doc=...)]` payloads. Non-doc bytes are preserved verbatim; rustfmt is not invoked.",
    long_about = "Default mode is a read-only doc-comment budget linter: walks ROOT for .rs files \
                  and reports doc comments whose prose word count exceeds --doc-max-words. \
                  Fenced code blocks (` ``` ` or `~~~`) are excluded from the count; the \
                  doctrine allows 0-3 such fenced examples per doc comment and they do not \
                  consume the word budget. Examples are detected mechanically by fence \
                  delimiters only — there is no semantic example detection. Each finding is \
                  followed by a DOC_LINT_MSG line carrying the project doctrine. Doc comments \
                  are NEVER deleted by this tool.\n\
                  \n\
                  `--rewrite` is a byte-preserving doc-only pass: mutates ONLY `///`, `//!`, \
                  `#[doc = \"...\"]`, `#![doc = \"...\"]`, and `#[cfg_attr(_, doc = \"...\")]` \
                  payload text via the rustdoc-link idiom normaliser. Every other byte in the \
                  source is preserved verbatim. Does NOT run rustfmt, does NOT strip non-doc \
                  `//` and `/* */` comments, does NOT touch block doc comments (`/** */`). \
                  Line-count-preserving and idempotent.\n\
                  \n\
                  --dry-run is always safe in both modes.\n\
                  \n\
                  Exit codes:\n\
                    0  clean (no findings, no errors)\n\
                    1  catastrophic / unmapped IO error\n\
                    2  invalid CLI arguments (clap rejection)\n\
                    4  doc-lint findings observed (default mode)\n\
                    5  per-file parse/IO errors observed during processing (both modes)\n\
                  \n\
                  Output streams: findings (DOC_LINT, REWRITE, WOULD_REWRITE, diffs) on \
                  stdout; metadata (SUMMARY, DOC_WARN, errors) on stderr."
)]
struct Options {
    #[arg(default_value = ".", value_name = "ROOT")]
    root: PathBuf,
    /// Byte-preserving rewrite of doc-link idioms inside `///`, `//!`,
    /// `#[doc = "..."]`, `#![doc = "..."]`, and `#[cfg_attr(_, doc = "...")]`
    /// payloads. Every other byte in the source is preserved verbatim.
    /// rustfmt is not invoked; non-doc comments survive; block doc
    /// comments (`/** */`) are left untouched.
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
    #[arg(long)]
    rewrite: bool,
    /// Preview the rewrite as a unified diff without modifying files.
    /// Only meaningful with `--rewrite`. Default (lint) mode is
    /// already read-only.
    #[arg(long, short = 'n', requires = "rewrite")]
    dry_run: bool,
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
        Ok(run_rewrite(opts))
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
fn run_rewrite(opts: &Options) -> u32 {
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
    print_summary_rewrite(mode, rewritten, unchanged, errors);
    errors
}
/// Rewrite-mode summary emitter. Emits to stderr (consistent with the
/// metadata-vs-data convention: findings/diffs/REWRITE lines are data on
/// stdout; the summary is metadata about the run on stderr).
fn print_summary_rewrite(mode: &str, rewritten: u32, unchanged: u32, errors: u32) {
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
