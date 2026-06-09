//! Pure logic for the `comment-free` tool: parse, re-emit, lint doc-comment budget.
#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![warn(clippy::missing_const_for_fn)]
use ra_ap_rustc_lexer::{FrontmatterAllowed, TokenKind, tokenize};
use similar::{ChangeTag, TextDiff};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Attribute, File, Meta, Token};
use walkdir::WalkDir;
/// Doctrine warning emitted on the `DOC_LINT_HEADER` for every kind.
pub const DOC_LINT_DOCTRINE_MSG: &str = "Rust docs must contain a concise summary, optionally 0-3 clear code examples (fenced ``` or ~~~ blocks), and sections explaining edge cases like panics, errors, and safety. Fenced code examples are excluded from the prose word length. If applicable references to ADRs must be given.";
/// Stable record-format version emitted on every `DOC_LINT_HEADER`,
/// `DOC_LINT_HINT`, and `DOC_LINT_TRUNCATED` line as `v=<N>`. Consumers
/// reject records whose `v=` is greater than the version they understand;
/// bumped on any incompatible field-shape change.
pub const DOC_LINT_RECORD_VERSION: u32 = 1;
/// Grammar of the structured lint records emitted in default lint mode,
/// in BNF-ish form. Stable across patch versions for a fixed
/// [`DOC_LINT_RECORD_VERSION`]; intended for external agent parsers and
/// for the binary's `--help` output.
///
/// Tab characters (`\t`) separate fields; every record terminates with
/// `\n`. Field order is fixed.
///
/// ```text
/// DOC_LINT_HEADER\tkind=<KIND>\tv=<N>\tdoctrine=<STRING>\n
/// DOC_LINT_HINT\t<PATH>:<LINE>\titem=<LABEL>\twords=<U32>\tbudget=<U32>\tkind=<KIND>\tv=<N>\n
/// DOC_LINT_TRUNCATED\tkind=<KIND>\tremaining=<U32>\tv=<N>\n
/// ```
///
/// Today `<KIND>` is always `overlong_doc`; new finding kinds emit
/// their own `DOC_LINT_HEADER` and a separate run of `DOC_LINT_HINT`
/// records. `<PATH>` is the path as walked by the tool and may contain
/// path separators; `<LABEL>` is human-readable and may contain spaces
/// but never a tab. Hints are sorted by `(words - budget)` descending
/// before the cap of 50 records per kind is applied.
pub const DOC_LINT_RECORD_GRAMMAR: &str = "\
DOC_LINT_HEADER\\tkind=<KIND>\\tv=<N>\\tdoctrine=<STRING>\\n
DOC_LINT_HINT\\t<PATH>:<LINE>\\titem=<LABEL>\\twords=<U32>\\tbudget=<U32>\\tkind=<KIND>\\tv=<N>\\n
DOC_LINT_TRUNCATED\\tkind=<KIND>\\tremaining=<U32>\\tv=<N>\\n";
/// All terminal-error variants raised by the `comment-free` binary.
#[derive(Debug, thiserror::Error)]
pub enum CommentFreeError {
    /// ROOT path passed on the CLI is not a directory.
    #[error("ROOT is not a directory")]
    NotADirectory,
    /// Generic IO error surfaced from [`std::io`].
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    /// Doc-lint violation: at least one `DOC_LINT` finding emitted under default lint mode.
    #[error("doc lint failure")]
    DocLintFailure,
}
impl From<&CommentFreeError> for ExitCode {
    fn from(e: &CommentFreeError) -> Self {
        match e {
            CommentFreeError::NotADirectory => Self::from(2),
            CommentFreeError::DocLintFailure => Self::from(4),
            CommentFreeError::Io(_) => Self::from(1),
        }
    }
}
/// Outcome of processing one source file.
#[derive(Debug)]
pub enum FileOutcome {
    Rewritten { diff: Option<String> },
    Unchanged,
    ParseError(String),
    IoError(String),
}
/// Knobs [`process_file`] reads. `main.rs`'s clap `Options` is intentionally a
/// superset; this trims the surface to what the pure logic actually needs.
pub struct ProcessOptions {
    pub dry_run: bool,
    pub context: usize,
}
/// Process `path`: doc-comment link-idiom canonicalisation + lexer-based
/// non-doc comment strip. Both passes are byte-preserving outside their
/// targets; code formatting and whitespace outside comments are untouched.
///
/// Returns:
///
/// - [`FileOutcome::Rewritten`] when the file content changed (with a
///   unified diff in `dry_run` mode, `None` otherwise).
/// - [`FileOutcome::Unchanged`] when neither pass produced bytes that
///   differ from the input.
/// - [`FileOutcome::ParseError`] when the syn parse required for the
///   doc-link pass fails. The file is left untouched on disk.
/// - [`FileOutcome::IoError`] for any I/O failure.
///
/// Stripped: ordinary `//` line comments and `/* */` block comments.
/// Preserved: doc comments (`///`, `//!`, `/** */`, `/*! */`),
/// `// SAFETY:` / `// SAFETY` lines, and lines containing
/// `AUTO-TRAIT-POLICY-BEGIN` / `AUTO-TRAIT-POLICY-END` markers.
#[must_use]
pub fn process_file(path: &Path, opts: &ProcessOptions) -> FileOutcome {
    let original = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return FileOutcome::IoError(e.to_string()),
    };
    let ast: File = match syn::parse_file(&original) {
        Ok(f) => f,
        Err(e) => return FileOutcome::ParseError(e.to_string()),
    };
    let splices = collect_doc_splices(&ast, &original);
    let after_links = if splices.is_empty() {
        original.clone()
    } else {
        apply_splices(&original, splices)
    };
    let rewritten = strip_line_comments(&after_links);
    if rewritten == original {
        return FileOutcome::Unchanged;
    }
    if opts.dry_run {
        let diff = unified_diff(path, &original, &rewritten, opts.context);
        FileOutcome::Rewritten { diff: Some(diff) }
    } else {
        match fs::write(path, rewritten) {
            Ok(()) => FileOutcome::Rewritten { diff: None },
            Err(e) => FileOutcome::IoError(e.to_string()),
        }
    }
}
/// Substring tokens identifying line comments that must be preserved
/// when stripping non-doc comments. Block comments are NEVER on the
/// preserved list — these idioms are line-comment-shaped by convention
/// (`// SAFETY:`, the `assert_auto_traits!` sentinel markers).
///
/// Conservatively matched as substrings (not full-line) so leading
/// whitespace and trailing prose around the token still preserve the
/// line. `// SAFETY:` is included for forward-compatibility with
/// `unsafe` code (ADR-0014 forbids workspace-authored `unsafe` today;
/// the idiom is preserved doctrinally for the future).
const PRESERVED_LINE_COMMENT_TOKENS: &[&str] = &[
    "AUTO-TRAIT-POLICY-BEGIN",
    "AUTO-TRAIT-POLICY-END",
    "SAFETY:",
    "SAFETY ",
];
/// True iff `line_comment_body` (including its `//` prefix) matches one of
/// the preserved line-comment substrings. Block-comment bodies are never
/// preserved — see [`PRESERVED_LINE_COMMENT_TOKENS`].
#[must_use]
pub fn line_comment_is_preserved(line_comment_body: &str) -> bool {
    PRESERVED_LINE_COMMENT_TOKENS
        .iter()
        .any(|tok| line_comment_body.contains(tok))
}
/// Strip non-doc line and block comments from `src` using
/// [`ra_ap_rustc_lexer`], preserving every other byte verbatim.
///
/// Each token's text is dropped iff:
/// - it is a `LineComment` with `doc_style: None`, OR
/// - it is a `BlockComment` with `doc_style: None`,
///
/// AND the comment body does not match [`comment_text_is_preserved`].
/// Doc comments (`doc_style: Some(_)`) and every non-comment token are
/// preserved unchanged. String literals (whose interiors the lexer
/// classifies as `Literal { kind: Str | ByteStr | CStr | RawStr | … }`)
/// are structurally unreachable by this pass: their bytes cannot be
/// reclassified as comment tokens, so marker-looking text inside a
/// string literal round-trips byte-identical.
///
/// When a stripped comment sat on a line of its own (only whitespace
/// before it on that line), the trailing newline that would otherwise
/// remain as a blank line is collapsed away by trimming the next
/// whitespace token's leading `\n`. This avoids leaving a blank line
/// scar in place of a removed comment.
///
/// When a stripped comment sat inline after code on the same line, the
/// run of horizontal whitespace separating the code from the removed
/// comment token is also trimmed, so `drop(rx); // close receiver`
/// becomes `drop(rx);` with no trailing space. Lines that did not lose
/// a comment token are untouched; pre-existing trailing whitespace on
/// such lines is preserved verbatim.
#[must_use]
pub fn strip_line_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut cursor = 0usize;
    let mut pending_blank_collapse = false;
    for token in tokenize(src, FrontmatterAllowed::Yes) {
        let end = cursor + token.len as usize;
        let text = &src[cursor..end];
        cursor = end;
        let is_comment = matches!(
            token.kind,
            TokenKind::LineComment { .. } | TokenKind::BlockComment { .. }
        );
        let drop = match token.kind {
            TokenKind::LineComment { doc_style: None } => !line_comment_is_preserved(text),
            TokenKind::BlockComment { doc_style: None, .. } => true,
            _ => false,
        };
        if drop {
            let before_comment = &src[..end - text.len()];
            let was_line_alone = line_was_blank_before(before_comment);
            trim_trailing_whitespace_to_last_newline(&mut out);
            if was_line_alone {
                pending_blank_collapse = true;
            }
            continue;
        }
        if pending_blank_collapse && matches!(token.kind, TokenKind::Whitespace) {
            let trimmed = text.strip_prefix('\n').unwrap_or(text);
            out.push_str(trimmed);
            pending_blank_collapse = false;
        } else {
            out.push_str(text);
            if !matches!(token.kind, TokenKind::Whitespace) || !is_comment {
                pending_blank_collapse = false;
            }
        }
    }
    out
}
/// True iff `prefix` ends with a sequence that includes no characters
/// other than horizontal whitespace since the most recent `\n` (or the
/// start of input). Caller passes the source bytes up to but excluding
/// the comment whose blankness is being judged.
fn line_was_blank_before(prefix: &str) -> bool {
    let line_start = prefix.rfind('\n').map_or(0, |p| p + 1);
    prefix[line_start..]
        .chars()
        .all(|c| c == ' ' || c == '\t')
}
/// In-place: drop any trailing run of horizontal whitespace from `s`,
/// leaving prior `\n` and earlier content intact. Used to clean up the
/// indentation that preceded a stripped solo-line comment so the
/// collapse leaves no trailing-whitespace residue.
fn trim_trailing_whitespace_to_last_newline(s: &mut String) {
    while matches!(s.chars().last(), Some(' ' | '\t')) {
        s.pop();
    }
}
/// One byte-range replacement against the original source.
///
/// `range` is a byte range in the original source string; `replacement`
/// is the substitute. Splices are applied in reverse start-order so
/// earlier offsets are not invalidated by later mutations.
#[derive(Debug, Clone)]
struct DocSplice {
    range: std::ops::Range<usize>,
    replacement: String,
}
/// Apply `splices` to `original` and return the rewritten source.
///
/// Splices must not overlap. Applied in reverse order of start byte
/// so each application leaves the not-yet-applied splices' offsets
/// valid.
fn apply_splices(original: &str, mut splices: Vec<DocSplice>) -> String {
    splices.sort_by_key(|s| std::cmp::Reverse(s.range.start));
    let mut out = original.to_string();
    for splice in splices {
        out.replace_range(splice.range, &splice.replacement);
    }
    out
}
/// Walk `ast`, collect a [`DocSplice`] for every doc surface whose
/// payload changes under [`rewrite_rustdoc_link_idioms`].
///
/// Surfaces handled: file-level inner attributes (`#![doc = "..."]`,
/// `//!`); per-item attributes (`#[doc = "..."]`, `///`) grouped by
/// run; `cfg_attr(_, doc = "...")` payloads in isolation; trait-item,
/// impl-item, field, and variant attributes (same model).
///
/// Block doc comments (`/** ... */`) are NOT touched — the in-memory
/// payload and on-disk source bytes diverge (lexer strips leading
/// `*`).
fn collect_doc_splices(ast: &syn::File, original: &str) -> Vec<DocSplice> {
    let mut out = Vec::new();
    collect_attr_run_splices(&ast.attrs, original, &mut out);
    collect_cfg_attr_doc_splices(&ast.attrs, original, &mut out);
    for item in &ast.items {
        collect_item_splices(item, original, &mut out);
    }
    out
}
fn collect_item_splices(item: &syn::Item, original: &str, out: &mut Vec<DocSplice>) {
    if let Some(attrs) = item_attrs(item) {
        collect_attr_run_splices(attrs, original, out);
        collect_cfg_attr_doc_splices(attrs, original, out);
    }
    match item {
        syn::Item::Struct(s) => {
            for field in &s.fields {
                collect_attr_run_splices(&field.attrs, original, out);
                collect_cfg_attr_doc_splices(&field.attrs, original, out);
            }
        }
        syn::Item::Enum(e) => {
            for v in &e.variants {
                collect_attr_run_splices(&v.attrs, original, out);
                collect_cfg_attr_doc_splices(&v.attrs, original, out);
                for f in &v.fields {
                    collect_attr_run_splices(&f.attrs, original, out);
                    collect_cfg_attr_doc_splices(&f.attrs, original, out);
                }
            }
        }
        syn::Item::Union(u) => {
            for f in &u.fields.named {
                collect_attr_run_splices(&f.attrs, original, out);
                collect_cfg_attr_doc_splices(&f.attrs, original, out);
            }
        }
        syn::Item::Trait(t) => {
            for ti in &t.items {
                if let Some(attrs) = trait_item_attrs(ti) {
                    collect_attr_run_splices(attrs, original, out);
                    collect_cfg_attr_doc_splices(attrs, original, out);
                }
            }
        }
        syn::Item::Impl(i) => {
            for ii in &i.items {
                if let Some(attrs) = impl_item_attrs(ii) {
                    collect_attr_run_splices(attrs, original, out);
                    collect_cfg_attr_doc_splices(attrs, original, out);
                }
            }
        }
        syn::Item::Mod(m) => {
            if let Some((_, items)) = &m.content {
                for inner in items {
                    collect_item_splices(inner, original, out);
                }
            }
        }
        _ => {}
    }
}
const fn item_attrs(item: &syn::Item) -> Option<&Vec<Attribute>> {
    use syn::Item::{
        Const, Enum, ExternCrate, Fn, Impl, Macro, Mod, Static, Struct, Trait, TraitAlias, Type,
        Union, Use,
    };
    Some(match item {
        Const(i) => &i.attrs,
        Enum(i) => &i.attrs,
        ExternCrate(i) => &i.attrs,
        Fn(i) => &i.attrs,
        Impl(i) => &i.attrs,
        Macro(i) => &i.attrs,
        Mod(i) => &i.attrs,
        Static(i) => &i.attrs,
        Struct(i) => &i.attrs,
        Trait(i) => &i.attrs,
        TraitAlias(i) => &i.attrs,
        Type(i) => &i.attrs,
        Union(i) => &i.attrs,
        Use(i) => &i.attrs,
        _ => return None,
    })
}
const fn trait_item_attrs(item: &syn::TraitItem) -> Option<&Vec<Attribute>> {
    use syn::TraitItem::{Const, Fn, Macro, Type};
    Some(match item {
        Const(i) => &i.attrs,
        Fn(i) => &i.attrs,
        Macro(i) => &i.attrs,
        Type(i) => &i.attrs,
        _ => return None,
    })
}
const fn impl_item_attrs(item: &syn::ImplItem) -> Option<&Vec<Attribute>> {
    use syn::ImplItem::{Const, Fn, Macro, Type};
    Some(match item {
        Const(i) => &i.attrs,
        Fn(i) => &i.attrs,
        Macro(i) => &i.attrs,
        Type(i) => &i.attrs,
        _ => return None,
    })
}
/// Group `attrs` into contiguous runs of safe-to-splice doc payloads
/// and emit one [`DocSplice`] per literal whose rewrite differs from
/// the original payload text.
///
/// "Safe to splice" excludes block doc comments (`/** ... */`); the
/// run is broken on encountering one or on any non-doc attribute.
fn collect_attr_run_splices(attrs: &[Attribute], original: &str, out: &mut Vec<DocSplice>) {
    let mut i = 0;
    while i < attrs.len() {
        let Some(_) = doc_attr_literal_span(&attrs[i], original, DocShape::SafeLineOrAttr) else {
            i += 1;
            continue;
        };
        let start = i;
        while i < attrs.len()
            && doc_attr_literal_span(&attrs[i], original, DocShape::SafeLineOrAttr).is_some()
        {
            i += 1;
        }
        emit_run_splices(&attrs[start..i], original, out);
    }
}
#[derive(Debug, Clone, Copy)]
enum DocShape {
    SafeLineOrAttr,
}
/// Return the byte-range and shape of a doc literal's storage in
/// `original` if `attr` is one of:
///
/// - `///` / `//!` (line doc, range covers the whole `///`+payload line)
/// - `#[doc = "…"]` / `#![doc = "…"]` (range covers the quoted literal token)
///
/// Returns `None` for non-`doc` attributes, `cfg_attr`, and block
/// doc comments (`/** … */`).
fn doc_attr_literal_span(
    attr: &Attribute,
    original: &str,
    _shape: DocShape,
) -> Option<DocLiteralSite> {
    let Meta::NameValue(nv) = &attr.meta else {
        return None;
    };
    if !nv.path.is_ident("doc") {
        return None;
    }
    let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(s),
        ..
    }) = &nv.value
    else {
        return None;
    };
    let range = s.span().byte_range();
    let body = original.get(range.clone())?;
    let kind = classify_doc_literal(body)?;
    Some(DocLiteralSite {
        range,
        kind,
        value: s.value(),
    })
}
#[derive(Debug, Clone)]
struct DocLiteralSite {
    range: std::ops::Range<usize>,
    kind: DocLiteralKind,
    value: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocLiteralKind {
    OuterLine,
    InnerLine,
    QuotedAttr,
}
/// Classify the source-byte form of a doc literal.
///
/// Returns `None` for block doc comments (`/** … */`) — these are
/// deliberately left untouched by the safe path.
fn classify_doc_literal(body: &str) -> Option<DocLiteralKind> {
    if body.starts_with("///") && !body.starts_with("////") {
        Some(DocLiteralKind::OuterLine)
    } else if body.starts_with("//!") {
        Some(DocLiteralKind::InnerLine)
    } else if body.starts_with('"') && body.ends_with('"') {
        Some(DocLiteralKind::QuotedAttr)
    } else {
        None
    }
}
/// Emit splices for a run of contiguous doc literals.
///
/// All literals in `run` are spliceable line- or attribute-form docs
/// (i.e. classified by [`classify_doc_literal`]). The run is joined with
/// `\n`, transformed once so the fenced-code tracker sees the whole block,
/// then split back into the same number of lines. Each line is individually
/// spliced (its quoted form for `#[doc = …]`, or its `///`/`//!`-prefixed
/// form for line docs) so non-doc bytes between docs in the run are
/// preserved verbatim.
fn emit_run_splices(run: &[Attribute], original: &str, out: &mut Vec<DocSplice>) {
    let sites: Vec<DocLiteralSite> = run
        .iter()
        .filter_map(|a| doc_attr_literal_span(a, original, DocShape::SafeLineOrAttr))
        .collect();
    if sites.is_empty() {
        return;
    }
    let joined = sites
        .iter()
        .map(|s| s.value.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let rewritten = rewrite_rustdoc_link_idioms(&joined);
    if rewritten == joined {
        return;
    }
    let parts: Vec<&str> = rewritten.split('\n').collect();
    if parts.len() != sites.len() {
        return;
    }
    for (site, new_payload) in sites.iter().zip(parts) {
        if new_payload == site.value {
            continue;
        }
        let Some(body) = original.get(site.range.clone()) else {
            continue;
        };
        let replacement = match site.kind {
            DocLiteralKind::OuterLine => render_line_doc(body, "///", new_payload),
            DocLiteralKind::InnerLine => render_line_doc(body, "//!", new_payload),
            DocLiteralKind::QuotedAttr => Some(render_quoted_doc_literal(new_payload)),
        };
        let Some(replacement) = replacement else {
            continue;
        };
        out.push(DocSplice {
            range: site.range.clone(),
            replacement,
        });
    }
}
/// Render a replacement for a `///` or `//!` line-doc storage range.
///
/// The original `body` is the full `///…` or `//!…` source line. Its
/// leading marker may be `///`, `////` (impossible — filtered earlier),
/// or `//!`. We preserve the *exact* marker bytes the source used
/// (just `///` or `//!`) and substitute the trailing payload with
/// `new_payload`.
///
/// Returns `None` if the body doesn't start with the expected marker
/// (defensive; should not happen given [`classify_doc_literal`]).
fn render_line_doc(body: &str, marker: &str, new_payload: &str) -> Option<String> {
    if !body.starts_with(marker) {
        return None;
    }
    let mut out = String::with_capacity(marker.len() + new_payload.len());
    out.push_str(marker);
    out.push_str(new_payload);
    Some(out)
}
/// Render a properly-quoted Rust string literal for a `#[doc = "…"]`
/// payload value. Uses [`proc_macro2::Literal::string`] for the
/// quoting/escaping rules; converts to its source-form via [`ToString`].
fn render_quoted_doc_literal(value: &str) -> String {
    proc_macro2::Literal::string(value).to_string()
}
/// Collect splices for every `#[cfg_attr(_, doc = "…")]` payload in `attrs`.
///
/// Each `doc = "…"` payload literal inside a `cfg_attr` list is transformed
/// in isolation (gating predicates may differ) and spliced at its own
/// [`syn::LitStr`] byte-range.
fn collect_cfg_attr_doc_splices(attrs: &[Attribute], original: &str, out: &mut Vec<DocSplice>) {
    for attr in attrs {
        if !is_cfg_attr(attr) {
            continue;
        }
        let Meta::List(list) = &attr.meta else {
            continue;
        };
        let parsed: Result<Punctuated<Meta, Token![,]>, _> =
            list.parse_args_with(Punctuated::parse_terminated);
        let Ok(metas) = parsed else {
            continue;
        };
        for meta in metas.iter().skip(1) {
            let Meta::NameValue(nv) = meta else {
                continue;
            };
            if !nv.path.is_ident("doc") {
                continue;
            }
            let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
            else {
                continue;
            };
            let range = s.span().byte_range();
            let Some(body) = original.get(range.clone()) else {
                continue;
            };
            if !(body.starts_with('"') && body.ends_with('"')) {
                continue;
            }
            let value = s.value();
            let rewritten = rewrite_rustdoc_link_idioms(&value);
            if rewritten == value {
                continue;
            }
            let replacement = render_quoted_doc_literal(&rewritten);
            out.push(DocSplice { range, replacement });
        }
    }
}
/// Render a unified diff between `original` and `rewritten` for `path`.
#[must_use]
pub fn unified_diff(path: &Path, original: &str, rewritten: &str, context: usize) -> String {
    let display = path.display().to_string();
    let diff = TextDiff::from_lines(original, rewritten);
    let mut out = String::new();
    writeln!(out, "--- a/{display}").expect("Write for String never fails");
    writeln!(out, "+++ b/{display}").expect("Write for String never fails");
    for hunk in diff.unified_diff().context_radius(context).iter_hunks() {
        writeln!(out, "{}", hunk.header()).expect("Write for String never fails");
        for change in hunk.iter_changes() {
            let sign = match change.tag() {
                ChangeTag::Equal => ' ',
                ChangeTag::Delete => '-',
                ChangeTag::Insert => '+',
            };
            let value = change.value();
            out.push(sign);
            out.push_str(value);
            if !value.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out
}
/// True if `attr` is a `#[cfg_attr(...)]` attribute.
#[must_use]
pub fn is_cfg_attr(attr: &Attribute) -> bool {
    match &attr.meta {
        Meta::List(list) => list.path.is_ident("cfg_attr"),
        _ => false,
    }
}
/// Walk `root` and return every file path that looks like documentation.
///
/// Skips dotfiles/dotdirs, `target/`, and common vendor/build directories
/// (`node_modules`, `vendor`, `dist`, `build`) to avoid polyglot-repo noise.
#[must_use]
pub fn scan_doc_files(root: &Path) -> Vec<PathBuf> {
    let mut hits = Vec::new();
    let walker = WalkDir::new(root)
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
        });
    for entry in walker.filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if is_doc_path(path, root) {
            hits.push(path.to_path_buf());
        }
    }
    hits
}
/// Directories `scan_doc_files` and `.rs` traversal skip wholesale.
pub const SKIP_DIRS: &[&str] = &["target", "node_modules", "vendor", "dist", "build"];
/// True if `path` looks like documentation: doc file extension, bare
/// README/LICENSE-style stem, or living under a top-level `docs/` / `doc/`
/// directory directly beneath `root`.
///
/// The `docs/`/`doc/` rule is **scoped to the first relative component
/// under `root`**, so `src/docs/mod.rs` or `crates/foo/doc/inner.rs` do
/// NOT match. This narrows the strip-mode `DOC_WARN` noise to genuine
/// top-level documentation directories.
#[must_use]
pub fn is_doc_path(path: &Path, root: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        let ext_lc = ext.to_ascii_lowercase();
        if matches!(
            ext_lc.as_str(),
            "md" | "markdown" | "rst" | "adoc" | "asciidoc" | "txt"
        ) {
            return true;
        }
    }
    if let Ok(rel) = path.strip_prefix(root)
        && let Some(first) = rel.components().next()
        && let Some(s) = first.as_os_str().to_str()
    {
        let s_lc = s.to_ascii_lowercase();
        if s_lc == "docs" || s_lc == "doc" {
            return true;
        }
    }
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        let stem_uc = stem.to_ascii_uppercase();
        for known in BARE_DOC_STEMS {
            if stem_uc == *known {
                return true;
            }
        }
    }
    false
}
const BARE_DOC_STEMS: &[&str] = &[
    "LICENSE",
    "LICENCE",
    "NOTICE",
    "COPYING",
    "README",
    "CHANGELOG",
    "AUTHORS",
    "CONTRIBUTORS",
];
/// Doc-comment word budget for the [`doc_lint_file`] linter.
///
/// The doctrine attached to every finding ([`DOC_LINT_DOCTRINE_MSG`])
/// allows 0-3 fenced code examples per doc comment; examples are
/// defined mechanically as fenced code blocks (` ``` ` or `~~~`) and
/// do not count toward the prose word budget. The linter has no
/// semantic notion of an "example" — fence delimiters are the only
/// signal.
#[derive(Debug, Clone, Copy)]
pub struct DocBudget {
    /// Maximum words allowed per doc comment (prose only; fenced code,
    /// including ` ``` ` and `~~~` blocks, is excluded from the count).
    pub max_words: usize,
}
/// Result of counting prose words in a doc comment.
#[derive(Debug, Clone, Copy)]
pub enum WordCount {
    /// Fence state was balanced; `count` excludes fenced code.
    Balanced(usize),
    /// Fence was opened but never closed; `count` is the fail-closed
    /// recount treating every line as prose.
    FailClosed(usize),
}
impl WordCount {
    /// Return the numeric count, regardless of balance state.
    #[must_use]
    pub const fn count(self) -> usize {
        match self {
            Self::Balanced(n) | Self::FailClosed(n) => n,
        }
    }
    /// True iff this count came from the fail-closed recount path.
    #[must_use]
    pub const fn is_fail_closed(self) -> bool {
        matches!(self, Self::FailClosed(_))
    }
}
/// A single doc-comment over-budget finding emitted by [`doc_lint_file`].
#[derive(Debug, Clone)]
pub struct DocFinding {
    /// Human-readable label for the docced item, e.g. `"fn foo"` or `"struct Bar"`.
    pub item_label: String,
    /// Approximate source line of the docced item (from `proc_macro2` spans).
    pub line: usize,
    /// Word count of the item's doc-comment prose (fenced code excluded).
    pub word_count: usize,
    /// The budget the count exceeded.
    pub budget: usize,
    /// True when `word_count` came from the fail-closed recount path
    /// (unbalanced fence at EOF). `words=` is then an inflated number,
    /// not the real prose count.
    pub fail_closed: bool,
}
/// Lint `ast` for doc-comments whose prose word count exceeds `budget.max_words`.
///
/// Concatenates `///`, `//!`, `#[doc=...]`, and `cfg_attr` doc payloads in
/// source order; only triple-backtick fenced lines are excluded from the count.
/// Docs inside opaque macro bodies are not visited.
#[must_use]
pub fn doc_lint_file(ast: &syn::File, budget: DocBudget) -> Vec<DocFinding> {
    let mut visitor = DocLintVisitor {
        budget,
        findings: Vec::new(),
    };
    visitor.lint_attrs(&ast.attrs, "file-level", None);
    syn::visit::Visit::visit_file(&mut visitor, ast);
    visitor.findings
}
struct DocLintVisitor {
    budget: DocBudget,
    findings: Vec<DocFinding>,
}
impl DocLintVisitor {
    fn lint_attrs(&mut self, attrs: &[Attribute], label: &str, span_line: Option<usize>) {
        let Some((text, attr_line)) = extract_doc_text(attrs) else {
            return;
        };
        let words = prose_word_count(&text);
        if words.count() > self.budget.max_words {
            self.findings.push(DocFinding {
                item_label: label.to_string(),
                line: span_line.unwrap_or(attr_line),
                word_count: words.count(),
                budget: self.budget.max_words,
                fail_closed: words.is_fail_closed(),
            });
        }
    }
}
/// Concatenate doc payloads from `attrs` (in source order) and return the
/// combined text plus the approximate source line of the first doc attribute.
/// `None` if `attrs` carries no doc payloads.
fn extract_doc_text(attrs: &[Attribute]) -> Option<(String, usize)> {
    let mut parts: Vec<String> = Vec::new();
    let mut first_line: Option<usize> = None;
    for attr in attrs {
        let line = attr
            .path()
            .get_ident()
            .map_or_else(|| attr.span().start().line, |id| id.span().start().line);
        if let Some(payload) = doc_payload(attr) {
            if first_line.is_none() {
                first_line = Some(line);
            }
            parts.push(payload);
        } else if is_cfg_attr(attr) {
            for payload in cfg_attr_doc_payloads(attr) {
                if first_line.is_none() {
                    first_line = Some(line);
                }
                parts.push(payload);
            }
        }
    }
    let line = first_line?;
    Some((parts.join("\n"), line))
}
/// Extract the string payload of a `#[doc = "..."]` attribute, if it is one.
fn doc_payload(attr: &Attribute) -> Option<String> {
    let Meta::NameValue(nv) = &attr.meta else {
        return None;
    };
    if !nv.path.is_ident("doc") {
        return None;
    }
    let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(s),
        ..
    }) = &nv.value
    else {
        return None;
    };
    Some(s.value())
}
/// Extract every `doc = "..."` payload from inside a `#[cfg_attr(<pred>, ...)]`
/// list, ignoring the predicate. Returns empty vec if none.
fn cfg_attr_doc_payloads(attr: &Attribute) -> Vec<String> {
    let Meta::List(list) = &attr.meta else {
        return Vec::new();
    };
    if !list.path.is_ident("cfg_attr") {
        return Vec::new();
    }
    let parsed: Result<Punctuated<Meta, Token![,]>, _> =
        list.parse_args_with(Punctuated::parse_terminated);
    let Ok(metas) = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for meta in metas.into_iter().skip(1) {
        if let Meta::NameValue(nv) = &meta
            && nv.path.is_ident("doc")
            && let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
        {
            out.push(s.value());
        }
    }
    out
}
/// Walk `file`, mutating every doc-comment payload through
/// [`rewrite_rustdoc_link_idioms`]. Reaches every surface
/// [`doc_lint_file`] inspects (top-level attrs, items, trait/impl
/// items, fields, variants).
///
/// Per item, contiguous runs of unconditional `#[doc = "..."]`
/// attributes are joined with `\n`, transformed once (so a fenced
/// block spanning multiple `///` lines tracks fence state correctly),
/// then split back per attribute — the transform is line-count
/// invariant. `cfg_attr(_, doc = "...")` payloads transform
/// independently (different gating predicates).
pub fn apply_rustdoc_link_idioms_to_ast(file: &mut syn::File) {
    use syn::visit_mut::VisitMut;
    struct Visitor;
    impl VisitMut for Visitor {
        fn visit_file_mut(&mut self, node: &mut syn::File) {
            rewrite_attrs_doc_links(&mut node.attrs);
            syn::visit_mut::visit_file_mut(self, node);
        }
        fn visit_item_mut(&mut self, node: &mut syn::Item) {
            if let Some(attrs) = item_attrs_mut(node) {
                rewrite_attrs_doc_links(attrs);
            }
            syn::visit_mut::visit_item_mut(self, node);
        }
        fn visit_trait_item_mut(&mut self, node: &mut syn::TraitItem) {
            if let Some(attrs) = trait_item_attrs_mut(node) {
                rewrite_attrs_doc_links(attrs);
            }
            syn::visit_mut::visit_trait_item_mut(self, node);
        }
        fn visit_impl_item_mut(&mut self, node: &mut syn::ImplItem) {
            if let Some(attrs) = impl_item_attrs_mut(node) {
                rewrite_attrs_doc_links(attrs);
            }
            syn::visit_mut::visit_impl_item_mut(self, node);
        }
        fn visit_field_mut(&mut self, node: &mut syn::Field) {
            rewrite_attrs_doc_links(&mut node.attrs);
            syn::visit_mut::visit_field_mut(self, node);
        }
        fn visit_variant_mut(&mut self, node: &mut syn::Variant) {
            rewrite_attrs_doc_links(&mut node.attrs);
            syn::visit_mut::visit_variant_mut(self, node);
        }
    }
    Visitor.visit_file_mut(file);
}
/// Borrow the `attrs` slot of any `syn::Item` variant that carries one.
const fn item_attrs_mut(item: &mut syn::Item) -> Option<&mut Vec<Attribute>> {
    use syn::Item::{
        Const, Enum, ExternCrate, Fn, Impl, Macro, Mod, Static, Struct, Trait, TraitAlias, Type,
        Union, Use,
    };
    Some(match item {
        Const(i) => &mut i.attrs,
        Enum(i) => &mut i.attrs,
        ExternCrate(i) => &mut i.attrs,
        Fn(i) => &mut i.attrs,
        Impl(i) => &mut i.attrs,
        Macro(i) => &mut i.attrs,
        Mod(i) => &mut i.attrs,
        Static(i) => &mut i.attrs,
        Struct(i) => &mut i.attrs,
        Trait(i) => &mut i.attrs,
        TraitAlias(i) => &mut i.attrs,
        Type(i) => &mut i.attrs,
        Union(i) => &mut i.attrs,
        Use(i) => &mut i.attrs,
        _ => return None,
    })
}
const fn trait_item_attrs_mut(item: &mut syn::TraitItem) -> Option<&mut Vec<Attribute>> {
    use syn::TraitItem::{Const, Fn, Macro, Type};
    Some(match item {
        Const(i) => &mut i.attrs,
        Fn(i) => &mut i.attrs,
        Macro(i) => &mut i.attrs,
        Type(i) => &mut i.attrs,
        _ => return None,
    })
}
const fn impl_item_attrs_mut(item: &mut syn::ImplItem) -> Option<&mut Vec<Attribute>> {
    use syn::ImplItem::{Const, Fn, Macro, Type};
    Some(match item {
        Const(i) => &mut i.attrs,
        Fn(i) => &mut i.attrs,
        Macro(i) => &mut i.attrs,
        Type(i) => &mut i.attrs,
        _ => return None,
    })
}
/// Apply the link-idiom transform to one item's `attrs` slice.
///
/// Contiguous runs of unconditional `#[doc = "..."]` are joined and
/// transformed together; `cfg_attr(_, doc = "...")` payloads are each
/// transformed in isolation (they may be gated independently).
fn rewrite_attrs_doc_links(attrs: &mut [Attribute]) {
    let mut i = 0;
    while i < attrs.len() {
        if doc_string_payload(&attrs[i]).is_some() {
            let start = i;
            while i < attrs.len() && doc_string_payload(&attrs[i]).is_some() {
                i += 1;
            }
            rewrite_doc_run(&mut attrs[start..i]);
            continue;
        }
        if is_cfg_attr(&attrs[i]) {
            rewrite_cfg_attr_doc_payloads(&mut attrs[i]);
        }
        i += 1;
    }
}
/// Join, transform, and split-back a contiguous run of `#[doc = "..."]`
/// attributes. The transform is line-count-preserving, so the split has
/// the same length as the input — assigned 1:1 back into each attribute.
fn rewrite_doc_run(run: &mut [Attribute]) {
    if run.is_empty() {
        return;
    }
    let originals: Vec<String> = run
        .iter()
        .map(|a| doc_string_payload(a).unwrap_or_default())
        .collect();
    let joined = originals.join("\n");
    let rewritten = rewrite_rustdoc_link_idioms(&joined);
    if rewritten == joined {
        return;
    }
    let parts: Vec<&str> = rewritten.split('\n').collect();
    if parts.len() != originals.len() {
        return;
    }
    for (attr, new) in run.iter_mut().zip(parts) {
        set_doc_string_payload(attr, new);
    }
}
/// Rewrite every `doc = "..."` payload inside a `#[cfg_attr(_, ...)]`
/// list independently. Predicate position is left untouched.
fn rewrite_cfg_attr_doc_payloads(attr: &mut Attribute) {
    let Meta::List(list) = &mut attr.meta else {
        return;
    };
    if !list.path.is_ident("cfg_attr") {
        return;
    }
    let parsed: Result<Punctuated<Meta, Token![,]>, _> =
        list.parse_args_with(Punctuated::parse_terminated);
    let Ok(mut metas) = parsed else {
        return;
    };
    let mut changed = false;
    for meta in metas.iter_mut().skip(1) {
        if let Meta::NameValue(nv) = meta
            && nv.path.is_ident("doc")
            && let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &mut nv.value
        {
            let original = s.value();
            let rewritten = rewrite_rustdoc_link_idioms(&original);
            if rewritten != original {
                *s = syn::LitStr::new(&rewritten, s.span());
                changed = true;
            }
        }
    }
    if !changed {
        return;
    }
    list.tokens = quote::quote!(#metas);
}
/// Return the string payload of `#[doc = "..."]`, if it's literal.
fn doc_string_payload(attr: &Attribute) -> Option<String> {
    let Meta::NameValue(nv) = &attr.meta else {
        return None;
    };
    if !nv.path.is_ident("doc") {
        return None;
    }
    let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(s),
        ..
    }) = &nv.value
    else {
        return None;
    };
    Some(s.value())
}
/// Replace the string payload of a `#[doc = "..."]` attribute in place.
fn set_doc_string_payload(attr: &mut Attribute, new: &str) {
    let Meta::NameValue(nv) = &mut attr.meta else {
        return;
    };
    if !nv.path.is_ident("doc") {
        return;
    }
    let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(s),
        ..
    }) = &mut nv.value
    else {
        return;
    };
    *s = syn::LitStr::new(new, s.span());
}
/// Count words in `doc_text`, excluding fenced code.
///
/// Recognises ` ``` ` and `~~~` fences. Fail-closed: if a fence opens but
/// never closes, returns [`WordCount::FailClosed`] with a whole-text
/// recount so a malformed doc cannot silently suppress budget checking.
fn prose_word_count(doc_text: &str) -> WordCount {
    let mut in_fence = false;
    let mut words = 0usize;
    for line in doc_text.lines() {
        let stripped = line.trim_start();
        if stripped.starts_with("```") || stripped.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        words += line.split_whitespace().count();
    }
    if in_fence {
        let recount = doc_text.lines().map(|l| l.split_whitespace().count()).sum();
        return WordCount::FailClosed(recount);
    }
    WordCount::Balanced(words)
}
impl<'ast> syn::visit::Visit<'ast> for DocLintVisitor {
    fn visit_item(&mut self, node: &'ast syn::Item) {
        if let Some((label, attrs, line)) = item_label_and_attrs(node) {
            self.lint_attrs(attrs, &label, Some(line));
        }
        syn::visit::visit_item(self, node);
    }
    fn visit_trait_item(&mut self, node: &'ast syn::TraitItem) {
        if let Some((label, attrs, line)) = trait_item_label_and_attrs(node) {
            self.lint_attrs(attrs, &label, Some(line));
        }
        syn::visit::visit_trait_item(self, node);
    }
    fn visit_impl_item(&mut self, node: &'ast syn::ImplItem) {
        if let Some((label, attrs, line)) = impl_item_label_and_attrs(node) {
            self.lint_attrs(attrs, &label, Some(line));
        }
        syn::visit::visit_impl_item(self, node);
    }
    fn visit_field(&mut self, node: &'ast syn::Field) {
        let line = node.span().start().line;
        let label = node
            .ident
            .as_ref()
            .map_or_else(|| "field (tuple)".to_string(), |id| format!("field {id}"));
        self.lint_attrs(&node.attrs, &label, Some(line));
        syn::visit::visit_field(self, node);
    }
    fn visit_variant(&mut self, node: &'ast syn::Variant) {
        let line = node.span().start().line;
        let label = format!("variant {}", node.ident);
        self.lint_attrs(&node.attrs, &label, Some(line));
        syn::visit::visit_variant(self, node);
    }
}
fn item_label_and_attrs(item: &syn::Item) -> Option<(String, &[Attribute], usize)> {
    use syn::Item::{
        Const, Enum, ExternCrate, Fn, Impl, Macro, Mod, Static, Struct, Trait, Type, Union, Use,
    };
    let (label, attrs, line): (String, &[Attribute], usize) = match item {
        Fn(i) => (
            format!("fn {}", i.sig.ident),
            &i.attrs,
            i.sig.fn_token.span.start().line,
        ),
        Struct(i) => (
            format!("struct {}", i.ident),
            &i.attrs,
            i.struct_token.span.start().line,
        ),
        Enum(i) => (
            format!("enum {}", i.ident),
            &i.attrs,
            i.enum_token.span.start().line,
        ),
        Trait(i) => (
            format!("trait {}", i.ident),
            &i.attrs,
            i.trait_token.span.start().line,
        ),
        Mod(i) => (
            format!("mod {}", i.ident),
            &i.attrs,
            i.mod_token.span.start().line,
        ),
        Const(i) => (
            format!("const {}", i.ident),
            &i.attrs,
            i.const_token.span.start().line,
        ),
        Static(i) => (
            format!("static {}", i.ident),
            &i.attrs,
            i.static_token.span.start().line,
        ),
        Type(i) => (
            format!("type {}", i.ident),
            &i.attrs,
            i.type_token.span.start().line,
        ),
        Union(i) => (
            format!("union {}", i.ident),
            &i.attrs,
            i.union_token.span.start().line,
        ),
        Impl(i) => ("impl".to_string(), &i.attrs, i.impl_token.span.start().line),
        Use(i) => ("use".to_string(), &i.attrs, i.use_token.span.start().line),
        ExternCrate(i) => (
            format!("extern crate {}", i.ident),
            &i.attrs,
            i.extern_token.span.start().line,
        ),
        Macro(i) => (
            i.ident
                .as_ref()
                .map_or_else(|| "macro".to_string(), |id| format!("macro {id}")),
            &i.attrs,
            i.mac.span().start().line,
        ),
        _ => return None,
    };
    Some((label, attrs, line))
}
fn trait_item_label_and_attrs(item: &syn::TraitItem) -> Option<(String, &[Attribute], usize)> {
    use syn::TraitItem::{Const, Fn, Type};
    let (label, attrs, line): (String, &[Attribute], usize) = match item {
        Fn(i) => (
            format!("trait fn {}", i.sig.ident),
            &i.attrs,
            i.sig.fn_token.span.start().line,
        ),
        Const(i) => (
            format!("trait const {}", i.ident),
            &i.attrs,
            i.const_token.span.start().line,
        ),
        Type(i) => (
            format!("trait type {}", i.ident),
            &i.attrs,
            i.type_token.span.start().line,
        ),
        _ => return None,
    };
    Some((label, attrs, line))
}
fn impl_item_label_and_attrs(item: &syn::ImplItem) -> Option<(String, &[Attribute], usize)> {
    use syn::ImplItem::{Const, Fn, Type};
    let (label, attrs, line): (String, &[Attribute], usize) = match item {
        Fn(i) => (
            format!("impl fn {}", i.sig.ident),
            &i.attrs,
            i.sig.fn_token.span.start().line,
        ),
        Const(i) => (
            format!("impl const {}", i.ident),
            &i.attrs,
            i.const_token.span.start().line,
        ),
        Type(i) => (
            format!("impl type {}", i.ident),
            &i.attrs,
            i.type_token.span.start().line,
        ),
        _ => return None,
    };
    Some((label, attrs, line))
}
#[cfg(test)]
mod process_file_tests {
    use super::{FileOutcome, ProcessOptions, process_file, strip_line_comments};
    use std::fs;
    fn opts() -> ProcessOptions {
        ProcessOptions {
            dry_run: true,
            context: 3,
        }
    }
    #[test]
    fn whitespace_only_file_is_unchanged() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "   \n\t\n  \n").unwrap();
        match process_file(&path, &opts()) {
            FileOutcome::Unchanged => {}
            other => panic!("expected Unchanged for whitespace-only file, got {other:?}"),
        }
    }
    #[test]
    fn empty_file_is_unchanged() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "").unwrap();
        match process_file(&path, &opts()) {
            FileOutcome::Unchanged => {}
            other => panic!("expected Unchanged for empty file, got {other:?}"),
        }
    }
    #[test]
    fn strip_line_comments_drops_ordinary_line_comments() {
        let src = "// kill me\nfn f() {}\n";
        assert_eq!(strip_line_comments(src), "fn f() {}\n");
    }
    #[test]
    fn strip_line_comments_drops_ordinary_block_comments() {
        let src = "/* kill me */\nfn f() {}\n";
        assert_eq!(strip_line_comments(src), "fn f() {}\n");
    }
    #[test]
    fn strip_line_comments_keeps_doc_comments() {
        let src = "/// keep me\nfn f() {}\n";
        assert_eq!(strip_line_comments(src), src);
        let src = "//! keep me\nfn f() {}\n";
        assert_eq!(strip_line_comments(src), src);
        let src = "/** keep me */\nfn f() {}\n";
        assert_eq!(strip_line_comments(src), src);
    }
    #[test]
    fn strip_line_comments_keeps_safety_idiom() {
        let src = "// SAFETY: hand-written invariant\nfn f() {}\n";
        assert_eq!(strip_line_comments(src), src);
    }
    #[test]
    fn strip_line_comments_keeps_auto_trait_policy_markers() {
        let src = "// AUTO-TRAIT-POLICY-BEGIN\nfn f() {}\n// AUTO-TRAIT-POLICY-END\n";
        assert_eq!(strip_line_comments(src), src);
    }
    #[test]
    fn strip_line_comments_preserves_string_literals_with_marker_text() {
        let src = "const X: &str = \"// not actually a comment\";\nfn f() {}\n";
        assert_eq!(strip_line_comments(src), src);
        let src = "const X: &str = \"// SAFETY: still inside a string\";\nfn f() {}\n";
        assert_eq!(strip_line_comments(src), src);
    }
    #[test]
    fn strip_line_comments_is_a_fixed_point_on_clean_source() {
        let src = "pub fn f(x: u32) -> u32 { x + 1 }\n";
        let once = strip_line_comments(src);
        let twice = strip_line_comments(&once);
        assert_eq!(once, twice);
        assert_eq!(once, src);
    }
    #[test]
    fn strip_line_comments_does_not_reflow_code() {
        let src = "fn f(  x  :  u32  )  ->  u32  {\n    // kill me\n    x  +  1\n}\n";
        let expected = "fn f(  x  :  u32  )  ->  u32  {\n    x  +  1\n}\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_inline_line_comment_trims_preceding_whitespace() {
        let src = "let x = 1; // trailing\nlet y = 2;\n";
        let expected = "let x = 1;\nlet y = 2;\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_inline_block_comment_trims_preceding_whitespace() {
        let src = "let x = 1; /* inline */\nlet y = 2;\n";
        let expected = "let x = 1;\nlet y = 2;\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_inline_trim_handles_multiple_consecutive_trailing_comments() {
        let src = "let x = 1; /* a */ /* b */ // tail\nlet y = 2;\n";
        let expected = "let x = 1;\nlet y = 2;\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_inline_trim_handles_mixed_tabs_and_spaces() {
        let src = "let x = 1;\t \t// tabs\nlet y = 2;\n";
        let expected = "let x = 1;\nlet y = 2;\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_inline_trim_drop_receiver_pattern() {
        let src = "drop(rx); // close receiver\n";
        let expected = "drop(rx);\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_inline_trim_does_not_touch_line_with_no_removed_comment() {
        let src = "let x = 1;   \nlet y = 2;\n";
        assert_eq!(strip_line_comments(src), src);
    }
    #[test]
    fn strip_line_comments_inline_trim_does_not_touch_safety_line() {
        let src = "let x = 1; // SAFETY: invariant\nlet y = 2;\n";
        assert_eq!(strip_line_comments(src), src);
    }
    #[test]
    fn strip_line_comments_inline_trim_does_not_touch_doc_comment() {
        let src = "let x = 1; /// doc-shaped (illegal but lexer keeps it)\n";
        assert_eq!(strip_line_comments(src), src);
    }
    #[test]
    fn strip_line_comments_inline_trim_already_clean_is_noop() {
        let src = "let x = 1;\nlet y = 2;\n";
        assert_eq!(strip_line_comments(src), src);
    }
}
#[cfg(test)]
mod doc_lint_tests {
    use super::{DocBudget, doc_lint_file};
    use syn::parse_quote;
    fn lint(file: &syn::File, max_words: usize) -> Vec<super::DocFinding> {
        doc_lint_file(file, DocBudget { max_words })
    }
    #[test]
    fn no_docs_yields_no_findings() {
        let f: syn::File = parse_quote! {
            pub fn foo() {}
        };
        assert!(lint(&f, 40).is_empty());
    }
    #[test]
    fn short_doc_under_budget_yields_no_findings() {
        let f: syn::File = parse_quote! {
            #[doc = " one two three four five"] pub fn foo() {}
        };
        assert!(lint(&f, 40).is_empty());
    }
    #[test]
    fn long_doc_over_budget_yields_one_finding() {
        let f: syn::File = parse_quote! {
            #[doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[doc =
            " w11 w12 w13 w14 w15 w16 w17 w18 w19 w20"] #[doc =
            " w21 w22 w23 w24 w25 w26 w27 w28 w29 w30"] #[doc =
            " w31 w32 w33 w34 w35 w36 w37 w38 w39 w40"] #[doc =
            " w41 w42 w43 w44 w45 w46 w47 w48 w49 w50"] pub fn foo() {}
        };
        let findings = lint(&f, 40);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].word_count, 50);
        assert_eq!(findings[0].budget, 40);
        assert_eq!(findings[0].item_label, "fn foo");
    }
    #[test]
    fn fenced_code_excluded_brings_under_budget() {
        let f: syn::File = parse_quote! {
            #[doc = " p01 p02 p03 p04 p05 p06 p07 p08 p09 p10"] #[doc = " ```"] #[doc =
            " c01 c02 c03 c04 c05 c06 c07 c08 c09 c10"] #[doc =
            " c11 c12 c13 c14 c15 c16 c17 c18 c19 c20"] #[doc =
            " c21 c22 c23 c24 c25 c26 c27 c28 c29 c30"] #[doc =
            " c31 c32 c33 c34 c35 c36 c37 c38 c39 c40"] #[doc =
            " c41 c42 c43 c44 c45 c46 c47 c48 c49 c50"] #[doc = " ```"] pub fn foo() {}
        };
        let findings = lint(&f, 40);
        assert!(findings.is_empty(), "{findings:?}");
    }
    #[test]
    fn multi_attr_docs_concatenate() {
        let f: syn::File = parse_quote! {
            #[doc = " w01 w02 w03 w04 w05"] #[doc = " w06 w07 w08 w09 w10"] #[doc =
            "w11 w12 w13 w14 w15"] pub fn foo() {}
        };
        let findings = lint(&f, 10);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].word_count, 15);
    }
    #[test]
    fn cfg_attr_doc_payload_counted() {
        let f: syn::File = parse_quote! {
            #[doc = " w01 w02 w03 w04 w05"] #[cfg_attr(test, doc =
            "w06 w07 w08 w09 w10")] pub fn foo() {}
        };
        let findings = lint(&f, 7);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].word_count, 10);
    }
    #[test]
    fn doc_inside_macro_rules_not_linted() {
        let f: syn::File = parse_quote! {
            macro_rules! noisy { () => { #[doc =
            " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[doc =
            " w11 w12 w13 w14 w15 w16 w17 w18 w19 w20"] #[doc =
            " w21 w22 w23 w24 w25 w26 w27 w28 w29 w30"] #[doc =
            " w31 w32 w33 w34 w35 w36 w37 w38 w39 w40"] #[doc =
            " w41 w42 w43 w44 w45 w46 w47 w48 w49 w50"] pub fn inner() {} }; }
        };
        let findings = lint(&f, 5);
        assert!(findings.is_empty(), "{findings:?}");
    }
    #[test]
    fn field_and_variant_docs_linted_independently() {
        let f: syn::File = parse_quote! {
            pub struct S { #[doc = " w01 w02 w03 w04 w05"] pub a : u32, #[doc =
            " w01 w02 w03 w04 w05 w06"] pub b : u32, }
        };
        let findings = lint(&f, 3);
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(findings.iter().all(|f| f.item_label.starts_with("field ")));
        let f: syn::File = parse_quote! {
            pub enum E { #[doc = " w01 w02 w03 w04 w05"] One, #[doc =
            " w01 w02 w03 w04 w05 w06"] Two, }
        };
        let findings = lint(&f, 3);
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(
            findings
                .iter()
                .all(|f| f.item_label.starts_with("variant "))
        );
    }
    #[test]
    fn closing_fence_returns_to_prose() {
        let f: syn::File = parse_quote! {
            #[doc = " w01 w02 w03 w04 w05"] #[doc = " ```"] #[doc = " c01 c02 c03"] #[doc
            = " ```"] #[doc = " w06 w07 w08 w09 w10 w11"] pub fn foo() {}
        };
        let findings = lint(&f, 10);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].word_count, 11);
    }
    #[test]
    fn equal_to_budget_does_not_trigger() {
        let f: syn::File = parse_quote! {
            #[doc = " w01 w02 w03 w04 w05"] pub fn foo() {}
        };
        assert!(lint(&f, 5).is_empty());
    }
    #[test]
    fn tilde_fence_excludes_code() {
        let f: syn::File = parse_quote! {
            #[doc = " p01 p02 p03 p04 p05 p06 p07 p08 p09 p10"] #[doc = " ~~~"] #[doc =
            " c01 c02 c03 c04 c05 c06 c07 c08 c09 c10"] #[doc =
            " c11 c12 c13 c14 c15 c16 c17 c18 c19 c20"] #[doc =
            " c21 c22 c23 c24 c25 c26 c27 c28 c29 c30"] #[doc =
            " c31 c32 c33 c34 c35 c36 c37 c38 c39 c40"] #[doc =
            " c41 c42 c43 c44 c45 c46 c47 c48 c49 c50"] #[doc = " ~~~"] pub fn foo() {}
        };
        let findings = lint(&f, 40);
        assert!(findings.is_empty(), "{findings:?}");
    }
    #[test]
    fn unclosed_fence_fails_closed() {
        let f: syn::File = parse_quote! {
            #[doc = " p01 p02 p03 p04 p05"] #[doc = " ```"] #[doc =
            " c01 c02 c03 c04 c05 c06 c07 c08 c09 c10"] #[doc =
            " c11 c12 c13 c14 c15 c16 c17 c18 c19 c20"] #[doc =
            " c21 c22 c23 c24 c25 c26 c27 c28 c29 c30"] #[doc =
            " c31 c32 c33 c34 c35 c36 c37 c38 c39 c40"] #[doc =
            " c41 c42 c43 c44 c45 c46 c47 c48 c49 c50"] pub fn foo() {}
        };
        let findings = lint(&f, 40);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].word_count, 56);
        assert!(
            findings[0].fail_closed,
            "unbalanced fence must set fail_closed=true: {:?}",
            findings[0]
        );
    }
    #[test]
    fn over_budget_doc_on_pub_use_is_linted() {
        let f: syn::File = parse_quote! {
            #[doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[doc =
            " w11 w12 w13 w14 w15 w16 w17 w18 w19 w20"] #[doc =
            " w21 w22 w23 w24 w25 w26 w27 w28 w29 w30"] #[doc =
            " w31 w32 w33 w34 w35 w36 w37 w38 w39 w40"] #[doc =
            " w41 w42 w43 w44 w45 w46 w47 w48 w49 w50"] pub use crate ::foo::Bar;
        };
        let findings = lint(&f, 40);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].item_label, "use");
        assert_eq!(findings[0].word_count, 50);
    }
    #[test]
    fn over_budget_doc_on_extern_crate_is_linted() {
        let f: syn::File = parse_quote! {
            #[doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[doc =
            " w11 w12 w13 w14 w15 w16 w17 w18 w19 w20"] #[doc =
            " w21 w22 w23 w24 w25 w26 w27 w28 w29 w30"] #[doc =
            " w31 w32 w33 w34 w35 w36 w37 w38 w39 w40"] #[doc =
            " w41 w42 w43 w44 w45 w46 w47 w48 w49 w50"] extern crate alloc;
        };
        let findings = lint(&f, 40);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].item_label, "extern crate alloc");
    }
}
/// Rewrite mechanically-safe Rust item links in `doc_text`.
///
/// Operates on the prose of a single doc-comment block (concatenated
/// payloads of one item, joined by `\n`). Maintains fenced-code state
/// across lines (` ``` ` and `~~~`); transforms inside a fence are
/// suppressed, as are byte ranges covered by inline code spans
/// (single-backtick pairs).
///
/// Rules applied only when the label is a conservative Rust item
/// token (see [`is_codeish_token`]):
///
/// ```text
/// [Type](Type)             -> [`Type`]              (redundant target collapsed)
/// [Type]                   -> [`Type`]              (shortcut form gets ticks)
/// [label](Target)          -> [`label`](Target)    (label ticked; target kept)
/// ```
///
/// Skipped (left verbatim):
///
/// ```text
/// - lines inside fenced code blocks
/// - spans inside inline code (`code`)
/// - URL targets (contain ://, or start with /, #, mailto:)
/// - reference definitions ([label]: <url>) and reference links ([label][ref])
/// - targets with generics, disambiguators, or fragments (< > @ # ( ) ! ?)
/// - labels already wrapped in backticks (idempotent)
/// - prose labels — anything not matching is_codeish_token
/// - empty link bodies
/// ```
#[must_use]
pub fn rewrite_rustdoc_link_idioms(doc_text: &str) -> String {
    let mut out = String::with_capacity(doc_text.len());
    let mut in_fence = false;
    let mut first = true;
    for line in doc_text.split('\n') {
        if !first {
            out.push('\n');
        }
        first = false;
        let stripped = line.trim_start();
        if stripped.starts_with("```") || stripped.starts_with("~~~") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        if is_reference_definition(line) {
            out.push_str(line);
            continue;
        }
        rewrite_line_links(line, &mut out);
    }
    out
}
/// True if `line` is a Markdown link-reference definition
/// (`[label]: <target>` at the start of the line, ignoring leading whitespace).
fn is_reference_definition(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix('[') else {
        return false;
    };
    let Some(close) = rest.find(']') else {
        return false;
    };
    rest[close + 1..].starts_with(':')
}
/// Apply per-link rewrites to one prose `line`, appending to `out`.
///
/// Iterates the line scanning for `[`. Code-span backticks are tracked so
/// `[Type]` inside `` `code` `` is left verbatim. Each candidate link is
/// classified into [`LinkShape`] and rewritten or skipped accordingly.
///
/// Walks `char_indices` so multi-byte UTF-8 sequences round-trip intact.
/// The bracket / paren matchers operate on ASCII bytes only (`[`, `]`,
/// `(`, `)`, backslash), so the byte indices they return are always char
/// boundaries.
fn rewrite_line_links(line: &str, out: &mut String) {
    let mut chars = line.char_indices().peekable();
    let mut in_code_span = false;
    while let Some(&(i, ch)) = chars.peek() {
        if ch == '`' {
            in_code_span = !in_code_span;
            out.push('`');
            chars.next();
            continue;
        }
        if in_code_span || ch != '[' {
            out.push(ch);
            chars.next();
            continue;
        }
        if let Some((shape, consumed)) = parse_link_at(line, i) {
            emit_link(out, &shape);
            while let Some(&(j, _)) = chars.peek() {
                if j >= i + consumed {
                    break;
                }
                chars.next();
            }
        } else {
            out.push('[');
            chars.next();
        }
    }
}
/// Markdown link shapes recognised (and possibly rewritten) by
/// [`rewrite_rustdoc_link_idioms`]. Source spans are preserved verbatim
/// in the `*_src` fields so unrecognised cases can be re-emitted unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LinkShape {
    /// `[label](target)` — explicit inline link.
    Inline {
        label_src: String,
        target_src: String,
    },
    /// `[label][ref]` — reference-style link. Always preserved verbatim.
    Reference { raw: String },
    /// `[label]` — shortcut reference / candidate intra-doc link.
    Shortcut { label_src: String },
}
/// Parse a link starting at byte offset `start` (which must be `[`).
/// Returns `Some((shape, bytes_consumed))` if a complete link was parsed,
/// `None` if the `[` is not part of a recognisable link (e.g. unmatched).
fn parse_link_at(line: &str, start: usize) -> Option<(LinkShape, usize)> {
    let bytes = line.as_bytes();
    let label_end = find_matching_bracket(line, start)?;
    let label_src = line[start + 1..label_end].to_string();
    let after_label = label_end + 1;
    if after_label < bytes.len() && bytes[after_label] == b'(' {
        let paren_end = find_matching_paren(line, after_label)?;
        let target_src = line[after_label + 1..paren_end].to_string();
        return Some((
            LinkShape::Inline {
                label_src,
                target_src,
            },
            paren_end + 1 - start,
        ));
    }
    if after_label < bytes.len() && bytes[after_label] == b'[' {
        let ref_end = find_matching_bracket(line, after_label)?;
        let raw = line[start..=ref_end].to_string();
        return Some((LinkShape::Reference { raw }, ref_end + 1 - start));
    }
    Some((LinkShape::Shortcut { label_src }, after_label - start))
}
/// Find the matching `]` for the `[` at `open`. Backslash-escaped
/// brackets are skipped. Nested `[` / `]` inside an item link label
/// is uncommon; treat the first unescaped `]` as the close. Returns
/// `None` if no close is found on `line`.
fn find_matching_bracket(line: &str, open: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b']' => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}
/// Find the matching `)` for the `(` at `open`, respecting backslash
/// escapes. Returns `None` if no close is found on `line`.
fn find_matching_paren(line: &str, open: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b')' => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}
/// Decide how to re-emit a parsed link.
fn emit_link(out: &mut String, shape: &LinkShape) {
    match shape {
        LinkShape::Reference { raw } => {
            out.push_str(raw);
        }
        LinkShape::Inline {
            label_src,
            target_src,
        } => emit_inline_link(out, label_src, target_src),
        LinkShape::Shortcut { label_src } => emit_shortcut_link(out, label_src),
    }
}
/// Re-emit `[label](target)`, possibly with idiom rewrites.
fn emit_inline_link(out: &mut String, label_src: &str, target_src: &str) {
    let target_trim = target_src.trim();
    if target_trim.is_empty() || !is_safe_intra_doc_target(target_trim) {
        write_inline(out, label_src, target_src);
        return;
    }
    if label_src == target_trim && is_codeish_token(label_src) {
        write_shortcut_ticked(out, label_src);
        return;
    }
    if is_codeish_token(label_src) && !label_src_has_backticks(label_src) {
        write_inline_label_ticked(out, label_src, target_src);
        return;
    }
    write_inline(out, label_src, target_src);
}
/// Re-emit `[label]`, possibly ticking when the label is code-ish.
fn emit_shortcut_link(out: &mut String, label_src: &str) {
    if is_codeish_token(label_src) && !label_src_has_backticks(label_src) {
        write_shortcut_ticked(out, label_src);
    } else {
        out.push('[');
        out.push_str(label_src);
        out.push(']');
    }
}
fn write_shortcut_ticked(out: &mut String, label: &str) {
    out.push('[');
    out.push('`');
    out.push_str(label);
    out.push('`');
    out.push(']');
}
fn write_inline_label_ticked(out: &mut String, label: &str, target: &str) {
    out.push('[');
    out.push('`');
    out.push_str(label);
    out.push('`');
    out.push(']');
    out.push('(');
    out.push_str(target);
    out.push(')');
}
fn write_inline(out: &mut String, label: &str, target: &str) {
    out.push('[');
    out.push_str(label);
    out.push(']');
    out.push('(');
    out.push_str(target);
    out.push(')');
}
/// True if the label already contains literal backticks. Such labels
/// are left verbatim — the user has already chosen their wrapping.
fn label_src_has_backticks(label: &str) -> bool {
    label.contains('`')
}
/// True if `target` is a safe intra-doc-link target for mechanical
/// rewrite: no URL scheme, no fragment, no generic / disambiguator /
/// argument syntax. Pure paths of identifiers separated by `::`,
/// optionally prefixed with `crate`, `self`, `super`, or `Self`.
fn is_safe_intra_doc_target(target: &str) -> bool {
    if target.is_empty() {
        return false;
    }
    if target.contains("://")
        || target.starts_with('#')
        || target.starts_with('/')
        || target.starts_with("mailto:")
    {
        return false;
    }
    for ch in target.chars() {
        match ch {
            '<' | '>' | '@' | '#' | '(' | ')' | '!' | '?' | ' ' | '\t' => return false,
            _ => {}
        }
    }
    is_codeish_path(target)
}
/// True if `s` is a single code-ish Rust item token:
/// `CamelCase`, `snake_case` identifier, path-with-`::`,
/// or one of `Self` / `self` / `super` / `crate`.
#[must_use]
pub fn is_codeish_token(s: &str) -> bool {
    is_codeish_path(s)
}
/// True if `s` is a syntactically plausible Rust path:
/// `::`-separated segments, each segment a non-empty ident
/// (`[A-Za-z_][A-Za-z0-9_]*`). Leading `::` is permitted.
fn is_codeish_path(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let trimmed = s.strip_prefix("::").unwrap_or(s);
    if trimmed.is_empty() {
        return false;
    }
    for segment in trimmed.split("::") {
        if !is_rust_ident(segment) {
            return false;
        }
    }
    true
}
fn is_rust_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    if s.len() == 1 && first == '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
#[cfg(test)]
mod rustdoc_link_idiom_tests {
    use super::{is_codeish_token, rewrite_rustdoc_link_idioms};
    #[test]
    fn multibyte_utf8_survives_rewrite_pure() {
        let input = "see [Type] — also русский and 🦀";
        let out = super::rewrite_rustdoc_link_idioms(input);
        assert_eq!(out, "see [`Type`] — also русский and 🦀");
    }
    #[test]
    fn em_dash_survives_litstr_new_via_set_payload() {
        use syn::Attribute;
        use syn::parse_quote;
        let mut a: Attribute = parse_quote!(#[doc = " starting payload"]);
        super::set_doc_string_payload(&mut a, " hello — world");
        let payload = super::doc_string_payload(&a).unwrap();
        assert!(
            payload.contains('—'),
            "em-dash lost via set_doc_string_payload: payload={payload:?}"
        );
    }
    fn rw(s: &str) -> String {
        rewrite_rustdoc_link_idioms(s)
    }
    #[test]
    fn redundant_explicit_link_collapses_and_ticks() {
        assert_eq!(rw("see [Type](Type) for"), "see [`Type`] for");
    }
    #[test]
    fn redundant_explicit_path_link_collapses_and_ticks() {
        assert_eq!(rw("via [foo::Bar](foo::Bar) here"), "via [`foo::Bar`] here");
    }
    #[test]
    fn explicit_target_retained_label_ticked() {
        assert_eq!(
            rw("call [begin](Self::begin) first"),
            "call [`begin`](Self::begin) first"
        );
        assert_eq!(
            rw("see [Reader](crate::Reader) docs"),
            "see [`Reader`](crate::Reader) docs"
        );
    }
    #[test]
    fn shortcut_camel_case_ticked() {
        assert_eq!(rw("the [Type] applies"), "the [`Type`] applies");
    }
    #[test]
    fn shortcut_path_ticked() {
        assert_eq!(rw("see [foo::Bar] above"), "see [`foo::Bar`] above");
    }
    #[test]
    fn shortcut_self_super_crate_ticked() {
        assert_eq!(rw("the [Self] of"), "the [`Self`] of");
        assert_eq!(rw("from [super::Foo]"), "from [`super::Foo`]");
        assert_eq!(rw("via [crate::Reader]"), "via [`crate::Reader`]");
    }
    #[test]
    fn shortcut_snake_case_ticked() {
        assert_eq!(rw("call [do_thing] next"), "call [`do_thing`] next");
    }
    #[test]
    fn prose_label_not_rewritten() {
        assert_eq!(
            rw("see [the writer](Writer) for"),
            "see [the writer](Writer) for"
        );
    }
    #[test]
    fn url_link_not_rewritten() {
        assert_eq!(
            rw("see [docs](https://example.com)"),
            "see [docs](https://example.com)"
        );
        assert_eq!(rw("the [home](/index.html)"), "the [home](/index.html)");
        assert_eq!(
            rw("mail [admin](mailto:a@example.com)"),
            "mail [admin](mailto:a@example.com)"
        );
    }
    #[test]
    fn fragment_target_not_rewritten() {
        assert_eq!(rw("see [Foo](#anchor)"), "see [Foo](#anchor)");
    }
    #[test]
    fn target_with_generics_not_rewritten() {
        assert_eq!(rw("see [Vec](Vec<u8>) usage"), "see [Vec](Vec<u8>) usage");
    }
    #[test]
    fn target_with_disambiguator_not_rewritten() {
        assert_eq!(rw("call [foo](foo()) here"), "call [foo](foo()) here");
        assert_eq!(rw("call [m](m!) macro"), "call [m](m!) macro");
        assert_eq!(
            rw("see [t](struct@Type) struct"),
            "see [t](struct@Type) struct"
        );
    }
    #[test]
    fn reference_style_link_not_rewritten() {
        assert_eq!(rw("see [Type][ref] later"), "see [Type][ref] later");
    }
    #[test]
    fn reference_definition_not_rewritten() {
        assert_eq!(
            rw("[ref]: https://example.com"),
            "[ref]: https://example.com"
        );
    }
    #[test]
    fn fenced_code_block_left_verbatim() {
        let input = "before\n```\nlet x: [Type] = foo();\n[Type](Type)\n```\nafter [Type]";
        let expected = "before\n```\nlet x: [Type] = foo();\n[Type](Type)\n```\nafter [`Type`]";
        assert_eq!(rw(input), expected);
    }
    #[test]
    fn tilde_fenced_code_block_left_verbatim() {
        let input = "before\n~~~\n[Type](Type)\n~~~\nafter [Type]";
        let expected = "before\n~~~\n[Type](Type)\n~~~\nafter [`Type`]";
        assert_eq!(rw(input), expected);
    }
    #[test]
    fn inline_code_span_left_verbatim() {
        assert_eq!(
            rw("use `[Type]` syntax for [Type]"),
            "use `[Type]` syntax for [`Type`]"
        );
    }
    #[test]
    fn already_ticked_shortcut_left_verbatim() {
        assert_eq!(rw("see [`Type`] above"), "see [`Type`] above");
    }
    #[test]
    fn already_ticked_inline_left_verbatim() {
        assert_eq!(
            rw("call [`foo`](Self::foo) now"),
            "call [`foo`](Self::foo) now"
        );
    }
    #[test]
    fn empty_link_body_not_rewritten() {
        assert_eq!(rw("an [] empty"), "an [] empty");
    }
    #[test]
    fn empty_target_not_rewritten() {
        assert_eq!(rw("a [Type]() blank"), "a [Type]() blank");
    }
    #[test]
    fn is_codeish_token_basic() {
        assert!(is_codeish_token("Type"));
        assert!(is_codeish_token("foo_bar"));
        assert!(is_codeish_token("foo::Bar"));
        assert!(is_codeish_token("Self"));
        assert!(is_codeish_token("self"));
        assert!(is_codeish_token("super::Foo"));
        assert!(is_codeish_token("crate::Reader"));
        assert!(is_codeish_token("::foo::Bar"));
        assert!(!is_codeish_token(""));
        assert!(!is_codeish_token("two words"));
        assert!(!is_codeish_token("foo()"));
        assert!(!is_codeish_token("Vec<u8>"));
        assert!(!is_codeish_token("foo!"));
        assert!(!is_codeish_token("_"));
        assert!(!is_codeish_token("9bad"));
        assert!(!is_codeish_token("foo:bar"));
        assert!(!is_codeish_token("foo::"));
    }
    #[test]
    fn idempotent_rewrite() {
        let inputs = [
            "see [Type](Type)",
            "call [begin](Self::begin)",
            "the [Type]",
            "see `[Type]` literal",
            "in a fence\n```\n[Type]\n```\nout",
            "see [the writer](Writer)",
        ];
        for input in inputs {
            let once = rw(input);
            let twice = rw(&once);
            assert_eq!(once, twice, "non-idempotent for: {input}");
        }
    }
    #[test]
    fn multiline_fence_state_persists() {
        let input = "p1\n```\n[A](A)\n```\np2 [B]\n```\n[C]\n```\np3 [D](D)";
        let expected = "p1\n```\n[A](A)\n```\np2 [`B`]\n```\n[C]\n```\np3 [`D`]";
        assert_eq!(rw(input), expected);
    }
    #[test]
    fn multiple_links_on_one_line() {
        assert_eq!(
            rw("see [A] and [B](B) then [C](Self::C)"),
            "see [`A`] and [`B`] then [`C`](Self::C)"
        );
    }
    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(rw(""), "");
    }
    #[test]
    fn no_links_passthrough() {
        let s = "plain prose with no links\nand a second line\n";
        assert_eq!(rw(s), s);
    }
}
#[cfg(test)]
mod doc_path_tests {
    use super::is_doc_path;
    use std::path::Path;
    #[test]
    fn bare_doc_stem_requires_exact_match() {
        let root = Path::new("");
        assert!(is_doc_path(Path::new("README"), root));
        assert!(is_doc_path(Path::new("README.md"), root));
        assert!(is_doc_path(Path::new("LICENSE"), root));
        assert!(!is_doc_path(Path::new("READMEISH"), root));
        assert!(!is_doc_path(Path::new("READMEISH.rs"), root));
        assert!(!is_doc_path(Path::new("LICENSEABLE.rs"), root));
        assert!(!is_doc_path(Path::new("NOTICED.rs"), root));
    }
    #[test]
    fn docs_dir_matches_only_at_top_level_under_root() {
        let root = Path::new("/proj");
        assert!(is_doc_path(Path::new("/proj/docs/guide.rs"), root));
        assert!(is_doc_path(Path::new("/proj/doc/inner.rs"), root));
        assert!(!is_doc_path(Path::new("/proj/src/docs/mod.rs"), root));
        assert!(!is_doc_path(
            Path::new("/proj/crates/foo/doc/inner.rs"),
            root
        ));
        assert!(!is_doc_path(Path::new("/proj/src/doc/util.rs"), root));
    }
}
