#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![warn(clippy::missing_const_for_fn)]
use clap::Parser;
use comment_free::{
    CommentFreeError, DOC_LINT_DOCTRINE_MSG, DOC_LINT_RECORD_VERSION, DocBudget, FileOutcome,
    ProcessOptions, SKIP_DIRS, doc_lint_file, process_file, scan_doc_files,
};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use walkdir::WalkDir;
#[derive(Parser, Debug)]
#[command(
    name = "comment-free",
    about = "Doc-comment linter and byte-preserving rustdoc-link rewriter for Rust crates. \
             Default mode lints doc-comment word budget. \
             `--rewrite` strips non-doc `//` and `/* */` comments via the rustc lexer (preserving doc comments, AUTO-TRAIT-POLICY markers, and `// SAFETY:` lines) and canonicalises Rust intra-doc-link idioms in doc-comment payloads. Both passes are byte-preserving outside their targets.",
    long_about = "Default mode is a read-only doc-comment budget linter: walks ROOT for .rs files \
                  and reports doc comments whose prose word count exceeds --doc-max-words. \
                  Fenced code blocks (` ``` ` or `~~~`) are excluded from the count; the \
                  doctrine allows 0-3 such fenced examples per doc comment and they do not \
                  consume the word budget. Examples are detected mechanically by fence \
                  delimiters only — there is no semantic example detection. Doc comments \
                  are NEVER deleted by this tool.\n\
                  \n\
                  Lint output is structured for LLM-agent consumption. The full record\n\
                  grammar is published as `comment_free::DOC_LINT_RECORD_GRAMMAR` and the\n\
                  record-format version as `comment_free::DOC_LINT_RECORD_VERSION`.\n\
                  \n\
                    DOC_LINT          one per finding, full path:line + item + words + budget\n\
                    DOC_LINT_HEADER   one per finding kind (kind=overlong_doc), names the doctrine once\n\
                    DOC_LINT_HINT     up to 50 per kind, tab-separated structured fields:\n\
                                        path:line, item=…, words=N, budget=M, kind=overlong_doc, v=1\n\
                                        sorted by overshoot (words - budget) descending\n\
                    DOC_LINT_TRUNCATED tail summary when a kind has > 50 findings\n\
                  \n\
                  Rewrite mode (`--rewrite`):\n\
                  \n\
                  Two passes run in sequence, both byte-preserving outside their targets:\n\
                  \n\
                    1. Doc-link idiom canonicalisation: mutates ONLY `///`, `//!`, \
                       `#[doc = \"...\"]`, `#![doc = \"...\"]`, and `#[cfg_attr(_, doc = \"...\")]` \
                       payload text to canonical rustdoc link form ([Type](Type) -> [`Type`], etc.).\n\
                    2. Non-doc comment strip: removes `//` line comments and `/* */` block \
                       comments via the rustc lexer. Doc comments are kept. `// SAFETY:` lines \
                       and lines containing `AUTO-TRAIT-POLICY-BEGIN` / `AUTO-TRAIT-POLICY-END` \
                       markers are also kept. String literals are structurally unreachable by \
                       the strip pass — marker-looking text inside any string round-trips byte-identical.\n\
                  \n\
                  `--dry-run` is always safe; it emits unified diffs to stdout without writing files.\n\
                  \n\
                  The `--rustdoc-link-idioms` flag is a deprecated alias retained for one release; \
                  it dispatches the same default `--rewrite` behaviour and emits a deprecation note \
                  on stderr.\n\
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
    /// Run the byte-preserving rewrite passes over every `.rs` file
    /// under ROOT: canonicalise rustdoc-link idioms in doc payloads,
    /// then strip non-doc `//` and `/* */` comments via the rustc
    /// lexer. Doc comments, `// SAFETY:` lines, and AUTO-TRAIT-POLICY
    /// markers are preserved.
    #[arg(long)]
    rewrite: bool,
    /// Preview the rewrite as a unified diff without modifying files.
    /// Only meaningful with `--rewrite`. Default (lint) mode is
    /// already read-only; `--dry-run` is meaningful only with
    /// `--rewrite` (enforced by clap).
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
    /// DEPRECATED alias for plain `--rewrite`. Retained for one
    /// release. Dispatches the same byte-preserving rewrite path
    /// `--rewrite` runs by default; emits a deprecation note on stderr.
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
    if opts.rustdoc_link_idioms {
        eprintln!(
            "warning: --rustdoc-link-idioms is deprecated; the default --rewrite path now \
             includes rustdoc-link idiom canonicalisation along with lexer-based comment \
             stripping. This flag is a no-op alias retained for one release."
        );
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
#[allow(
    clippy::unnecessary_wraps,
    reason = "symmetric Result shape with run_lint keeps the dispatch in `run()` uniform; the variant set may grow if rewrite mode regrows error variants"
)]
fn run_strip(opts: &Options) -> Result<u32, CommentFreeError> {
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
/// Cap on `DOC_LINT_HINT` records emitted per finding kind. Beyond this,
/// the residual count is surfaced as a single `DOC_LINT_TRUNCATED` line.
/// Picked as the upper end of "comfortable to scan in an agent context
/// window"; the hard contract is the truncation record, not the cap value.
const DOC_LINT_HINT_CAP: usize = 50;

fn run_lint(opts: &Options) -> Result<u32, CommentFreeError> {
    let budget = DocBudget {
        max_words: opts.doc_max_words,
    };
    let mut all_findings: Vec<(std::path::PathBuf, comment_free::DocFinding)> = Vec::new();
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
            all_findings.push((path.clone(), finding));
        }
    }
    let findings_total = u32::try_from(all_findings.len()).unwrap_or(u32::MAX);
    for (path, finding) in &all_findings {
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
    }
    emit_doc_lint_hints(&all_findings);
    print_summary_lint(files_scanned, findings_total, errors);
    if errors > 0 {
        return Ok(errors);
    }
    if findings_total > 0 {
        return Err(CommentFreeError::DocLintFailure);
    }
    Ok(0)
}

