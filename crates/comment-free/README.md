# comment-free

Doc-comment linter and byte-preserving doc-payload rewriter for Rust crates.

Default mode walks `ROOT` for `.rs` files and reports doc comments whose prose
word count exceeds a budget. Fenced code blocks (` ``` ` or `~~~`) are excluded
from the count; the doctrine allows 0-3 such fenced examples per doc comment
and they do not consume the word budget. Examples are detected mechanically by
fence delimiters only — there is no semantic example detection.

The rewrite mode (`--rewrite`) is a **byte-preserving doc-payload-only**
pass: it mutates only `///`, `//!`, `#[doc = "..."]`, `#![doc = "..."]`, and
`#[cfg_attr(_, doc = "...")]` payload text via a small set of mechanically-safe
rustdoc intra-doc link normalisations. Every other byte in the source —
including non-doc `//` and `/* */` comments, whitespace, attribute placement,
and marker tokens inside string literals — is preserved verbatim. `rustfmt` is
not invoked; block doc comments (`/** */`) are left untouched. Idempotent and
line-count-preserving. Safe to dogfood.

Traversal is restricted to `.rs` files under `crates/` and `src/`. `target/`
and dotted directories are skipped.

## Exit codes

| Code | Meaning                                                            |
|------|--------------------------------------------------------------------|
| 0    | clean (no findings, no errors)                                     |
| 1    | catastrophic / unmapped IO error                                   |
| 2    | invalid CLI arguments (clap rejection)                             |
| 4    | doc-lint findings observed (default mode)                          |
| 5    | per-file parse/IO errors observed during processing (both modes)   |

Findings (`DOC_LINT`, `REWRITE`, `WOULD_REWRITE`, diffs) go to stdout.
Metadata (`SUMMARY`, `DOC_WARN`, errors) goes to stderr.

## CLI invocations

### Default: doc-comment lint

Read-only. Walks `ROOT` (default `.`) and emits one `DOC_LINT` line per
over-budget doc comment, followed by a doctrine `DOC_LINT_MSG`. Exits 4 on
any finding.

The doctrine allows 0-3 fenced code examples (` ``` ` or `~~~` blocks) per
doc comment; their content does not count toward `--doc-max-words`. Examples
are detected mechanically by fence delimiters only — there is no semantic
example detection, and no rewriting of example content. Rendering, links, and
formatting of example bodies remain the responsibility of `rustdoc`,
`clippy`, and `rustfmt`.

CommonMark/Markdown reference-style link definition lines (`[label]: url`,
optionally indented) are also excluded from the prose word count — they are
URL bookkeeping, not prose. Ordinary inline links (`[label](target)`) and
shortcut references (`[label]`) are still counted as one whitespace token
each, because they are part of the prose body.

```sh
comment-free
comment-free crates/foo
comment-free --doc-max-words 200
comment-free --doc-max-words 60 crates/foo
```

### Rewrite: byte-preserving doc-payload rewrite

Mechanically normalises a small set of safe Rust intra-doc link forms inside
doc-comment and doc-attribute payloads. Every non-doc byte is preserved
verbatim. Emits one `REWRITE` line per modified file.

```sh
comment-free --rewrite
comment-free --rewrite crates/foo
```

Rewrites applied (only when the label is a conservative Rust item token
— `CamelCase`, `snake_case`, path-with-`::`, or `Self` / `self` /
`super` / `crate`):

| Before                       | After                              |
|------------------------------|------------------------------------|
| `[Type](Type)`               | `` [`Type`] ``                     |
| `[foo::Bar](foo::Bar)`       | `` [`foo::Bar`] ``                 |
| `[Type]` (when code-ish)     | `` [`Type`] ``                     |
| `[begin](Self::begin)`       | `` [`begin`](Self::begin) ``       |
| `[Reader](crate::Reader)`    | `` [`Reader`](crate::Reader) ``    |

Skipped (left verbatim):

- Lines inside fenced code blocks (` ``` ` or `~~~`)
- Spans inside inline code (`` `Type` ``)
- URL targets (`https://…`, `/…`, `#…`, `mailto:…`)
- Reference-style links (`[Type][ref]`) and their definitions
  (`[ref]: …`)
- Prose labels with whitespace or non-ident syntax
  (e.g. `[the writer](Writer)`)
- Targets with generics, disambiguators, or fragments
  (`Vec<u8>`, `foo()`, `m!`, `struct@Type`, `Type#variant`)
- Labels already wrapped in backticks (idempotent)
- Block doc comments (`/** */`) — the entire attribute is left verbatim

The transform is line-count-preserving and reapplies cleanly
(idempotent). Non-doc bytes are guaranteed unchanged.

### Rewrite preview (dry-run)

Prints `WOULD_REWRITE` plus a unified diff per file; writes nothing. Safe on
a dirty tree.

```sh
comment-free --rewrite --dry-run
comment-free --rewrite --dry-run --context 5
comment-free --rewrite -n crates/foo
```

## Flag matrix

| Flag                      | Mode      | Default | Notes                                              |
|---------------------------|-----------|---------|----------------------------------------------------|
| `ROOT` (positional)       | both      | `.`     | Must be a directory                                |
| `--doc-max-words N`       | lint      | `120`   | Prose word budget; fenced code (``` or ~~~) and reference-style link definitions (`[label]: url`) excluded; 0-3 fenced examples allowed |
| `--rewrite`               | rewrite   | off     | Switches to rewrite mode                           |
| `--dry-run` / `-n`        | rewrite   | off     | Requires `--rewrite`; prints diffs, writes nothing |
| `--context N`             | rewrite   | `3`     | Requires `--rewrite`; unified-diff context lines   |

## Caveats

- Mutates only doc-comment and doc-attribute payload text. All other
  source bytes — including non-doc `//` and `/* */` comments, whitespace,
  attribute placement, marker tokens inside string literals — are preserved
  verbatim.
- Block doc comments (`/** */`) are left untouched. The lexer strips
  leading `*` so the in-memory payload diverges from the source bytes;
  this tool refuses to risk a corrupted round-trip on such surfaces.
- Does NOT require `rustfmt` to be installed.
- Does NOT require a clean git working tree; there is no clean-tree gate.
- Idempotent; safe to run repeatedly.