/// Emit one `DOC_LINT_HEADER` per finding kind, up to
/// [`DOC_LINT_HINT_CAP`] structured `DOC_LINT_HINT` records (sorted by
/// `words - budget` descending), and a `DOC_LINT_TRUNCATED` summary line
/// when the kind has more findings than the cap.
///
/// Replaces the per-finding `DOC_LINT_MSG` doctrine spam: the doctrine
/// is named once on the header; hints carry only structured site
/// coordinates so an LLM-agent consumer can ingest the worst N
/// offenders without parsing prose.
fn emit_doc_lint_hints(findings: &[(std::path::PathBuf, comment_free::DocFinding)]) {
    if findings.is_empty() {
        return;
    }
    let kind = "overlong_doc";
    let v = DOC_LINT_RECORD_VERSION;
    println!("DOC_LINT_HEADER\tkind={kind}\tv={v}\tdoctrine={DOC_LINT_DOCTRINE_MSG}");
    let mut sorted: Vec<&(std::path::PathBuf, comment_free::DocFinding)> =
        findings.iter().collect();
    sorted.sort_by(|(_, a), (_, b)| {
        let oa = a.word_count.saturating_sub(a.budget);
        let ob = b.word_count.saturating_sub(b.budget);
        ob.cmp(&oa)
    });
    let kept = sorted.iter().take(DOC_LINT_HINT_CAP);
    for (path, f) in kept {
        println!(
            "DOC_LINT_HINT\t{}:{}\titem={}\twords={}\tbudget={}\tkind={kind}\tv={v}",
            path.display(),
            f.line,
            f.item_label,
            f.word_count,
            f.budget
        );
    }
    if sorted.len() > DOC_LINT_HINT_CAP {
        let remaining = sorted.len() - DOC_LINT_HINT_CAP;
        println!("DOC_LINT_TRUNCATED\tkind={kind}\tremaining={remaining}\tv={v}");
    }
}
